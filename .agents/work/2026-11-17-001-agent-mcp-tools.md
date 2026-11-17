# Agent MCP tools

## Plan

`.agents/plans/2026-11-17-001-agent-mcp-tools.md`

## Summary

Standalone sessions' agents now have four new `ox` tools besides `set_title`. `list_sessions` reads the first batch of a `watch`. `send_message`, `read_transcript` and `start_session` each call one new daemon method: `session.send_message`, `session.transcript` and `session.request`. A peer message is recorded with its sender (`from`) and starts a turn or steers the running one, leaving a held queue held. A parked recipient, or one restored after a restart, is loaded first with the sender's environment. `start_session` checks the request and then asks the user through a permission request on the requester. A worktree can be added on an existing local branch. Pi's `session/new` now starts Pi and answers with its options and commands, so `Opening.options` applies to Pi, and the `'reported` wait in `run.rs` is gone. The desktop shows the sender above a peer message and a permission request's description under its title. The plan's goal is met.

## Departures from the plan

- **Item fields the UI reads:** `from` on prompt and steered items and `description` on permission items are left out of the JSON when unset (`skip_serializing_if`), and they are optional in `protocol.ts`. Existing fixtures that build these items stay valid this way.
- **`read_transcript` description:** Hypercube's text says "child reports arrive automatically", which is not true in Ox. That clause is dropped, and "a report" becomes "a message".
- **Mock scenario:** the `sidebar` scenario gains the session "Hand off the store review". It has a peer message and a session request, per the UI-change rule in `docs/dev/development.md`.

## Decisions

- `describe(bridge, cwd, env)` runs the bridge with `env` over the caller's environment, without clearing it first. The desktop passes its own UTF-8 environment, so the result is the same apart from non-UTF-8 variables.
- `send_message` refuses an empty message. A never-started recipient is refused with `Session::load`'s existing error, "the session never started, so it has nothing to load".
- A request's description has a `Branch:` line: the named branch, `New branch from <head>` for a new worktree, or the checkout's branch when Git reports one.
- `list_sessions` lists every workspace, writing `No sessions.` under one with none to message. It gets each checkout's branch with `workspace.branches`, using `ox-mcp`'s own environment. Model and effort show the option's current value ID.
- The fake agent's `--describe <dir>` prints its config options and commands, so `Options::bridges` daemons can check model and effort.

## Checks run

- `make check-rust`: passed.
- `make check-ui`: passed (82 tests). `npm ci` ran first, because this worktree had no `ui/node_modules`.
- `make e2e`: passed (2 spec files).
- `make smoke-claude`: failed in `check_workflow`. Step 2 answered "Haiku" instead of repeating the token, the same model error the queued-prompts work log records. Every check before it passed, including steering, queueing, `agent_set_title_through_ox_mcp`, loading after a restart, and parking.
- `make smoke-pi`: failed in `check_dialog`. The model replied "ox-color" instead of "ox-color-Blue", a failure the queued-prompts work log also saw. Every check before it passed, including `real_session_created` with the requested model through the new `session/new`, `describe_lists_options_and_commands`, and reattaching mid-turn. The checks after `check_dialog` did not run.

## Manual verification

None ran. The sender line and the request description were not checked on screen against their peers. To see them in mock mode:

```sh
cd ui && npm run mock   # open ?scenario=sidebar and view "Hand off the store review"
```

## Follow-up work

- An agent's MCP client may time out a `start_session` call while the user has not answered yet. That depends on each harness's MCP tool timeout and was not tested against real agents.
