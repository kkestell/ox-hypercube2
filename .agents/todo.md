# To do

Items are grouped by kind, then by area, most severe first. Each opens with a
bold one-line summary, then its severity, details, and the code or spec it
concerns. The spec describes current behavior; update the affected spec
alongside any change in behavior. A `TODO` comment in the code marks several
of these; remove it with the fix.

## Structure

Each of these underlies several bugs below, which the fix closes.

- [x] **A session's state has no single owner.** High. It lives in `Phase`
  inside `State`, ten separately locked fields on `Session` (`worktree`,
  `archived`, `config`, `unseen`, `parker` and others), the `Registry`
  (`sessions`, `worktrees`, `moving_checkouts`), SQLite rows, and history
  that may not be loaded. Each operation updates a subset in an order chosen
  by hand, and lock order is stated only in comments. The store records no
  lifecycle status, so startup infers one from `agent_session_id` and
  `worktree_id`. One owner per session would compute each transition's next
  state, rows and events together, then persist before publishing. Underlies
  restart losing never-started sessions, the dangling worktree ID, and
  workspace removal racing unarchive or delete.
  `crates/oxd/src/session.rs:70`, `crates/oxd/src/daemon.rs:28`,
  `crates/oxd/src/daemon.rs:139`.
- [x] **Callers enforce session preconditions instead of the transitions.**
  Medium. Each handler checks `refuse_moving` and takes the lifecycle lock
  itself (`send_unmoved`, `SessionLoad`, `send_message`, `new_session`).
  `start_queued` starts a turn from inside `Session::update`, where neither is
  visible, and `remove_workspace` skips the lifecycle lock. A session that
  knows its checkout is moving can refuse a turn in the transition itself,
  whatever path requests it. Underlies the queued prompt starting during a
  branch move and the workspace removal race. `crates/oxd/src/daemon.rs:280`,
  `crates/oxd/src/state.rs:813`, `crates/oxd/src/daemon.rs:612`.
- [ ] **A restored session is a second mode of `State`.** Medium. Its events
  are `None` until first read, and crash recovery runs in `fill` at that read
  rather than at startup. Events are persisted in batches after their seq is
  assigned in memory, and replay treats `after_seq` as a vector offset. A
  durable log that owns seqs and recovery would give `State` one mode; the
  memory cost of holding every session bears on it (see Scale). Underlies the
  crash error shown only once history is read, the subscriber hung by a
  delete during the first read, and replay breaking after a failed write.
  `crates/oxd/src/state.rs:715`, `crates/oxd/src/store.rs`.
- [ ] **The bridges infer which turn a result or dialog belongs to.** Medium.
  Claude's stream-json and Pi's RPC tag neither with a prompt, so the bridges
  track flags (`preempted`, `cancelling`, `initializing`, `unclaimed_error`)
  and match by position. A pure turn state machine in each bridge, like
  `Phase::after` in `oxd`, could be tested over interleavings of prompts,
  steering, cancellation, dialogs and model changes. Underlies cancelling
  during a preempted turn, cancelling before a Pi dialog arrives, and a
  prompt running on the previous model.
  `crates/claude-code-acp/src/runner.rs:646`, `crates/pi-acp/src/pi.rs:383`,
  `crates/pi-acp/src/bridge.rs:273`.
- [ ] **Clients sequence session lifecycle themselves.** Medium. Selecting an
  exited session awaits `load_session` before switching, and `set_config`
  replaces the whole config, so the composer tracks the base each change
  builds on. Daemon operations that state intent and are idempotent, such as
  focus loading the session or applying one setting, would leave the UI with
  latest-wins requests and no sequencing. Underlies a slow session switch
  undoing a later one and concurrent config edits dropping a setting.
  `ui/src/features/sessions/SessionList.tsx:188`,
  `ui/src/features/composer/Composer.tsx:277`.

## Bugs

### Daemon

- [x] **Restart loses never-started sessions.** High. Startup deletes every
  session with no agent session and no worktree, archived ones included, so
  archiving a session whose agent failed to start reports success and the
  session is gone after a restart. A never-started worktree session survives
  but loses its first prompt, because it is restored with no events.
  `crates/oxd/src/daemon.rs:139`, `crates/oxd/src/state.rs:691`.
- [x] **A queued prompt can start while the session's branch is moving.**
  Medium. Queue draining starts the next prompt inside `Session::update`
  without the daemon's move guard, though `session.prompt` refuses the same
  prompt. `crates/oxd/src/state.rs:811`, `crates/oxd/src/session.rs:816`.
- [ ] **Deleting a session during its first history read can hang a
  subscriber.** Medium; dropped once during review. Closing a session whose
  history is not loaded records no `session_closed`; the load then fills the
  closed state and the subscription is accepted, so it never ends.
  `crates/oxd/src/conn.rs:276`, `crates/oxd/src/state.rs:1150`.
- [ ] **A failed event write breaks replay after restart.** Low; deferred
  until a storage failure affects normal use or a supported caller depends on
  a nonzero replay cursor. The writer drops the failed batch, and replay uses
  `after_seq` as a vector offset, so a subscription from a nonzero cursor
  stalls and one from zero can show incomplete history.
  `crates/oxd/src/store.rs`, `crates/oxd/src/state.rs`. To reproduce:

  1. Run `cargo build -p oxd -p oxctl` from the repository root, then set up
     a fresh scratch repository and socket:

     ```sh
     bin="$PWD/target/debug"
     export PATH="$bin:$PATH"
     scratch=$(mktemp -d /tmp/ox-repro.XXXXXX)
     export OX_SOCKET="$scratch/oxd.sock"
     repo="$scratch/repo"
     git init -q -b main "$repo"
     git -C "$repo" config user.name "Ox Reproduction"
     git -C "$repo" config user.email "ox-repro@example.invalid"
     git -C "$repo" commit --allow-empty -qm base
     ```

  2. Start the daemon with `"$bin/oxd" --socket "$OX_SOCKET"`, note its PID,
     and wait for it to print `ready`. Then run
     `workspace=$(oxctl workspace add "$repo")`, start a session with
     `oxctl new --workspace "$workspace" -- "$bin/oxd-fake-agent"`, and save
     its ID as `session`.
  3. Install this trigger in `"$OX_SOCKET.db"`:

     ```sql
     CREATE TRIGGER injected BEFORE INSERT ON events
     WHEN json_extract(NEW.kind, '$.type') = 'prompt_started'
     BEGIN
       SELECT RAISE(ABORT, 'injected event write failure');
     END;
     ```

  4. Follow events with `oxctl attach "$session"` and send
     `oxctl prompt "$session" 'say 1 0'`. After `turn_ended` arrives, send the
     daemon SIGTERM and wait for it to exit; its stderr logs the failed write.
     Inspect the database with
     `SELECT seq, json_extract(kind, '$.type') FROM events ORDER BY seq;`.
     The expected sequence is `1, 3, 4, 5, 6, 7`, ending in `turn_ended`.
     If the prompt and turn end landed in the same failed batch, repeat with a
     fresh session and `oxctl prompt "$session" wait`, wait for the write
     error in stderr, then run `oxctl cancel "$session"` to record a later
     event in a separate batch.
  5. Restart `oxd` on the same socket and run
     `oxctl attach "$session" --after 6`, using one less than the final
     stored sequence if it differs. The subscription waits even though event 7
     is already in the database.

- [ ] **A crash-interrupted turn shows its error only once history is read.**
  Low. The error and its unseen outcome are added when the session's history
  is first loaded, not when `oxd` restarts. `crates/oxd/src/state.rs:768`.
- [ ] **A new session whose agent exits before its first prompt resumes
  without its MCP servers and Ox instructions.** Low.
  `crates/oxd/src/session.rs:1008`.
- [x] **Removing a workspace can race an archived session's unarchive or
  delete.** Low. Workspace removal deletes rows without the sessions'
  lifecycle guards. `crates/oxd/src/daemon.rs:630`.
- [x] **A failed worktree start briefly publishes a dangling worktree ID.**
  Low. The worktree is removed and announced before `fail_start` clears the
  session's reference. `crates/oxd/src/daemon.rs:859`.
- [ ] **Compact transcripts omit replayed user messages.** Low.
  `read_transcript` renders prompts and assistant updates but not user
  message updates from a replay, such as an imported session's.
  `crates/oxd/src/transcript.rs:42`.
- [ ] **A worktree that lost its `.git` file can be impossible to delete.**
  Low. When the worktrees directory sits inside another repository, the
  usability check discovers that repository and `git worktree remove` then
  fails, even with discard. Comparing `--git-common-dir` with the recorded
  common directory would close it. `crates/oxd/src/worktree.rs:653`.

### File watching

- [ ] **On macOS, opening or closing a folder can make another folder miss a
  change.** Low. The FSEvents stream restarts from "now", so a folder stays
  stale until it changes again. `crates/oxd/src/directories.rs:43`.
- [ ] **Unwatching a folder can block an async worker.** Low. `Changes::drop`
  calls the synchronous watcher's `unwatch` directly.
  `crates/oxd/src/directories.rs:179`.
- [ ] **Failing to watch every ancestor panics.** Low. The ancestor loop ends
  in `unreachable!`, though watching can fail for each ancestor.
  `crates/oxd/src/directories.rs:143`.
- [ ] **On Linux, a folder deleted and recreated between two reads keeps a
  stale inotify watch.** Low; matters only once the app runs on Linux.
  `crates/oxd/src/directories.rs:39`.

### Claude Code bridge

- [ ] **Cancelling during a preempted turn can leave it open.** Medium. The
  first result after a preemption is treated as the continuation, which
  neither ends the turn nor resolves the cancellation.
  `crates/claude-code-acp/src/runner.rs:646`.
- [ ] **Resume misses schedules a subagent created.** Medium; accepted as a
  limitation at the time. Resume seeds background work only from the main
  transcript, not subagent transcripts, so a restored recurring job can leave
  the session reported idle. `crates/claude-code-acp/src/bridge.rs:438`.
- [ ] **Decide what an empty MCP server list means on resume.** The bridge
  replaces the previous servers when given a nonempty list but keeps them
  when given an empty one, so an empty list cannot clear them. Check the ACP
  contract. See [Claude Code bridge](../spec/claude.md#mcp-servers).
- [ ] **Import leaves out transcripts with no timestamp in their head.** Low.
  `crates/claude-code-acp/src/sessions.rs:18`.

### Pi bridge

- [ ] **Cancelling before an extension dialog arrives leaves the prompt
  pending.** Medium. Cancellation withdraws only dialogs already open; a later
  dialog for the cancelled prompt opens normally. `crates/pi-acp/src/pi.rs:383`.
- [ ] **A prompt sent during a model change can run on the previous model.**
  Low. Pi handles RPC lines concurrently, so a following prompt can start
  while `set_model` awaits authentication. `crates/pi-acp/src/bridge.rs:273`.
- [ ] **A session keeps a title from a first prompt Pi rejected.** Low. The
  title is published before Pi answers the prompt.
  `crates/pi-acp/src/bridge.rs:166`.
- [ ] **Tool locations ignore Pi's path expansion.** Low. Paths are joined to
  the working directory without expanding `~` or stripping Pi's `@` prefix.
  `crates/pi-acp/src/tools.rs:73`.
- [ ] **Generated patch headers do not quote unusual paths.** Low.
  `crates/pi-acp/src/tools.rs:135`.

### Desktop

- [ ] **A slow session switch can undo a later one.** Medium. Selecting an
  exited session awaits its load before switching, so selecting another
  session meanwhile is overridden when the load finishes.
  `ui/src/features/sessions/SessionList.tsx:188`.
- [ ] **The searchable model picker can highlight an option out of view.**
  Medium. Arrow keys move the active option without scrolling it into view,
  so Enter can choose an option the user cannot see.
  `ui/src/features/composer/ModelPicker.tsx:53`.
- [ ] **Highlighting a large file stalls the UI.** Medium. Highlighting and
  rendering run synchronously. `ui/src/features/files/highlight.ts:12`.
- [ ] **Decide whether withdrawn permission requests should be removed.**
  An agent can withdraw a permission request, but Ox leaves it pending until
  it is answered, the turn is cancelled, or the agent or session stops.
  Withdrawn forms are already removed. Decide whether a withdrawn permission
  request should likewise be removed, including when an answer arrives at the
  same time. See
  [Permissions and questions](../spec/requests.md#answer-a-permission-request).
- [ ] **Concurrent config edits can drop a pending setting.** Low. A listing
  that changes something else while a change is pending resets the base the
  next change builds on. `ui/src/features/composer/Composer.tsx:277`.
- [ ] **Simultaneous delete outcomes overwrite each other's dialogs.** Low.
  Session actions hold one unsaved-work outcome and one kept-branch outcome
  for all sessions. `ui/src/features/sessions/useSessionActions.tsx:18`.
- [ ] **An optional single-choice answer cannot be cleared.** Low. Radios
  offer no empty choice. `ui/src/features/composer/Elicitation.tsx:117`.
- [ ] **A steered peer message's sender is not dimmed.** Low. Only
  `.prompt.steered` is dimmed, not the sender label beside it.
  `ui/src/styles/conversation.css:81`.
- [ ] **Markdown link and footnote definitions do not resolve across
  blocks.** Low. `ui/src/features/conversation/blocks.ts:12`.
- [ ] **Align form validation across clients.** Low. The desktop enforces
  text patterns and formats; command-line answers do not. Shared checks
  already cover field names, required fields, value types, choices, lengths,
  ranges, and choice counts. See
  [Permissions and questions](../spec/requests.md#rules).

## Tests and tooling

- [ ] **The live smoke clients send an obsolete handshake.** Medium.
  `smoke-acp.py` sends `protocol_version=1`; the daemon requires the string
  build hash, so every live smoke run fails at `hello`.
  `scripts/smoke-acp.py:144`, `crates/ox-protocol/src/messages.rs:15`.
- [ ] **The Pi smoke test assumes the default sessions directory.** Low. It
  ignores `PI_CODING_AGENT_SESSION_DIR`. `scripts/smoke-acp.py:1173`.
- [ ] **A blocked FIFO in the Git test fixture can hang past its timeout.**
  Low. The timeout does not cancel the blocking open, and the runtime waits
  for it. `crates/oxd/tests/common/mod.rs:503`.

## Docs and cleanup

- [ ] **The spec still describes a collapsed Archived group.** Archived
  sessions are shown with Show Archived in the workspace menu.
  [Workspaces](../spec/workspaces.md), `ui/src/features/sessions/SessionList.tsx:214`.
- [ ] **The performance guide omits worktree identity in the UI.**
  `docs/dev/performance.md:26`.
- [x] **The session lifecycle comment says the session adds its worktree.**
  The daemon does. `crates/oxd/src/session.rs:87`.
- [ ] **Decide what the bottom panel is for, or remove it.** It has a toggle
  and a sash but no content. `ui/src/components/Workbench.tsx:173`.
- [ ] **Remove the fake agent's unused `--model-providers` flag.**
  `crates/oxd/src/bin/oxd-fake-agent.rs:113`.
- [ ] **Remove the unused `--radius-sm` token.** `ui/src/styles/tokens.css:63`.

## Features

- [ ] **Show the checkout's Git status in the desktop.** Show the current
  branch, staged and unstaged changes, and whether the session runs in a
  worktree. Nothing in the desktop shows this today, for a session's worktree
  or for the workspace's checkout.
- [ ] **Start a session in an existing worktree.** `start_session` runs the
  new session in the workspace's checkout or in a new worktree. Let it also
  take an existing worktree, such as the requesting session's own, so a
  session can hand off work in the checkout it is using.
  `crates/ox-mcp/src/main.rs`.
- [ ] **Decide whether to embed image and PDF attachments.** Desktop
  attachments send local file references, including for images and PDFs.
  Decide whether supported images and PDFs should be embedded instead, and
  what happens for agents that do not support that content. See
  [Conversations](../spec/conversations.md#compose-and-send).
- [ ] **Pull request and review support.**

## Scale

Imported Claude Code sessions could put a very large number of archived
sessions in one workspace. Measure where each of these breaks before
designing a change.

- [ ] **Measure the cost of holding every session in memory.** The daemon
  loads every saved session at startup, archived ones included.
  `crates/oxd/src/store.rs`.
- [ ] **Measure the cost of sending every session to every client.** Clients
  receive archived sessions like any other, flagged `archived`, so each
  client update and the archive list grow with them. Archived sessions would
  need to be paged rather than sent in full.
  `crates/ox-protocol/src/messages.rs`.
