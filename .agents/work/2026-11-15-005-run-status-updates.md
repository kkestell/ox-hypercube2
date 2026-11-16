# Step start times and agent status updates

## Plan

`.agents/plans/2026-11-15-005-run-status-updates.md`

## Summary

Steps record `started_at` when they leave `pending`. Every session's `ox` MCP server offers `post_status_update`, which records an `EventKind::StatusUpdate` in the session's history through `session.post_status_update`. The desktop core folds it into an `Item::StatusUpdate`, and the run view reads each step's updates from its child's conversation and its start time from `started_at`. `statusUpdates.mock.ts` is deleted. The plan's goal is met.

## Departures from the plan

- The e2e test's reporting step uses the `pi` harness. In the e2e app `pi-acp` is the fake agent, which advertises `session.mcp.stdio` and so gets the `ox` server; `claude-code-acp` there is the real bridge over a stub `claude`, which cannot run a script. The daemon test and `runs.rs` test cover a `claude` step.
- The e2e test's second step is a shell step. The fake agent runs the first step's output as part of the next agent step's script, and `Status update posted.` is not a directive.
- `runs.rs` posts the update from a one-step workflow for the same reason.
- `target/dev/oxd.sock.db` (and its `-shm`/`-wal`) were deleted while a detached dev `oxd` (pid 60487, started 12:36) still held them open. That daemon runs the old binary and keeps writing to the unlinked files; the next dev daemon creates a new database.

## Decisions

- The fake agent's `mcp-title` and `mcp-status` parse into one `Directive::McpTool { name, arguments }`, which `call_tool` runs.
- `Run::set_step` keeps its state argument; `advance` now calls it only with `Succeeded`.
- The `now()` doc comment in `daemon.rs` sat above `resolved`'s; the two comments are back with their functions.

## Checks run

- `make check` — passed.
- `make e2e` — 12 of 13 spec files passed. `run.e2e.ts` failed 5 of 6 tests; the new status update test passed. The five failures predate this change: four look for the old run view's step sections (`Can't call $ on element with selector ".conversation section.step[aria-label="First"]" because element wasn't found`), and "starts from the New Task menu and follows its steps" expects `["session", "run", "child", "child", "session"]` in the sidebar but receives a third `"child"`, since pending steps are now listed. Both come from the uncommitted run view and sidebar work this plan builds on.

## Manual verification

1. The status update test alone.

   ```sh
   cd ui && npx wdio run e2e/wdio.conf.ts --spec e2e/specs/run.e2e.ts --mochaOpts.grep "status updates"
   ```

   1 passing.

## Follow-up work

- Rewrite the five failing `run.e2e.ts` tests for the new run view and sidebar.
- The run view's list was not compared on screen with the user's design; this change altered only its data sources.
