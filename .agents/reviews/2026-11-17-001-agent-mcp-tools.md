# Agent MCP tools review

## Scope and coverage

The staged change for `.agents/plans/2026-11-17-001-agent-mcp-tools.md`: the four new `ox` tools in `ox-mcp`, the daemon methods `session.send_message`, `session.transcript` and `session.request`, peer message state and events, the compact transcript, worktrees on an existing branch, the bridge contract change that makes Pi answer `session/new` with its options, the desktop core's `from` and `description` items, and the UI. Checked against the brief `.agents/briefs/2026-11-16-006-agent-mcp-tools.md` and the plan's decisions. The change delivers the brief's outcome and keeps its decisions.

Gaps:

- No live agent runs were repeated. The work log's `make smoke-claude` and `make smoke-pi` runs each failed on a model answer after the new checks passed.
- The sender line and request description were not viewed on screen.
- How long each harness's MCP client waits for a `start_session` call while the user has not answered was not checked against real agents.

## Summary

| ID | Severity | Category      | Fix       | Title                                                        |
| -- | -------- | ------------- | --------- | ------------------------------------------------------------ |
| F1 | Low      | Correctness   | Not fixed | A steered peer message's sender line is not dimmed           |
| F2 | Low      | Correctness   | Not fixed | Transcript leaves out user messages recovered by replay      |
| F3 | Low      | Documentation | Not fixed | `session.new` doc comment line runs past the wrap width      |

## F1. A steered peer message's sender line is not dimmed

Low · Correctness · Not fixed · `ui/src/styles/conversation.css:86`

A steered prompt's bubble is dimmed with `.prompt.steered { opacity }` until the agent takes it in. For a steered peer message, the `.sender` line sits outside the bubble in `.peer-message`, so it stays at full strength while the bubble below it is dimmed.

### Fix

Not fixed.

## F2. Transcript leaves out user messages recovered by replay

Low · Correctness · Not fixed · `crates/oxd/src/transcript.rs:40`

`transcript::entries` writes `User:` lines only for `PromptStarted` and `PromptSteered`. A session with no recorded `agent_started` recovers its conversation by replay on load, which records the user's messages as `user_message_chunk` updates. Its transcript then has the assistant's replies and no user lines. Sessions saved this way are uncommon; every session the daemon starts records `agent_started`.

### Fix

Not fixed.

## F3. `session.new` doc comment line runs past the wrap width

Low · Documentation · Not fixed · `crates/ox-protocol/src/messages.rs:57`

The rewrapped `Method::SessionNew` doc comment leaves one line at about 95 characters, where the rest of the comment wraps at 80.

### Fix

Not fixed.

## Checks run

- `make check-rust`: passed.
- `make check-ui`: passed (82 tests).
- Traced peer message delivery (`Daemon::send_message`, `Session::started`, `State::send_now` with `resume_queue`), loading an exited recipient against `session.load`, daemon-made permission requests through cancel and exit, `worktree::plan` and `roll_back` with an existing branch, the Pi `session/new` path, and `run.rs` step configuration after the `'reported` loop's removal.

## Verdict

The change meets the brief and the plan. No High or Medium findings; F1 to F3 are Low and left unchanged.
