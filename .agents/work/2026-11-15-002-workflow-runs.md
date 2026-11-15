# Run two agent steps through oxctl

## Plan

`.agents/plans/2026-11-15-002-workflow-runs.md`

## Summary

`oxd` loads `<XDG config>/ox/workflows/<name>.yaml`, starts a run in a worktree the run owns, and advances its agent steps in child sessions. Each step's final reply is handed to the next as plain text. Runs continue without clients, can be cancelled and removed, are listed and watched as `RunInfo`, and come back `interrupted` after a restart. `oxctl run start|wait|cancel|rm` drives them, and `Harness` is shared by the desktop and the daemon. The plan's goal is met. The checks below that did not pass fail the same way they did before this change.

## Departures from the plan

- The runner waits for the agent's first `config_option_update` before setting options when the agent started with no options and the step sets any. `pi-acp` answers `session/new` with no options and reports them once Pi starts, so the first live Pi run failed with "the agent has no model option". No integration test covers this. The test fixture launches the fake agent under a bridge name with no arguments, so it cannot ask for late options. The live Pi workflow phase covers it. A local `TODO` covers an agent that never reports options.
- `SavedRun.steps` is `Vec<StepInfo>`. A separate `SavedStep` would have had the same fields.
- `add_step_session` takes the step's `cwd` as well, because the runner keeps the plan's `cwd` in memory.
- `model`, `effort`, and `permission_mode` are all optional in `AgentStep`.
- The fake agent's `ConfigOption.category` is no longer an `Option`, since every option has one now.
- `crates/oxd/tests/daemon.rs` folds `run` and `run_removed` changes in its exhaustive listing `apply`.
- `README.md`'s `oxctl` row and document links mention workflows, matching the new `oxctl` about text.
- The Pi smoke step uses effort `low`, not Pi's lowest level. Pi's levels depend on the model, and the existing thinking phase already shows that `low` works with Make's model. For Claude, `low` is the lowest.

## Decisions

- `run.start`'s `input` is required. `env` defaults to empty.
- Failure messages: `starting the agent: …`, `the agent did not start: …`, `the agent has no <model|effort|permission mode> option`, `setting the <label> to <value>: …`, the message of an `error` stop reason, `the turn ended with stop reason <reason>`, a rejected prompt's error, and `the step's session was deleted`.
- Cancelling sends `session/cancel` only when the step has prompted. A step cancelled earlier ends without touching its session.
- The final reply comes from the message that started last in the turn. Chunks append to their message, and an `agent_message` update with content replaces it.
- A step and its run are saved and published in one change when a run ends, so a watch never sees a failed step under a running run.
- `delete_run` answers `NOT_FOUND` if the run was removed while it waited for its child sessions' lifecycle locks.
- `oxctl run wait` prints the run, then fails with `the run did not succeed` unless it succeeded.

## Checks run

- `make check`: passed (rustfmt, clippy `-D warnings`, workspace tests including 8 `runs`, 66 `daemon`, and 64 `worktrees` integration tests, Biome, tsc, ruff). The first run failed once in `claude-code-acp`'s `a_prompt_is_answered_when_it_enters_the_conversation`, a crate this change does not touch. That test passed three times alone and in the next full run.
- `make e2e`: 11 of 12 spec files passed. `stress.e2e.ts` failed "streams a long reply into a long transcript without stalling" three times, at 393–433 ms against a 100 ms budget. Run alone, it measured 414 ms and then 21 ms. The previous work log recorded this test failing in this checkout before this change. HEAD fails it too (comparison below).
- `make smoke-claude`: passed, including `workflow_hands_off_between_steps`.
- `make smoke-pi`: failed twice at `native_skill_invoked_with_arguments`, before the workflow phase. The model first answered `OX_NATIVE_COMMAND_` and then refused the skill. The previous work log recorded both failures, and every daemon check before that step passed. The workflow phase was run separately (below) and passed.

## Manual verification

1. The Pi workflow phase alone, through the smoke script's own `PiRun`.

   ```sh
   # /tmp/ox-pi-workflow-driver/run.py loads scripts/smoke-acp.py, subclasses
   # PiRun with checks() returning (self.check_workflow,), and calls main()
   # with --agent pi --model openrouter/deepseek/deepseek-v4.1-flash-1017.
   python3 -I /tmp/ox-pi-workflow-driver/run.py
   ```

   Before the late-options fix, the run failed with `the agent has no model option`. After it, `workflow_hands_off_between_steps` reported both outputs as the token. The driver then raised `AttributeError` in `run()`'s `disconnect()`, because no other phase opened an attachment. That error comes from the driver.

2. The stress spec, built from HEAD and from this change in a separate worktree.

   ```sh
   # /tmp/ox-stress-compare/run.sh
   git worktree add --detach /tmp/ox-baseline HEAD
   ln -s "$PWD/ui/node_modules" /tmp/ox-baseline/ui/node_modules
   cd /tmp/ox-baseline && cargo build -p oxd -p ox-mcp -p oxctl
   cd ui && npm run tauri -- build --debug --no-bundle --features webdriver
   npx wdio run e2e/wdio.conf.ts --spec e2e/specs/stress.e2e.ts   # three times
   # Then copy this change's files over, touch the crates' .rs files, rebuild, and run again.
   ```

   HEAD failed all three runs: 428 ms, 392 ms, and one run that failed after 7 m 48 s without a measurement. This change measured 25 ms, 402 ms, and 43 ms. The stall is intermittent on this machine and predates this change, whose UI part adds only types.

## Follow-up work

- Find why the stress spec's streaming stall intermittently reaches about 400 ms on this machine, at HEAD as well.
- Let the fake agent take its flags when it runs under a bridge name, so the late-options path gets an integration test.
