# Review of step start times and agent status updates

## Scope and coverage

The staged changes for `.agents/plans/2026-11-15-005-run-status-updates.md`: `started_at` on `StepInfo` and in `run_steps`, `Store::start_step` and `Run::start_step`, `EventKind::StatusUpdate`, `session.post_status_update` through `Client`, the daemon, `Session` and `State::request`, the `ox-mcp` tool router and `post_status_update`, the fake agent's `mcp-status`, `Item::StatusUpdate` in the desktop core, the run view's `RunStatus` reading updates and start times from `oxd`, the Rust tests, `run.e2e.ts`, and the workflow, architecture, ACP and glossary docs. The staged tree also holds the user's earlier run view and sidebar design (`RunHeader.tsx`, `RunStatus.tsx`, `RunView.tsx`, `SessionList.tsx`, the styles); it was reviewed for behaviour and against the tests and docs, not redesigned. No brief is linked from the plan. The change keeps each of the plan's decisions, including the approved changes in Check your assumptions: every session's `ox` server offers the tool, and only Claude steps can post. The working tree matched the staged tree when the review started.

Gaps:

- The live smoke suites were not run. The bridges and the ACP exchange are unchanged.
- The run view was not compared on screen with the user's design.

## Summary

| ID | Severity | Category      | Fix   | Title                                                         |
| -- | -------- | ------------- | ----- | ------------------------------------------------------------- |
| F1 | Medium   | Testing       | Fixed | Five run e2e tests target the removed run view and sidebar    |
| F2 | Low      | Documentation | Fixed | The workflow guide describes the old sidebar and run view     |
| F3 | Low      | Documentation | Fixed | A sentence added to the architecture doc is unwrapped         |

## F1. Five run e2e tests target the removed run view and sidebar

Medium · Testing · Fixed · `ui/e2e/specs/run.e2e.ts`

The run view no longer has a `section.step` per started step with its commands, prompts and replies, and the sidebar now lists every step beneath the run, including shell and pending steps. Five of the six tests in `run.e2e.ts` still looked for `.conversation section.step[...]` or expected two child rows, so `make e2e` fails and the run view's behaviour after a permission answer, a repair, a cancel and a failed precondition went untested.

### Evidence

`npx wdio run e2e/wdio.conf.ts --spec e2e/specs/run.e2e.ts` on the staged tree: 1 passing, 5 failing. "starts from the New Task menu and follows its steps" failed on `expect(received).toEqual(expected)` with one extra received `"child"` (the `Build` shell step's row); the other four failed with `Can't call $ on element with selector ".conversation section.step[aria-label="First"]" because element wasn't found` (and the same for `Fix`, `Hold` and `Guarded`).

### Fix

Rewrote the five tests for the new run view and sidebar, keeping each test's subject:

- "follows its steps" expects a child row for each of `First`, `Build` and `Second`, each succeeded, and in the run view the three step titles in order, each succeeded with an `HH:MM` start time.
- "takes an answer" checks each step's status in the run view turns `Succeeded` after its request is answered.
- The repair test, renamed "runs a repair turn in the child after a failed postcondition", checks the step succeeded and that the child's view holds two prompts ending with the repair's reply.
- The cancel test checks the run view shows the step `Cancelled` after the child takes a prompt.
- The precondition test, renamed "shows a failed precondition's error under its step", checks the step is `Failed` and its `.run-status-error` holds the run's error. The run view shows no command output, so the output assertions were dropped.

Removed the unused `stepSection`, `replyIn` and `stepOrder` helpers and added `stepStatus` and `lastReply`.

## F2. The workflow guide describes the old sidebar and run view

Low · Documentation · Fixed · `docs/workflows.md:212`

"Running a workflow" said only agent steps' child sessions appear beneath the run, as each step starts, with hollow dots. The sidebar now lists every step from the start, without hollow dots; non-session rows select the run. The run view paragraph left out the run header and the failure or interruption shown under its step, and one line was unwrapped.

### Fix

Rewrote both paragraphs to describe every step's row, the pending dot, the run header, and the reason shown under the step a run stopped at, wrapped at 80 columns.

## F3. A sentence added to the architecture doc is unwrapped

Low · Documentation · Fixed · `docs/dev/architecture.md:179`

Adding the `started_at` sentence to "Workflow runs" left a line of about 130 characters in a file wrapped at 80 columns.

### Fix

Rewrapped the paragraph.

## Checks run

- `make check-ui` on the working tree, identical to the staged tree before the fixes: passed. Run again after the fixes: passed.
- `make check-rust`: fmt, clippy and build passed; `cargo test --workspace` failed once in `a_stopped_daemon_stores_the_end_of_the_turn_it_interrupted` (`panicked at crates/oxd/tests/daemon.rs:379:10: subscription ended: Disconnected`). The test does not touch this change; it passed five times alone and the whole `oxd --test daemon` suite passed on a rerun (67 passed).
- `make e2e` build steps, then `npx wdio run e2e/wdio.conf.ts --spec e2e/specs/run.e2e.ts`: 1 passing, 5 failing before the fix (F1); 6 passing after.

## Verdict

The daemon, protocol, MCP tool and run view wiring match the plan, and the Rust and UI checks pass. The review found one Medium testing gap (F1) and two Low documentation findings (F2, F3), all fixed.
