# Review of workflow runs

## Scope and coverage

Reviewed the staged implementation of `.agents/plans/2026-11-15-002-workflow-runs.md`: `ox-protocol`'s messages, client and `config_dir`, `Harness` in the desktop core, `oxctl run`, `oxd`'s `run.rs`, `workflow.rs`, `daemon.rs`, `store.rs`, `session.rs`, `conn.rs` and `main.rs`, the fake agent, the `runs` and `daemon` integration tests, the smoke script's `workflow` phase, `ui/src/lib/protocol.ts`, and the changed documentation. Also read the code the change depends on: `Session::start`, `stop`, `close` and `cancel`, the session state machine's handling of an agent that exits or is parked mid-turn, `Worktree::remove`, `roll_back` and `apply_branch_rule`, archive and unarchive, startup cleanup, and the UI's use of worktrees and the Merge button. Checked against task 2 of the brief `.agents/briefs/2026-11-14-001-user-defined-workflows.md` and the plan's decisions, including the departures the work log records.

The change delivers task 2. A run loads its definition, owns a worktree on a new branch, advances agent steps in child sessions with plain-text handoff, continues without clients, can be cancelled and removed under the existing unsaved-work and branch rules, and is `interrupted` after a restart with its history kept. Removing a child session leaves the run's worktree to the other children.

Gaps:

- The late config options path, where the runner waits for Pi's first `config_option_update`, has no integration test. The fake agent cannot report options late under a bridge name. The live Pi workflow phase covers it.

## Summary

| ID | Severity | Category      | Fix       | Title                                                              |
| -- | -------- | ------------- | --------- | ------------------------------------------------------------------ |
| F1 | Low      | Concurrency   | Not fixed | A run's listing change can name a child session not yet sent       |
| F2 | Low      | Documentation | Not fixed | Performance doc leaves runs out of watch versioning                |

## F1. A run's listing change can name a child session not yet sent

Low · Concurrency · Not fixed · `crates/oxd/src/daemon.rs:641`

`listing_changes` picks the outdated runs under the registry lock, but reads each run's `RunInfo` after it releases the lock. If the runner starts the next step's child session in between, the `Run` change names a session that this batch does not carry; the session arrives in the next batch. The `ListingChange` doc promises a run's child sessions before the run. Sessions do not have this problem with their worktrees, because a session's `worktree_id` is fixed when the session is created, while a step's `session_id` is set later. The window is a few microseconds, and the desktop ignores runs until task 4.

### Evidence

Traced `add_step_session`, which inserts the session and calls `run.set_step_session` in one registry update, against `listing_changes`, which selects `outdated` and `outdated_runs` under the lock at lines 568–583 and calls `run.versioned_info()` after `drop(registry)` at line 641.

### Fix

Not fixed. Reading each outdated run's `versioned_info` while the registry lock is held would keep a run's child sessions in the same batch or an earlier one.

## F2. Performance doc leaves runs out of watch versioning

Low · Documentation · Not fixed · `docs/dev/performance.md:9`

The Daemon section says a watch sends every workspace and session once, then only those that changed, and that each session counts the changes to its `SessionInfo`. Runs are now listing entries with their own version counter, which `Sent.runs` tracks the same way. The plan's documentation updates do not cover this page.

### Fix

Not fixed.

## Fixes outside the review

These fixes address failures the user asked to have investigated. They are not findings against this change.

### `make smoke-pi` native-skill check

`PiRun.check_commands` in `scripts/smoke-acp.py` prompted `/skill:ox-command-probe ARGUMENTS` and expected `OX_NATIVE_COMMAND_ARGUMENTS`. pi-acp passes the text to Pi unchanged, and Pi expands the skill into a `<skill>` block followed by the argument, so the wiring works. The model read the literal word `ARGUMENTS` as an unfilled placeholder. Pi's session logs of seven failed runs show five answers of `OX_NATIVE_COMMAND_` with an empty value and two refusals that call the text a placeholder or an injected probe. Isolated reruns of the native-skill turn passed 24 of 30 times with `ARGUMENTS` and 89 of 90 times with `copper-lantern`.

The check now passes `copper-lantern` and expects `OX_NATIVE_COMMAND_copper-lantern`. It still proves that the native skill ran, since the marker exists only in the skill body, and that it received its argument, since the value appears only in the prompt. `make smoke-pi` then passed three consecutive full runs, including the workflow phase. The remaining failure, about 1 in 90, is a refusal caused by the skill's `disable-model-invocation: true`, which keeps the skill out of Pi's skill list. Removing that line passed 60 of 60, but it would let the model read `SKILL.md` with a tool, so the check would also need to assert that the turn made no tool calls.

### `make e2e` stress stall

Not fixed. At the user's direction, the investigation stopped, and the stall stays a known failure that predates this change. The likely cause is WebKit's memory-pressure relief, not app code or the test's timing. The macOS log records `Received memory pressure event: 16` for the WebContent process during the failing stress runs, at a footprint of 1549–1669 MB. That footprint is mostly graphics memory that grows by about 700 MB while a followed scroller paints the stream. A simulated low-memory notification during streaming with the long transcript open stalled the main thread for about 700 ms, and none of the app's code ran during the stall. The test's probe, the app's timers, the fake agent, the desktop core's batching, and CPU load alone were ruled out. The stall is intermittent because the peak crosses the warning threshold only in some runs. All of the investigation's temporary changes are reverted.

## Checks run

- `make check`: passed (rustfmt, clippy, workspace tests including the 8 `runs`, 66 `daemon` and 64 `worktrees` integration tests, Biome, tsc, ruff), with the Pi smoke fix in place.
- Removing runs after a restart, through a temporary addition to `runs_survive_a_restart_and_running_ones_are_interrupted`, since reverted: `run.delete` without `discard` removed both the interrupted run, whose worktree directory was then gone, and the succeeded run, and the listing then held no sessions.
- `make smoke-pi`: three consecutive passes with the native-skill fix, including `workflow_hands_off_between_steps`.
- `make e2e` and `make smoke-claude` were not rerun. The review changed no code under review. The work log records `make smoke-claude` passing, and 11 of 12 e2e spec files passing, with only the stress stall above failing.

## Verdict

The change delivers task 2 of the brief as the plan decided. The runner, ownership rules, store, protocol and `oxctl` hold up under tracing, including cancellation at each wait, an agent that exits or is parked mid-turn, child deletion, and removal of failed and interrupted runs. There are no High or Medium findings. F1 is a narrow ordering race in watch batches, and F2 is a documentation gap. Both are recorded and left unfixed.
