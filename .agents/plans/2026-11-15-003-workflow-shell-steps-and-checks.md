# Add shell steps and checks to workflow runs

## Goal

Deliver task 3 of the brief's delivery sequence on top of the workflow runs of `.agents/plans/2026-11-15-002-workflow-runs.md`. A step can run a shell command instead of an agent. Any step can have preconditions and postconditions. A run records every command it runs, with its result and output. Cancelling a run stops a running command. An agent step whose postcondition fails gets one repair turn. The brief's full definition runs end to end, and its final check catches a missing commit and a dirty worktree.

**Brief:** `.agents/briefs/2026-11-14-001-user-defined-workflows.md`

## Related code

- `crates/oxd/src/workflow.rs`: `Workflow`, `Step`, `AgentStep`, `load`, and `parse`. Unknown fields, including `preconditions`, `postconditions`, and `shell`, are refused today.
- `crates/oxd/src/run.rs`:
  - `Run` and `RunRecord`, with `set_step`, `set_step_session`, `end`, and `update`, which save through the store and signal `changed`.
  - `advance` loops over the steps and hands each step's output to the next.
  - `StepContext::run` starts the child session, waits for `AgentStarted`, calls `configure`, prompts, and returns `final_reply`.
  - `next_events` waits on the session beside the run's `cancel` watch.
- `crates/oxd/src/branches.rs`: `fetch` starts git in its own process group with `process_group(0)`, and stops it with `libc::killpg(group, SIGTERM)`.
- `crates/oxd/src/worktree.rs`: `LOCAL_ENV_VARS`, which `Git::command` removes from git's environment.
- `crates/oxd/src/session.rs`: `run` launches the agent with the client's `env` over the daemon's environment.
- `crates/oxd/src/store.rs`: the `runs` and `run_steps` tables, `add_run`, `runs`, `set_step`, `interrupt_runs`, and `remove_run`.
- `crates/oxd/src/daemon.rs`: `start_run` passes the plan's `cwd` and the client's `env` to the runner. `close_all` aborts runners before closing sessions.
- `crates/ox-protocol/src/messages.rs`: `RunInfo`, `StepInfo`, `RunState`, `StepState`, and the `run.start` docs.
- `crates/oxd/src/bin/oxd-fake-agent.rs`: the directive parser (`["text" | "thought", ..]` decodes a JSON string) and `State`. Each prompt's text is the turn's script, and an unknown directive refuses the prompt.
- `crates/oxd/tests/runs.rs`: the hard-linked `bin_dir` fixture, `Daemon::define`, `repo`, `until_run`, `until_ended`, and `turn_ended`.
- `crates/oxctl/src/main.rs`: `RunCommand::Wait` prints the ended run as JSON.
- `scripts/smoke-acp.py`: `check_workflow`, the two-step live phase, and each agent's `workflow_agent`.
- `ui/src/lib/protocol.ts`: mirrors `RunInfo` and `StepInfo`.

## Decisions

- **Definition.** In `workflow.rs`, `Step` becomes `{ name, action: Action, preconditions: Vec<Check>, postconditions: Vec<Check> }`:
  - `Action` is `Agent { agent: AgentStep, prompt: String }` or `Shell { command: String }`.
  - `Check { shell: String }`, with `deny_unknown_fields`.

  YAML gives a step either `agent` and `prompt`, or `shell`, plus optional `preconditions` and `postconditions` lists. `Step` deserializes through `#[serde(try_from = "StepFields")]`. `StepFields` holds every key as an `Option` with `deny_unknown_fields`, and the conversion refuses a step with `shell` beside `agent` or `prompt`, or with neither, with the message `step <name> needs either agent and prompt, or shell`.
- **Protocol.** `StepInfo` gains `commands: Vec<CommandInfo>`, the commands the step has started, in order:
  - `CommandInfo { kind: CommandKind, command: String, state: CommandState, exit_code: Option<i32>, output: String, truncated: bool }`
  - `CommandKind` is `precondition`, `shell`, or `postcondition`.
  - `CommandState` is `running`, `succeeded`, `failed`, `cancelled`, or `interrupted`.
  - `exit_code` is unset while the command runs, and when it was killed by a signal or could not start.
  - `output` is the command's standard output and standard error, interleaved as they arrived. When the command ends, it holds the last 64 KiB, and `truncated` says whether earlier output was dropped. It is empty while the command runs.

  `StepInfo.output` becomes the final reply of the step's last turn, or a shell step's standard output. The next step receives it once the step has succeeded. A step whose postcondition still fails after its repair turn keeps the repair turn's reply as its `output`.
- **Running a command.** New module `crates/oxd/src/shell.rs` with `shell::run(command, cwd, env, inputs, stop) -> Result<Finished, Stopped>`:
  - It runs `/bin/sh -c <command>` in `cwd`, the run's checkout, with stdin null, in its own process group.
  - `env` is the client's `env` over the daemon's environment, as for agents, without `LOCAL_ENV_VARS`. `worktree.rs` makes `LOCAL_ENV_VARS` `pub(crate)`.
  - `OX_RUN_INPUT` holds the run's input, and `OX_STEP_INPUT` the step's input: the previous step's output, or the run's input for the first step.
  - `Finished { exit: ExitStatus, stdout: String, output: String, truncated: bool }`. `stdout` is the full standard output and is kept only for a `shell` command. `output` and `truncated` are as in `CommandInfo`.
  - When the shell exits, the rest of its process group gets `SIGKILL`, and the output is read to the end. A command is done when its shell exits, and background processes it left in its group do not hold the run.
  - When `stop` resolves, the group gets `SIGTERM`. If the shell has not exited 5 seconds later, the group gets `SIGKILL`. `run` then returns `Stopped`.
  - A guard sends `SIGKILL` to the group when the future is dropped, so a runner that `close_all` aborts leaves no command running.
  - A command that cannot start, for example because its environment is too large, fails with the error as its `output`.
- **Checks and shell steps in the runner.** `StepContext::run` in `run.rs` does the following for each step:
  1. Runs the preconditions in order. The first one that fails fails the step and the run. An agent step's session is not started.
  2. Runs the action. An agent step runs as it does today, and its output is its final reply. A shell step runs its command as a `shell` command. A non-zero exit fails the step and the run, and a zero exit makes its standard output the step's output.
  3. Runs the postconditions in order, stopping at the first failure. For a shell step, that failure fails the step and the run.
  4. For an agent step, a failed postcondition prompts the same session once with the repair prompt, waits for that turn's final reply as in `final_reply`, and runs every postcondition again from the first. A failure then fails the step and the run, with the repair turn's reply as the step's output. A repair turn that ends other than with `end_turn` fails the step as any turn does.

  Each command is recorded in the step's `commands` as `running` when it starts, and with its result when it ends. `Run` gains `start_command(step, kind, command) -> usize` and `finish_command(step, index, state, exit_code, output, truncated)`, which save through the store and signal the change like `set_step`.
- **Repair prompt.** One text block:

  ```text
  A postcondition of this step failed after your turn. Fix what it checks so that it passes, or reply with the reason you cannot.

  Command:
  <command>

  Result: <result>

  Output:
  <output>
  ```

  `<result>` is `exited with status <n>`, `was killed by signal <n>`, or `could not start`. The `Output:` section is left out when the output is empty.
- **Run errors.** The run's `error` names the failed command and its result:
  - `a precondition exited with status 1`
  - `the command exited with status 2`
  - `a postcondition exited with status 1`, for a shell step
  - `a postcondition exited with status 1 after the repair turn`, for an agent step

  A command killed by a signal reads `was killed by signal <n>`, and one that could not start reads `could not start: <error>`.
- **Cancelling.** The runner passes `cancel.wait_for(|cancelled| *cancelled)` to `shell::run` as `stop`. A stopped command is `cancelled`, and the step and the run end `cancelled` as they do for a cancelled turn. A cancel during a repair turn cancels that turn.
- **Store.** New table, created with `CREATE TABLE IF NOT EXISTS` after `run_steps`, so databases from task 2 gain it without a reset:

  `run_commands (run_id TEXT NOT NULL, step INTEGER NOT NULL, position INTEGER NOT NULL, kind TEXT NOT NULL, command TEXT NOT NULL, state TEXT NOT NULL, exit_code INTEGER, output TEXT NOT NULL, truncated INTEGER NOT NULL, PRIMARY KEY (run_id, step, position), FOREIGN KEY (run_id, step) REFERENCES run_steps(run_id, position) ON DELETE CASCADE)`

  The store changes are:
  - `save_command(run_id, step, position, &CommandInfo)` inserts or replaces a command row.
  - `runs()` loads each step's commands in order.
  - `interrupt_runs()` also marks `running` commands `interrupted`.
  - `remove_run` removes the commands through the cascade.
- **Fake agent.** Two directives:
  - `shell <JSON string>` runs the decoded text with `sh -c` in the session's cwd and appends its standard output to the turn's message. A non-zero exit ends the turn with the stop reason `error` and the command's standard error as the message.
  - `next <JSON string>` makes the decoded text the script of the session's next prompt, which the agent then runs in place of that prompt's text. The prompt is still echoed as a `user_message`. With it, a test step answers the daemon's repair prompt.
- **oxctl.** `run wait` on a run that did not succeed fails with `the run <state>: <error>`, after printing the run as JSON as it does today.
- **UI.** `protocol.ts` mirrors `CommandInfo`, `CommandKind`, `CommandState`, and `StepInfo.commands`. The desktop shows none of it until task 4.

## Check your assumptions

1. **How shell commands receive the run's input and the previous step's output.** Kept.
   - **Decided:** As two environment variables, `OX_RUN_INPUT` and `OX_STEP_INPUT`, given to shell steps and to checks. Stdin is null. Ox already names its variables `OX_*`, such as `OX_SOCKET`.
   - **Consequences:** A command reads them as `"$OX_STEP_INPUT"`, with no quoting hazards. An input or output larger than the system's limit on arguments and environment, 1 MiB on macOS, makes the command fail with `could not start`.
   - **Alternative:** Pipe the step's input to the command's stdin, and give only the run's input as a variable.
   - **Trade-offs:** Variables are one mechanism for both texts and for every kind of command, but have a size limit. Stdin has no limit and makes a shell step a filter, such as `jq` or `grep`, but gives the two texts two different mechanisms, and a command that does not read stdin needs the writer to tolerate a closed pipe.
2. **Who writes the repair prompt.** Kept.
   - **Decided:** Ox sends the fixed text in Decisions, with the failed command, its result, and its output.
   - **Consequences:** Every agent step's repair turn reads the same way. The definition cannot change the wording.
   - **Alternative:** An optional `repair_prompt` field on agent steps, with Ox appending the failure details.
   - **Trade-offs:** The fixed text keeps the definition format smaller and gives every workflow the brief's behavior. A field lets a workflow tell the agent more about what the check means, at the cost of one more field to document and validate.
3. **Which reply a repaired step hands to the next step.** Kept.
   - **Decided:** The repair turn's final reply, the last turn of the step. A step that still fails keeps that reply as its `output`, which is how the agent's reason appears beside the failed check.
   - **Consequences:** When the repair fixes the problem, the next step receives something like "I staged the plan." instead of the first turn's fuller reply. The brief's example steps find their inputs among the staged files, so they are unaffected.
   - **Alternative:** Hand off the first turn's reply, and keep the repair turn's reply in a separate field.
   - **Trade-offs:** The last reply keeps one meaning for `output` and one field for the protocol and the view. Handing off the first reply keeps the substance for the next step, but adds a field and makes the handoff depend on whether a repair happened.
4. **How command output is kept.** Kept.
   - **Decided:** Each command's interleaved standard output and standard error, recorded when the command ends, limited to the last 64 KiB with `truncated` set when more was dropped. A shell step's standard output is handed off in full.
   - **Consequences:** A long `make check` keeps its final failures but not its start. The output of a running command appears only when it ends. `RunInfo`, which a watch resends on every change, stays small.
   - **Alternative:** Keep all output, or stream it while the command runs.
   - **Trade-offs:** The tail keeps the database and every listing change small, at the cost of the start of long outputs. Keeping or streaming everything shows all of it, but large outputs then travel with each change to the run.
5. **How a running command is stopped.** Kept.
   - **Decided:** Cancelling sends `SIGTERM` to the command's process group, then `SIGKILL` after 5 seconds. A daemon shutdown sends `SIGKILL` at once. Processes the command leaves in its group are killed when its shell exits. There is no timeout. This follows `branches::fetch`, which stops git with `SIGTERM` to its group.
   - **Consequences:** A hung check holds its run until the user cancels it. A command that daemonizes something with `setsid` escapes the group and keeps running.
   - **Alternative:** A `timeout` field on steps and checks.
   - **Trade-offs:** No timeout keeps the definition smaller and never stops a slow but healthy command. A timeout frees a hung run without the user, but every definition would need to choose one.
6. **How the full workflow definition is verified.** Kept.
   - **Decided:** Three ways:
     - An integration test runs the brief's definition with its checks verbatim, and with fake-agent scripts as prompts.
     - The smoke `workflow` phase gains a check and a shell step on each real bridge.
     - The implementer runs the brief's definition verbatim once, through `oxctl` on a scratch repository with a small task, answers Claude's `manual` permission requests with `oxctl permit`, and records the result in the work log.
   - **Consequences:** `make smoke-claude` and `make smoke-pi` take one more short model turn each. The verbatim run costs one Opus high-effort plan and review, one GPT 6.1 Terra implementation, and one Haiku commit, and is not repeated by `make smoke`.
   - **Alternative:** Add the verbatim definition to `make smoke`.
   - **Trade-offs:** One recorded run proves the real models and bridges complete the brief's workflow without making every smoke run pay for four high-effort turns. Putting it in the smoke run catches regressions automatically, at that cost each time, and needs the smoke script to answer permission requests.

## Test plan

- `crates/oxd/tests/runs.rs`, with the existing fixture:
  - **Shell handoff.** A run of a shell step `printf 'shell:%s' "$OX_STEP_INPUT"` then an agent step `text "agent "` with input `in` ends `succeeded`. Step 1's output is `shell:in`, step 2's is `agent shell:in`, and step 1 lists one `shell` command, `succeeded`, exit code 0, with output `shell:in`. A second shell step reads `$OX_RUN_INPUT`, and its output is `in`.
  - **Precondition failure.** An agent step whose precondition is `echo dirty >&2; exit 3` fails the run with `a precondition exited with status 3`. The command is `failed`, exit code 3, with output `dirty`. The step has no session, and later steps stay `pending`.
  - **Shell step failure.** A shell step `echo boom; exit 2` fails the run with `the command exited with status 2`, and its command's output is `boom`.
  - **Repair succeeds.** An agent step whose prompt is `next "shell \"touch fixed\"\ntext \"fixed it\""` and `text "done"`, with the postcondition `test -f fixed`, ends `succeeded` with output `fixed it`. Its commands are a failed then a succeeded postcondition. The session's second user message is the repair prompt, naming `test -f fixed` and `exited with status 1`.
  - **Full definition.** The brief's definition, with the fake agent's options and its checks verbatim. The scripts:
    - Plan writes and stages `.agents/plans/p.md`.
    - Work writes and stages `src.txt` and `.agents/work/w.md`.
    - Review runs `shell "git diff --cached --name-only"` and writes and stages `.agents/reviews/r.md`.
    - Commit runs `git commit`.

    The run ends `succeeded`. Review's output lists the plan, `src.txt`, and the work log. The worktree has one new commit over the starting `HEAD` containing all four files, and a clean status.
  - **Missing commit.** The same definition, with a Commit step that does not commit on either turn and replies `cannot commit` to the repair prompt, fails with `a postcondition exited with status 1 after the repair turn`. Commit's output is `cannot commit`, and its commands are two failed postconditions.
  - **Dirty worktree.** The same definition, with a Commit step that commits but leaves an untracked file on both turns, fails the same way.
  - **Cancel stops a command.** A shell step `sleep 30 & echo $! > "$OX_RUN_INPUT"; wait`, whose input is a path outside the worktree, ends `cancelled` within the test timeout after `run.cancel`. Its command is `cancelled`, and the background `sleep` process no longer exists.
  - **Restart during a command.** A run whose shell step is `sleep 30` lists that command `interrupted` after a restart.
- `workflow.rs` unit tests:
  - A shell step and an agent step with preconditions and postconditions parse.
  - A step with both `shell` and `agent`, with neither, or with a check that has an unknown field is refused.
  - The existing unknown-field case uses a field that stays unknown.
- `shell.rs` unit tests:
  - Standard output and standard error are kept apart in `stdout` and interleaved in `output`.
  - Output past 64 KiB keeps its tail and sets `truncated`.
  - `OX_RUN_INPUT` and `OX_STEP_INPUT` reach the command.
  - A command that leaves a background process holding its standard output returns once the shell exits.
  - `stop` ends a command that ignores `SIGTERM` after the grace period.
- `store.rs` unit tests: a run's commands round-trip in order, and `interrupt_runs` marks only `running` commands.
- `messages.rs`: a `RunInfo` with commands round-trips through JSON.
- The smoke `workflow` phase described below, and the verbatim run described in Check your assumptions.

## Implementation plan

1. `ox-protocol`: add `CommandInfo`, `CommandKind`, `CommandState`, and `StepInfo.commands`. Update the `StepInfo.output` and `run.start` docs. Extend the `RunInfo` serialization test.
2. `workflow.rs`: add `Action`, `Check`, `StepFields`, and the step fields, update the module doc, and adjust the unit tests.
3. `worktree.rs`: make `LOCAL_ENV_VARS` `pub(crate)`.
4. `shell.rs`: add `run`, `Finished`, `Stopped`, the process-group guard, and the unit tests. Register the module in `main.rs`.
5. `store.rs`: add the `run_commands` table and `save_command`, load commands in `runs`, extend `interrupt_runs`, and add the unit tests.
6. `run.rs`: add `start_command` and `finish_command`. Restructure `StepContext::run` into preconditions, the action, and postconditions with the repair turn. Carry a failed step's output through `end`. Write the run errors and the repair prompt as Decisions gives them.
7. `oxd-fake-agent`: add the `shell` and `next` directives, and list them in the module doc.
8. `crates/oxd/tests/runs.rs`: add the tests.
9. `oxctl`: include the state and error in `run wait`'s failure.
10. `ui/src/lib/protocol.ts`: mirror the new types.
11. `scripts/smoke-acp.py`: extend `check_workflow`'s definition and checks:
    - `Extract` gains the precondition `test -z "$(git status --porcelain)"`.
    - A new agent step `Write` runs before `Repeat`, with the prompt "Write your input, exactly, into a new file named token.txt in the current directory. Do not use other tools." and the postcondition `grep -qF "$OX_STEP_INPUT" token.txt`.
    - A shell step `Stage` runs `git add token.txt && git diff --cached --name-only`.

    The phase checks that `Stage`'s output is `token.txt`, and reports each step's commands.
12. Run the brief's definition verbatim through `oxctl` on a scratch repository, and record the run's JSON summary and the final commit in the work log.
13. Run `make smoke-claude` and `make smoke-pi`.

## Documentation updates

- `docs/workflows.md`:
  - The opening describes agent and shell steps.
  - New "Shell steps" section: the `shell` field, `/bin/sh -c` in the run's checkout, the client's environment, `OX_RUN_INPUT` and `OX_STEP_INPUT`, standard output as the step's output, and a non-zero exit failing the run.
  - New "Checks" section: `preconditions` and `postconditions` lists of `shell` entries, their order, the repair turn and its prompt, the step's output after a repair, and the recorded output and its 64 KiB limit.
  - The brief's full definition as an example.
  - "Cancelling" says that cancelling stops a running command and its process group.
- `docs/dev/architecture.md`, "Workflow runs": the runner runs checks and shell steps through `shell::run` in their own process groups, and records each command in the step's `commands`. A daemon shutdown kills running commands.
- `docs/dev/glossary.md`:
  - **Step** covers shell steps.
  - Add **Check**: a precondition or postcondition shell command of a step.
