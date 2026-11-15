# Add shell steps and checks to workflow runs

## Plan

`.agents/plans/2026-11-15-003-workflow-shell-steps-and-checks.md`

## Summary

A step is an agent step or a shell step, and any step can have preconditions and postconditions. `shell::run` runs each command with `/bin/sh -c` in the run's checkout, in its own process group, with `OX_RUN_INPUT` and `OX_STEP_INPUT`. Each command is recorded in the step's `commands`, saved in the new `run_commands` table, and marked `interrupted` after a restart. Cancelling stops a running command's group. An agent step whose postcondition fails gets one repair turn and keeps that turn's reply. `oxctl run wait` fails with the run's state and error. The brief's full definition ran end to end with real models, and the integration tests show that its final check catches a missing commit and a dirty worktree. The plan's goal is met.

## Departures from the plan

- The smoke suites were not run, and `scripts/smoke-acp.py` is unchanged. The live-check rule in `docs/dev/testing.md` changed during the work: the smoke suites now run only for changes to the bridges, to how `oxd` launches agents or exchanges ACP messages, or to the smoke script. Plan steps 11 and 13 were dropped. The verbatim run of the brief's definition (step 12) still ran once.
- `Finished.exit` is `Option<ExitStatus>`. It is unset when the command could not start, because no `ExitStatus` exists for that case.
- `Stopped` carries the output and `truncated` flag of the command until it was stopped, so a cancelled command keeps its output.
- In the integration tests, agent steps receive fake-agent directives as input, because the fake agent runs its whole prompt, input included, as a script. The shell-handoff test's first step prints `text "shell:%s"`, so its output is `text "shell:in"`, not `shell:in`. In the full definition, Review hands Commit the staged names as `text "<path>"` lines.
- The existing integration test `a_missing_or_invalid_workflow_is_refused` uses `retries` as its unknown field, as the `workflow.rs` unit test does.

## Decisions

- `shell::run` always collects standard output, and the runner uses it only for a shell step.
- The repair prompt trims trailing whitespace from the command and its output.
- A failed shell step, and an agent step whose precondition fails, end with no output.
- The fake agent's `next` script is kept per session ID. The `shell` directive runs in the directory that holds the session's log, which is its cwd. The session log records the prompt's text, not the `next` script that replaced it.

## Checks run

- `make check`: passed. This covers rustfmt, clippy `-D warnings`, workspace tests (including 15 `runs` integration tests and the new `shell`, `store`, `workflow`, and `messages` unit tests), Biome, tsc, and ruff.
- `make e2e`: 11 of 12 spec files passed. `stress.e2e.ts` failed "streams a long reply into a long transcript without stalling", which is known to fail at HEAD as well. Its next test, "keeps a long transcript responsive while other sessions change", then timed out in the same `settle` call while the webview was still stalled. The UI part of this change adds only types. The stress spec was left alone, as directed.

## Manual verification

1. The brief's definition, verbatim, on a scratch repository with a small task, through `oxctl`. Each permission request from Claude's `manual` mode was allowed with `oxctl permit`.

   ```sh
   # drive.py: makes a repository with greet.py, copies the brief's YAML block to
   # $XDG_CONFIG_HOME/ox/workflows/full.yaml, starts oxd on a scratch socket, runs
   # `oxctl run start --workspace <id> full "Add a --shout flag to greet.py ..."`,
   # attaches to each child session to answer permission requests with allow_once,
   # and prints `oxctl run wait`'s result.
   python3 -I drive.py "$(mktemp -d)"
   ```

   The run `succeeded` in 1.8 minutes. Each check ran once and exited 0: Plan's precondition and postcondition, then the postconditions of Work, Review, and Commit. No repair turn was needed. Claude asked for seven permissions across Plan, Review, and Commit. Commit `63684c0` "Add --shout flag to greet.py and unittest coverage" is the only commit over the starting `HEAD`. It contains `greet.py`, `test_greet.py`, the plan, the work log, and the review (5 files, +297/−2). The worktree was clean, and `ox-workflow-start-head` was removed. Review deleted the `__pycache__` that its test run left before finishing. Otherwise the final check would have counted it as untracked.

## Follow-up work

- The brief's final check counts files that a step's tools leave behind, such as `__pycache__`, unless the repository ignores them. A workflow for a repository without such ignores needs a step or prompt that cleans them up.
