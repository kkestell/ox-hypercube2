#!/usr/bin/env python3
"""Run opt-in smoke checks against an ACP bridge through oxd."""

import argparse
import json
import os
import select
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import traceback
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


def check(condition, message):
    if not condition:
        raise RuntimeError(message)


def command(args, *, cwd=REPO, env=None, timeout=30):
    result = subprocess.run(
        args,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    check(result.returncode == 0, f"{args[0]} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def stop(process):
    if process.poll() is None:
        process.kill()
    process.wait(timeout=10)


def running_after(pids, timeout):
    """Waits up to `timeout` seconds for each process to exit and returns the
    ones still running. Exits arrive as kqueue events on macOS and as readable
    pidfds on Linux."""
    deadline = time.monotonic() + timeout
    if hasattr(select, "kqueue"):
        queue = select.kqueue()
        running = set()
        for pid in pids:
            event = select.kevent(
                pid,
                select.KQ_FILTER_PROC,
                select.KQ_EV_ADD | select.KQ_EV_ONESHOT,
                select.KQ_NOTE_EXIT,
            )
            try:
                queue.control([event], 0)
                running.add(pid)
            except ProcessLookupError:
                pass
        while running and (remaining := deadline - time.monotonic()) > 0:
            for event in queue.control(None, len(running), remaining):
                running.discard(event.ident)
        queue.close()
        return running
    descriptors = {}
    for pid in pids:
        try:
            descriptors[os.pidfd_open(pid)] = pid
        except ProcessLookupError:
            pass
    poll = select.poll()
    for descriptor in descriptors:
        poll.register(descriptor, select.POLLIN)
    while descriptors and (remaining := deadline - time.monotonic()) > 0:
        for descriptor, _ in poll.poll(remaining * 1000):
            poll.unregister(descriptor)
            os.close(descriptor)
            del descriptors[descriptor]
    for descriptor in descriptors:
        os.close(descriptor)
    return set(descriptors.values())


def kind(event):
    return event["kind"]["type"]


def update(event):
    return event["kind"]["data"] if kind(event) == "update" else {}


def message(event):
    return update(event).get("sessionUpdate") in (
        "agent_message_chunk",
        "agent_message",
    )


def blocks_text(blocks):
    return "".join(b.get("text", "") for b in blocks or [])


def message_text(data):
    """The text of a message update: a chunk's one block, or a whole
    message's list of blocks."""
    if "content" in data and isinstance(data["content"], dict):
        return data["content"].get("text", "")
    return blocks_text(data.get("content"))


def text(events):
    return "".join(message_text(update(e)) for e in events if message(e))


def user_text(events):
    return "".join(
        message_text(update(e))
        for e in events
        if update(e).get("sessionUpdate") in ("user_message", "user_message_chunk")
    )


def turn_result(stop_reason):
    """The data of a `turn_ended` event whose turn ended with `stop_reason`."""
    return {"stop_reason": {"stopReason": stop_reason}, "error": None}


class Connection:
    """A connection to oxd's socket. Each message is one line of JSON: a
    request is answered by the message with its `id`, and a stream's messages
    name the request that started it as `subscription`."""

    def __init__(self, path, timeout):
        self.timeout = timeout
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.connect(str(path))
        self.buffer = b""
        self.next_id = 1
        self.call("hello", protocol_version=1)

    def read(self, deadline):
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            check(remaining > 0, "Timed out waiting for the daemon")
            ready, _, _ = select.select([self.socket], [], [], remaining)
            check(ready, "Timed out waiting for the daemon")
            data = self.socket.recv(65536)
            check(data, "The daemon closed the connection")
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def call(self, method, **params):
        """Sends a request and returns its result."""
        id = self.next_id
        self.next_id += 1
        request = {"id": id, "method": method}
        if params:
            request["params"] = params
        self.socket.sendall(json.dumps(request).encode() + b"\n")
        deadline = time.monotonic() + self.timeout
        while True:
            reply = self.read(deadline)
            if reply.get("id") == id:
                check("error" not in reply, f"{method} failed: {reply.get('error')}")
                return reply["result"]

    def changes(self, deadline):
        """The next changes of the connection's watch."""
        while True:
            message = self.read(deadline)
            if "changes" in message:
                return message["changes"]

    def close(self):
        self.socket.close()


class Attachment:
    """A subscription to a session's events after `after`, which logs each
    event it reads to `log`."""

    def __init__(self, path, session, after, timeout, log):
        self.connection = Connection(path, timeout)
        self.connection.call("session.subscribe", session_id=session, after_seq=after)
        self.log = log

    def read(self, deadline):
        message = self.connection.read(deadline)
        while "event" not in message:
            message = self.connection.read(deadline)
        event = message["event"]
        self.log.write(json.dumps(event) + "\n")
        self.log.flush()
        return event

    def close(self):
        self.connection.close()


class Watch:
    """A daemon watch on the socket, read without attaching to any session.
    A watch does not count as a viewer."""

    def __init__(self, path, session, timeout):
        self.session = session
        self.timeout = timeout
        self.connection = Connection(path, timeout)
        self.connection.call("watch")

    def session_changes(self, deadline):
        """The session's info from each of the next listing changes that
        names it."""
        while True:
            for change in self.connection.changes(deadline):
                if (
                    change["type"] == "session"
                    and change["session"]["id"] == self.session
                ):
                    yield change["session"]
                check(
                    change != {"type": "session_removed", "session_id": self.session},
                    "The session left the listing",
                )

    def until(self, state):
        """Waits for a change that shows the session in `state` and returns
        the states changed through on the way."""
        states = []
        for session in self.session_changes(time.monotonic() + self.timeout):
            states.append(session["state"])
            if session["state"] == state:
                return states

    def close(self):
        self.connection.close()


class SmokeRun:
    def __init__(self, args, directory, binaries, events, daemon_args=()):
        self.args = args
        self.daemon_args = list(daemon_args)
        self.directory = directory
        self.project = directory / "project"
        self.project.mkdir()
        self.write_project()
        self.socket = directory / "oxd.sock"
        self.binaries = binaries
        self.events = events
        self.env = os.environ.copy()
        self.agent = [str(binaries / self.bridge)]
        self.daemon = None
        self.workspace = None
        self.session = None
        self.attachment = None
        self.agent_pids = set()
        self.seq = 0
        self.results = []

    def report(self, name, **details):
        result = {"check": name, **details}
        self.results.append(result)
        (self.directory / "results.json").write_text(
            json.dumps(self.results, indent=2) + "\n"
        )
        print(json.dumps(result), flush=True)

    def call(self, method, **params):
        """Sends one request on a connection of its own and returns its
        result."""
        connection = Connection(self.socket, self.args.timeout)
        try:
            return connection.call(method, **params)
        finally:
            connection.close()

    def listing(self):
        """Every workspace, worktree, session and run, as a watch's first
        changes list them."""
        connection = Connection(self.socket, self.args.timeout)
        try:
            connection.call("watch")
            return connection.changes(time.monotonic() + self.args.timeout)
        finally:
            connection.close()

    def workspaces(self):
        return [c["workspace"] for c in self.listing() if c["type"] == "workspace"]

    def sessions(self):
        return [c["session"] for c in self.listing() if c["type"] == "session"]

    def session_info(self):
        """The session's info, or None once it is not listed."""
        return next((s for s in self.sessions() if s["id"] == self.session), None)

    def add_workspace(self, path):
        return self.call("workspace.add", path=str(path))["id"]

    def remove_workspace(self, workspace):
        self.call("workspace.remove", workspace_id=workspace)

    def start_session(self, workspace):
        """Starts a session of the bridge in `workspace`, waits for its agent
        to start, and returns the session's id. A session whose agent fails
        to start is deleted."""
        session = self.call(
            "session.new", workspace_id=workspace, command=self.agent, env=self.env
        )["id"]
        connection = Connection(self.socket, self.args.timeout)
        try:
            connection.call("session.subscribe", session_id=session, after_seq=0)
            deadline = time.monotonic() + self.args.timeout
            while True:
                event = connection.read(deadline).get("event")
                if event is None:
                    continue
                if kind(event) == "agent_started":
                    return session
                if kind(event) == "agent_exited":
                    self.call(
                        "session.delete", session_id=session, discard=True, env=self.env
                    )
                    raise RuntimeError(f"The session did not start: {event}")
        finally:
            connection.close()

    def load_session(self):
        """Restarts the session's agent and waits for it to start."""
        self.call("session.load", session_id=self.session, env=self.env)
        watch = Watch(self.socket, self.session, self.args.timeout)
        try:
            deadline = time.monotonic() + self.args.timeout
            for session in watch.session_changes(deadline):
                check(session["state"] != "exited", "The loaded agent did not start")
                if session["state"] != "starting":
                    return
        finally:
            watch.close()

    def delete_session(self):
        self.call("session.delete", session_id=self.session, env=self.env)

    def prompt(self, text):
        self.call(
            "session.prompt",
            session_id=self.session,
            prompt=[{"type": "text", "text": text}],
        )

    def queue_add(self, text):
        self.call(
            "session.queue_add",
            session_id=self.session,
            prompt=[{"type": "text", "text": text}],
        )

    def set_option(self, option, value):
        """Sets a select option and returns the agent's resulting value."""
        options = self.call(
            "session.set_config_option",
            session_id=self.session,
            config_id=option,
            value=value,
        )
        return next(o for o in options if o["configId"] == option)["currentValue"]

    def permit(self, request_id, option_id):
        self.call(
            "permission.respond",
            session_id=self.session,
            request_id=request_id,
            option_id=option_id,
        )

    def cancel(self):
        self.call("session.cancel", session_id=self.session)

    def answer(self, request_id, content):
        """Accepts the session's form `request_id` with `content`."""
        self.call(
            "elicitation.respond",
            session_id=self.session,
            request_id=request_id,
            action="accept",
            content=content,
        )

    def attach(self, after):
        if self.attachment is not None:
            self.attachment.close()
        self.attachment = Attachment(
            self.socket, self.session, after, self.args.timeout, self.events
        )

    def disconnect(self):
        self.attachment.close()
        self.attachment = None

    def until(self, predicate):
        deadline = time.monotonic() + self.args.timeout
        events = []
        while True:
            event = self.attachment.read(deadline)
            check(
                event["session_id"] == self.session, "Event belongs to another session"
            )
            check(event["seq"] == self.seq + 1, f"Sequence gap or duplicate: {event}")
            self.seq = event["seq"]
            events.append(event)
            if predicate(event):
                return events
            check(
                kind(event) not in ("turn_ended", "agent_exited"),
                f"Turn stopped before the expected event: {event}; text: {text(events)}",
            )

    def finish(self, stop_reason="end_turn"):
        events = self.until(lambda e: kind(e) == "turn_ended")
        check(
            events[-1]["kind"]["data"] == turn_result(stop_reason),
            f"Unexpected turn result: {events[-1]}",
        )
        check("Error:" not in text(events), f"Agent reported an error: {text(events)}")
        return events

    def remember_agents(self):
        # ACP agents own separate process groups; retain their PIDs for cleanup
        # if session startup or a later command fails.
        rows = command(["ps", "-axo", "pid=,ppid="]).splitlines()
        self.agent_pids.update(
            int(pid)
            for pid, parent in (row.split() for row in rows)
            if int(parent) == self.daemon.pid
        )

    def start_daemon(self):
        self.daemon = subprocess.Popen(
            [
                str(self.binaries / "oxd"),
                "--socket",
                str(self.socket),
                *self.daemon_args,
            ],
            stdout=subprocess.PIPE,
            stderr=self.daemon_log,
        )
        # oxd prints `ready` once it is listening and nothing else on stdout.
        with self.daemon.stdout:
            ready, _, _ = select.select([self.daemon.stdout], [], [], self.args.timeout)
            check(ready, "Daemon did not become ready")
            check(
                self.daemon.stdout.readline() == b"ready\n",
                "Daemon exited during startup; see oxd.log",
            )

    def run(self, daemon_log):
        self.daemon_log = daemon_log
        self.start_daemon()
        self.workspace = self.add_workspace(self.project)
        self.session = self.start_session(self.workspace)
        self.set_option("model", self.args.model)
        self.remember_agents()
        check(self.agent_pids, "Could not find the daemon's ACP agent process")
        check(self.session_info()["state"] == "idle", "New session is not idle")
        self.report(
            "real_session_created",
            session_id=self.session,
            requested_model=self.args.model,
        )

        watch = Watch(self.socket, self.session, self.args.timeout)
        try:
            deadline = time.monotonic() + self.args.timeout
            for current in watch.session_changes(deadline):
                probe = next(
                    (
                        c
                        for c in current["available_commands"]
                        if c["name"] == self.probe
                    ),
                    None,
                )
                if probe is not None:
                    self.check_probe(probe)
                    self.report(
                        "native_commands_discovered_before_prompt", command=probe
                    )
                    break
        finally:
            watch.close()

        for check_method in self.checks():
            check_method()
        self.report("all_event_sequences_contiguous", event_count=self.seq)

        self.disconnect()
        self.delete_session()
        self.session = None
        check(self.sessions() == [], "Deleted session is still listed")
        self.remove_workspace(self.workspace)
        check(self.workspaces() == [], "Removed workspace is still listed")
        self.workspace = None
        check(self.daemon.wait(timeout=15) == 0, "Daemon did not exit successfully")
        check(not self.socket.exists(), "Daemon left its socket behind")
        check(not running_after(self.agent_pids, 10), "ACP agent did not exit")
        self.report("delete_and_idle_exit", agent_exited=True, socket_removed=True)

    def check_describe(self):
        """Reads a new session's controls with the bridge's `--describe`, as
        the app does for a draft."""
        description = json.loads(
            command(
                [*self.agent, "--describe", str(self.project)],
                env=self.env,
                timeout=self.args.timeout,
            )
        )
        model = next(
            (o for o in description["configOptions"] if o.get("category") == "model"),
            None,
        )
        check(model is not None, f"Described no model option: {description}")
        probe = next(
            (c for c in description["availableCommands"] if c["name"] == self.probe),
            None,
        )
        check(probe is not None, f"Described no probe command: {description}")
        self.check_probe(probe)
        self.report(
            "describe_lists_options_and_commands",
            model=model["currentValue"],
            command=probe,
        )

    def check_reconnect(self):
        token = self.directory.name
        self.attach(0)
        self.prompt(
            f"Remember this token for my next message: {token}. Do not use tools. "
            "Write a numbered list of 100 short factual sentences about arithmetic.",
        )
        first = self.until(message)
        cursor = self.seq
        self.disconnect()
        time.sleep(6)  # Longer than oxd's default five-second idle exit.
        check(self.session_info(), "Session disappeared without a client")
        self.attach(cursor)
        rest = self.finish()
        self.report(
            "mid_turn_client_killed_and_reattached",
            detached_after_seq=cursor,
            replay_events=len(rest),
            text_characters=len(text(first + rest)),
            survived_without_clients_seconds=6,
        )
        self.prompt(
            "What exact token did I ask you to remember? Reply with only that token. "
            "Do not use tools.",
        )
        response = text(self.finish()).strip()
        check(token in response, f"Conversation token was not retained: {response}")
        self.report("followup_retains_conversation", response=response)

    def check_load_after_restart(self):
        token = self.directory.name
        title = self.session_info()["title"]
        check(title is not None, "The session has no title")
        stored = self.seq
        self.disconnect()
        self.remember_agents()
        self.daemon.send_signal(signal.SIGTERM)
        check(self.daemon.wait(timeout=15) == 0, "Daemon did not stop on SIGTERM")
        check(not running_after(self.agent_pids, 10), "ACP agent outlived the daemon")

        self.start_daemon()
        check(
            [(w["id"], w["path"]) for w in self.workspaces()]
            == [(self.workspace, str(self.project))],
            "The workspace was not saved",
        )
        info = self.session_info()
        check(info["state"] == "exited", f"Saved session is not exited: {info}")
        check(
            info["workspace_id"] == self.workspace,
            f"Saved session lost its workspace: {info}",
        )
        check(info["title"] == title, f"Saved session lost its title: {info}")
        self.report("saved_session_listed_after_daemon_restart", title=title)

        self.load_session()
        self.remember_agents()
        self.seq = 0
        self.attach(0)
        # The history comes from the daemon's database with its original seqs.
        replay = []
        deadline = time.monotonic() + self.args.timeout
        while self.seq < stored:
            event = self.attachment.read(deadline)
            check(event["seq"] == self.seq + 1, f"Sequence gap or duplicate: {event}")
            self.seq = event["seq"]
            replay.append(event)
        check(
            f"Remember this token for my next message: {token}" in user_text(replay),
            f"History lacks the first prompt: {user_text(replay)[:300]}",
        )
        check(
            token in text(replay),
            f"History lacks the agent's answer: {text(replay)[:300]}",
        )
        restarted = self.until(lambda e: kind(e) == "agent_started")
        check(
            len(restarted) == 1,
            f"Load recorded the agent's replay again: {restarted[:3]}",
        )
        self.report(
            "history_survives_daemon_restart",
            session_id=self.session,
            history_events=len(replay),
        )
        self.prompt(
            "What exact token did I ask you to remember at the start of this "
            "conversation? Reply with only that token. Do not use tools.",
        )
        response = text(self.finish()).strip()
        check(token in response, f"Loaded session lost the conversation: {response}")
        self.report("loaded_session_retains_conversation", response=response)

    def permitting(self, predicate):
        """The events up to the first that matches `predicate`, allowing each
        permission request on the way once."""
        events = []
        while True:
            events += self.until(
                lambda e: predicate(e) or kind(e) == "permission_requested"
            )
            if predicate(events[-1]):
                return events
            permission = events[-1]["kind"]["data"]
            option = next(
                o["optionId"]
                for o in permission["options"]
                if o["kind"] == "allow_once"
            )
            self.permit(permission["request_id"], option)

    def check_steer(self):
        self.prompt(
            "New task: use your shell tool to run `sleep 15; echo slept`. When it "
            "finishes, reply with only the word done.",
        )
        self.permitting(
            lambda e: (
                update(e).get("sessionUpdate") == "tool_call_update"
                and update(e).get("status") in ("pending", "in_progress")
            )
        )
        self.prompt(
            "Also include the word steered in your reply. Do not use tools.",
        )
        steered = self.permitting(lambda e: kind(e) == "steered_prompt_taken")
        check(
            any(kind(e) == "prompt_steered" for e in steered),
            "The prompt was not steered",
        )
        rest = self.permitting(lambda e: kind(e) == "turn_ended")
        check(
            rest[-1]["kind"]["data"] == turn_result("end_turn"),
            f"Unexpected turn result: {rest[-1]}",
        )
        reply = text(steered + rest)
        check("steered" in reply.lower(), f"The reply ignored the steer: {reply}")
        self.report("steered_prompt_taken_into_turn", reply=reply.strip())

    def check_queue(self):
        self.prompt(
            "New task: use your shell tool to run `sleep 5`. Then reply with only "
            "the word first.",
        )
        queued = "Reply with only the word queued. Do not use tools."
        self.queue_add(queued)
        first = self.permitting(lambda e: kind(e) == "turn_ended")
        check(
            first[-1]["kind"]["data"] == turn_result("end_turn"),
            f"Unexpected turn result: {first[-1]}",
        )
        started = self.until(lambda e: kind(e) == "prompt_started")
        prompt = blocks_text(started[-1]["kind"]["data"]["prompt"])
        check(prompt == queued, f"The next turn did not start with the queue: {prompt}")
        reply = text(self.finish()).strip()
        check("queued" in reply.lower(), f"The queued prompt was not answered: {reply}")
        check(
            self.session_info()["queue"] == [],
            "The sent prompt is still queued",
        )
        self.report("queued_prompt_starts_next_turn", reply=reply)

    def check_cancel_generation(self):
        self.prompt(
            "Do not use tools. Write a numbered list of 200 short factual sentences "
            "about geometry.",
        )
        self.until(message)
        self.cancel()
        self.finish("cancelled")
        self.report("cancel_running_generation", stop_reason="cancelled")

    def parking_run(self, delay):
        directory = self.directory / "parking"
        directory.mkdir()
        run = type(self)(
            self.args,
            directory,
            self.binaries,
            self.events,
            ["--park-after-ms", str(delay * 1000)],
        )
        run.daemon_log = self.daemon_log
        try:
            run.start_daemon()
            workspace = run.add_workspace(run.project)
            run.session = run.start_session(workspace)
            run.set_option("model", self.args.model)
            run.remember_agents()
            run.attach(0)
            return run
        except BaseException:
            run.cleanup()
            raise

    def cleanup(self):
        try:
            if self.attachment is not None:
                self.disconnect()
            if self.daemon is not None and self.daemon.poll() is None:
                self.remember_agents()
                if self.session is not None:
                    self.delete_session()
        finally:
            try:
                if self.daemon is not None:
                    self.daemon.wait(timeout=8)
            except subprocess.TimeoutExpired:
                # A failed delete must not leave a live model request behind.
                for pid in self.agent_pids:
                    try:
                        os.killpg(pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
            finally:
                if self.daemon is not None:
                    stop(self.daemon)

    def check_title_tool(self):
        """Has the agent retitle the session with the `ox` MCP server's
        `set_title` tool. A permission request would stall the turn."""
        title = f"Smoke {self.directory.name}"
        self.prompt(
            f"Call the set_title tool of the ox MCP server with the title {title} "
            "and do nothing else.",
        )
        self.finish()
        info = self.session_info()
        check(info["title"] == title, f"The title tool did not set the title: {info}")
        self.report("agent_set_title_through_ox_mcp", title=title)


class ClaudeRun(SmokeRun):
    harness = "claude"
    bridge = "claude-code-acp"
    probe = "ox-command-probe"

    def write_project(self):
        skill = self.project / ".claude" / "skills" / "ox-command-probe"
        skill.mkdir(parents=True)
        (skill / "SKILL.md").write_text(
            "---\nname: ox-command-probe\n"
            "description: Return the smoke command marker\n"
            "argument-hint: value\ndisable-model-invocation: true\n---\n"
            "Reply with exactly OX_NATIVE_COMMAND_$ARGUMENTS and nothing else. "
            "Do not use tools.\n"
        )

    def check_probe(self, command):
        check(command["input"] == {"type": "text", "hint": "value"}, command)
        check("smoke command marker" in command["description"], command)

    def checks(self):
        return (
            self.check_describe,
            self.check_reconnect,
            self.check_effort,
            self.check_commands,
            self.check_permission,
            self.check_question,
            self.check_background_wake,
            self.check_cancel_permission,
            self.check_steer,
            self.check_queue,
            self.check_cancel_generation,
            self.verify_model,
            self.check_title_tool,
            self.check_load_after_restart,
            self.check_parking,
        )

    def permission(self):
        events = self.until(lambda e: kind(e) == "permission_requested")
        return events[-1]

    def check_effort(self):
        """Sets the effort of the session's running Claude."""
        effort = self.set_option("effort", "low")
        check(effort == "low", f"Effort was not set: {effort}")
        self.report("effort_set_on_running_claude", effort=effort)

    def check_commands(self):
        """Invokes a native skill through the same content blocks the app sends."""
        self.prompt("/ox-command-probe ARGUMENTS")
        response = text(self.finish()).strip()
        check(response == "OX_NATIVE_COMMAND_ARGUMENTS", response)
        self.report("native_skill_invoked_with_arguments", response=response)
        self.prompt("/model")
        response = text(self.finish()).strip()
        check(
            "Current model:" in response,
            f"Local command did not return model: {response}",
        )
        self.report("native_local_command_completed", response=response)

    def check_permission(self):
        self.prompt(
            "Use your file write or patch tool to create approved.txt in the current "
            "working directory containing exactly permission-ok followed by a newline. "
            "Do not use shell commands. Then say done.",
        )
        requested = self.permission()
        permission = requested["kind"]["data"]
        self.disconnect()
        time.sleep(2)
        check(
            self.session_info()["state"] == "awaiting_permission",
            "Permission did not survive disconnect",
        )
        check(not (self.project / "approved.txt").exists(), "Write ran before approval")
        self.attach(self.seq - 1)
        replay = self.attachment.read(time.monotonic() + self.args.timeout)
        check(replay == requested, "Replayed permission differs from the original")
        option = next(
            o["optionId"] for o in permission["options"] if o["kind"] == "allow_once"
        )
        self.permit(permission["request_id"], option)
        events = self.finish()
        check(
            any(kind(e) == "permission_resolved" for e in events),
            "Approval was not recorded",
        )
        check(
            (self.project / "approved.txt").read_text() == "permission-ok\n",
            "Approved write did not produce the expected file",
        )
        self.report(
            "permission_survives_disconnect_and_allow_once",
            request_id=permission["request_id"],
        )

    def check_question(self):
        """Claude's AskUserQuestion call becomes a form that the app
        answers, and the answer reaches Claude."""
        self.prompt(
            "Use the AskUserQuestion tool to ask me one question with header Color, "
            "question 'Which color?', and options Red and Blue. After I answer, "
            "reply with only the color I chose.",
        )
        requested = self.until(lambda e: kind(e) == "elicitation_requested")[-1]
        elicitation = requested["kind"]["data"]
        check(
            self.session_info()["state"] == "awaiting_input",
            "The session is not awaiting input",
        )
        self.disconnect()
        self.attach(self.seq - 1)
        replay = self.attachment.read(time.monotonic() + self.args.timeout)
        check(replay == requested, "Replayed elicitation differs from the original")
        choices = [
            option["const"]
            for option in elicitation["requested_schema"]["properties"]["q1"]["oneOf"]
        ]
        check("Blue" in choices, f"The form does not offer Blue: {choices}")
        self.answer(elicitation["request_id"], {"q1": "Blue"})
        events = self.finish()
        check(
            any(kind(e) == "elicitation_resolved" for e in events),
            "The answer was not recorded",
        )
        response = text(events).strip()
        check("Blue" in response, f"Claude did not get the answer: {response}")
        self.report(
            "ask_user_question_answered_as_a_form",
            request_id=elicitation["request_id"],
            response=response,
        )

    def check_background_wake(self):
        token = f"woke-{self.directory.name}"
        self.prompt(
            "Use the Bash tool with run_in_background set to true to run "
            "`sleep 8; echo finished`. Then reply with only the word started and "
            "end your turn without waiting for the command. When you are later "
            f"told that the background command finished, reply with only {token}.",
        )
        while True:
            events = self.until(
                lambda e: kind(e) in ("permission_requested", "turn_ended")
            )
            if kind(events[-1]) == "turn_ended":
                break
            permission = events[-1]["kind"]["data"]
            option = next(
                o["optionId"]
                for o in permission["options"]
                if o["kind"] == "allow_once"
            )
            self.permit(permission["request_id"], option)
        check(
            events[-1]["kind"]["data"] == turn_result("end_turn"),
            f"Unexpected turn result: {events[-1]}",
        )

        def tool_updates(events, status):
            return [
                e["kind"]["data"]
                for e in events
                if kind(e) == "update"
                and e["kind"]["data"]["sessionUpdate"] == "tool_call_update"
                and e["kind"]["data"].get("status") == status
            ]

        # The Bash call stays in progress while its command runs.
        running = [u for u in tool_updates(events, "in_progress") if u.get("content")]
        check(running, "The background Bash tool call was not in progress")
        tool_call = running[-1]["toolCallId"]
        check(
            not any(
                u["toolCallId"] == tool_call for u in tool_updates(events, "completed")
            ),
            "The background Bash tool call completed before its command ended",
        )
        output = [
            block["content"]["text"]
            for block in running[-1]["content"]
            if block.get("content", {}).get("type") == "text"
        ]
        check(output, "The Bash tool call reported no output")
        woke = self.until(lambda e: kind(e) == "turn_ended")
        started = next(
            (i for i, e in enumerate(woke) if kind(e) == "turn_started"), None
        )
        check(
            started is not None,
            f"Claude's own turn did not start with turn_started: {woke}",
        )
        completed = [
            i
            for i, e in enumerate(woke)
            if kind(e) == "update"
            and e["kind"]["data"]["sessionUpdate"] == "tool_call_update"
            and e["kind"]["data"]["toolCallId"] == tool_call
            and e["kind"]["data"].get("status") == "completed"
        ]
        check(
            completed and completed[0] < started,
            "The Bash tool call did not complete before Claude's own turn",
        )
        check(
            woke[-1]["kind"]["data"] == turn_result("end_turn"),
            f"Unexpected end of Claude's own turn: {woke[-1]}",
        )
        check(token in text(woke), f"Claude did not answer: {text(woke)}")
        check(
            self.session_info()["state"] == "idle", "Session is not idle after the turn"
        )
        self.report(
            "background_task_wakes_claude",
            tool_output=output[-1].strip(),
            response=text(woke).strip(),
        )

    def check_cancel_permission(self):
        self.prompt(
            "Use your file write or patch tool to create cancelled.txt containing "
            "cancelled. Do not use shell commands.",
        )
        self.permission()
        self.cancel()
        events = self.finish("cancelled")
        check(
            any(
                kind(e) == "permission_resolved"
                and e["kind"]["data"]["outcome"]["outcome"] == "cancelled"
                for e in events
            ),
            "Pending permission was not cancelled",
        )
        check(
            not (self.project / "cancelled.txt").exists(),
            "Cancelled write created a file",
        )
        self.report("cancel_pending_permission", file_created=False)

    def verify_model(self):
        claude_dir = Path(
            self.env.get("CLAUDE_CONFIG_DIR", str(Path.home() / ".claude"))
        )
        transcript = next(
            (claude_dir / "projects").glob(
                f"*/{self.session_info()['agent_session_id']}.jsonl"
            )
        )
        models = set()
        for line in transcript.read_text().splitlines():
            model = json.loads(line).get("message", {}).get("model")
            if model and model != "<synthetic>":
                models.add(model)
        check(
            models == {self.args.model},
            f"Unexpected transcript models: {sorted(models)}",
        )
        self.report("model_verified", models=sorted(models))

    def check_parking(self):
        """A session with a background command is not parked while it runs,
        and is parked once Claude has handled it and the session has been idle
        and unviewed for the delay, in a daemon of its own with a short
        delay."""
        delay = 10
        run = self.parking_run(delay)
        watch = None
        try:
            token = f"parked-{self.directory.name}"
            run.prompt(
                "Use the Bash tool with run_in_background set to true to run "
                f"`sleep {delay * 2 + 5}; echo finished`. Then reply with only the "
                "word started and end your turn without waiting for the command. "
                "When you are later told that the background command finished, "
                f"reply with only {token}.",
            )
            while True:
                events = run.until(
                    lambda e: kind(e) in ("permission_requested", "turn_ended")
                )
                if kind(events[-1]) == "turn_ended":
                    break
                permission = events[-1]["kind"]["data"]
                option = next(
                    o["optionId"]
                    for o in permission["options"]
                    if o["kind"] == "allow_once"
                )
                run.permit(permission["request_id"], option)
            # Following the session through the watch leaves it unviewed.
            run.disconnect()
            watch = Watch(run.socket, run.session, self.args.timeout)
            before_wake = watch.until("running")
            check(
                "exited" not in before_wake,
                f"The session was parked while its command ran: {before_wake}",
            )
            watch.until("exited")
            run.attach(0)
            seen = []
            deadline = time.monotonic() + self.args.timeout
            while True:
                event = run.attachment.read(deadline)
                seen.append(kind(event))
                if kind(event) == "agent_parked":
                    break
            check(
                "turn_started" in seen,
                f"Claude's own turn is not in the history: {seen}",
            )
            check(
                seen.index("turn_started") < seen.index("agent_parked"),
                f"The session was parked before Claude's own turn: {seen}",
            )
            self.report(
                "idle_session_with_background_work_parks_after_it",
                park_after_seconds=delay,
            )
        finally:
            if watch is not None:
                watch.close()
            run.cleanup()


class PiRun(SmokeRun):
    harness = "pi"
    bridge = "pi-acp"
    probe = "skill:ox-command-probe"

    def write_project(self):
        skill = self.project / ".pi" / "skills" / "ox-command-probe"
        skill.mkdir(parents=True)
        (skill / "SKILL.md").write_text(
            "---\nname: ox-command-probe\n"
            "description: Return the smoke command marker\n"
            "disable-model-invocation: true\n---\n"
            "The entire text after the closing </skill> tag, ignoring surrounding "
            "whitespace, is the argument value. Reply with OX_NATIVE_COMMAND_ "
            "immediately followed by that value and nothing else. Do not use tools.\n"
        )
        extensions = self.project / ".pi" / "extensions"
        extensions.mkdir()
        (extensions / "ox-color.ts").write_text(
            "export default function (pi) {\n"
            '  pi.on("tool_call", async (event, ctx) => {\n'
            '    if (event.toolName !== "bash" || !event.input.command.includes("ox-color")) return;\n'
            '    const color = await ctx.ui.select("Which color?", ["Red", "Blue"]);\n'
            "    event.input.command = `echo ox-color-${color}`;\n"
            "  });\n"
            "}\n"
        )

    def check_probe(self, command):
        check("smoke command marker" in command["description"], command)

    def checks(self):
        return (
            self.check_describe,
            self.check_reconnect,
            self.check_thinking,
            self.check_commands,
            self.check_dialog,
            self.check_steer,
            self.check_queue,
            self.check_cancel_generation,
            self.verify_model,
            self.check_title_tool,
            self.check_load_after_restart,
            self.check_parking,
        )

    def check_thinking(self):
        thinking = self.set_option("thinking", "low")
        check(thinking == "low", f"Thinking was not set: {thinking}")
        self.report("thinking_set_on_running_pi", thinking=thinking)

    def check_commands(self):
        self.prompt("/skill:ox-command-probe copper-lantern")
        response = text(self.finish()).strip()
        check(response == "OX_NATIVE_COMMAND_copper-lantern", response)
        self.report("native_skill_invoked_with_arguments", response=response)

    def check_dialog(self):
        self.prompt(
            "Run exactly `echo ox-color` with the bash tool, then reply with only "
            "the command's output.",
        )
        requested = self.until(lambda e: kind(e) == "elicitation_requested")[-1]
        elicitation = requested["kind"]["data"]
        check(
            self.session_info()["state"] == "awaiting_input",
            "The session is not awaiting input",
        )
        self.disconnect()
        self.attach(self.seq - 1)
        replay = self.attachment.read(time.monotonic() + self.args.timeout)
        check(replay == requested, "Replayed elicitation differs from the original")
        choices = [
            option["const"]
            for option in elicitation["requested_schema"]["properties"]["answer"][
                "oneOf"
            ]
        ]
        check("Blue" in choices, f"The form does not offer Blue: {choices}")
        self.answer(elicitation["request_id"], {"answer": "Blue"})
        events = self.finish()
        check(
            any(kind(e) == "elicitation_resolved" for e in events),
            "The answer was not recorded",
        )
        response = text(events).strip()
        check("ox-color-Blue" in response, f"Pi did not get the answer: {response}")
        self.report(
            "extension_dialog_answered_as_a_form",
            request_id=elicitation["request_id"],
            response=response,
        )

    def verify_model(self):
        agent_dir = Path(
            self.env.get("PI_CODING_AGENT_DIR", str(Path.home() / ".pi" / "agent"))
        )
        transcript = next(
            (agent_dir / "sessions").glob(
                f"*/*_{self.session_info()['agent_session_id']}.jsonl"
            )
        )
        models = set()
        for line in transcript.read_text().splitlines():
            entry = json.loads(line)
            message = entry.get("message", {})
            if entry.get("type") == "message" and message.get("role") == "assistant":
                models.add(f"{message['provider']}/{message['model']}")
        check(
            models == {self.args.model},
            f"Unexpected transcript models: {sorted(models)}",
        )
        self.report("model_verified", models=sorted(models))

    def check_parking(self):
        delay = 10
        run = self.parking_run(delay)
        watch = None
        try:
            run.prompt("Reply with only ready. Do not use tools.")
            run.finish()
            run.disconnect()
            watch = Watch(run.socket, run.session, self.args.timeout)
            watch.until("exited")
            run.attach(0)
            deadline = time.monotonic() + self.args.timeout
            while kind(run.attachment.read(deadline)) != "agent_parked":
                pass
            self.report("idle_session_parks", park_after_seconds=delay)
        finally:
            if watch is not None:
                watch.close()
            run.cleanup()


AGENTS = {"claude": ClaudeRun, "pi": PiRun}


def positive_seconds(value):
    seconds = float(value)
    if not 0 < seconds < float("inf"):
        raise argparse.ArgumentTypeError("must be a positive, finite number")
    return seconds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", choices=AGENTS, default="claude")
    parser.add_argument("--model", required=True, help="Model ID; no fallback")
    parser.add_argument(
        "--timeout",
        type=positive_seconds,
        default=120,
        help="Seconds per command or event phase (default: 120)",
    )
    args = parser.parse_args()
    agent = AGENTS[args.agent]
    check(
        shutil.which(agent.harness),
        f"{agent.harness} is not installed or is not on PATH",
    )
    packages = ["oxd", "ox-mcp", agent.bridge]
    subprocess.run(
        ["cargo", "build", *[flag for p in packages for flag in ("-p", p)]],
        cwd=REPO,
        check=True,
    )
    metadata = json.loads(
        command(["cargo", "metadata", "--no-deps", "--format-version", "1"])
    )
    binaries = Path(metadata["target_directory"]) / "debug"
    directory = Path(tempfile.mkdtemp(prefix="ox-acp-", dir="/tmp")).resolve()
    print(f"Artifacts: {directory}", flush=True)
    with (
        (directory / "events.jsonl").open("w") as events,
        (directory / "oxd.log").open("w") as daemon_log,
    ):
        run = agent(args, directory, binaries, events)
        try:
            try:
                run.run(daemon_log)
            finally:
                run.cleanup()
        except (Exception, KeyboardInterrupt) as error:  # noqa: BLE001 - every failure is reported
            run.report("failed", error=str(error) or "Interrupted")
            traceback.print_exc()
            return 1
        run.report("passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
