# Review of shell steps and checks in workflow runs

## Scope and coverage

The staged changes for `.agents/plans/2026-11-15-003-workflow-shell-steps-and-checks.md`: the new `shell` module, the runner's preconditions, shell steps, postconditions, repair turn, and cancellation in `run.rs`, the `run_commands` table in `store.rs`, step parsing in `workflow.rs`, `CommandInfo` in `ox-protocol` and `protocol.ts`, `oxctl run wait`'s error, the fake agent's `shell` and `next` directives, the integration tests in `crates/oxd/tests/runs.rs`, and the workflow, architecture, and glossary docs. Checked against the brief `.agents/briefs/2026-11-14-001-user-defined-workflows.md`, delivery task 3, and the plan's decisions. The change delivers the task's outcome: shell steps, checks with command output, stopping a running command, one repair turn, the full definition run end to end, and a final check that catches a missing commit and a dirty worktree.

Gaps:

- The live smoke suites were not run, and `scripts/smoke-acp.py` was not extended as plan steps 11 and 13 describe. The live-check rule in `docs/dev/testing.md` limits them to changes to the bridges, to how `oxd` talks to agents, or to the smoke script, and this change touches none of them. The work log records one live run of the brief's verbatim definition.

## Summary

| ID | Severity | Category    | Fix       | Title                                                        |
| -- | -------- | ----------- | --------- | ------------------------------------------------------------ |
| F1 | Low      | Correctness | Not fixed | A detached process holding a command's output holds the step |
| F2 | Low      | Performance | Not fixed | Checks keep their whole standard output in memory            |

## F1. A detached process holding a command's output holds the step

Low · Correctness · Not fixed · `crates/oxd/src/shell.rs:125`

`shell::run` treats a command as done when its shell exits, kills the rest of its process group, and then reads standard output and standard error to their end. A process that left the group with `setsid` and still holds either pipe keeps the read open, so the step waits until that process exits. Cancelling does not help: the stop path also awaits the read after `SIGKILL` reaches the group. The plan accepted that such a process keeps running, but not that it holds the run. Nothing in normal use detaches while keeping a command's pipes; the daemon that `oxctl` starts with `setsid` redirects its standard output and standard error.

### Evidence

A temporary unit test ran `python3 -c 'import os,time; os.setsid(); time.sleep(3)' & sleep 0.5; echo done`. The shell exited after 0.5 seconds, and `run` returned after 3.05 seconds. The test was removed.

### Fix

Not fixed. The fix for this edge case is a comment at the read naming it.

## F2. Checks keep their whole standard output in memory

Low · Performance · Not fixed · `crates/oxd/src/shell.rs:228`

`Output::read` appends every byte of standard output to `stdout` for every command, while the runner uses it only for a shell step's output. A check that prints a lot, such as a test suite, holds all of it until it ends, beyond the 64 KiB kept for its record. The plan keeps `stdout` only for a `shell` command; the work log records collecting it always as a decision. Realistic check output is a few megabytes at most.

### Fix

Not fixed.

## Checks run

- `make check-rust`: passed on the first run. A second run failed `file_watch_follows_edits_replacement_and_recreation` in `crates/oxd/tests/daemon.rs`, which this change does not touch; it passed five reruns alone.
- `make check-ui`: passed.
- A temporary change to `a_command_running_at_a_restart_is_interrupted` started a background `sleep` from the running command and checked it after the restart: the process was gone, confirming that a daemon shutdown kills a running command's group. The change was reverted.
- The temporary `setsid` unit test described in F1, reverted.

## Verdict

The change delivers task 3 as the brief and plan describe, with no High or Medium findings. F1 and F2 are Low and left unchanged.
