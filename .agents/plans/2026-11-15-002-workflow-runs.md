# Run two agent steps through oxctl

## Goal

Deliver task 2 of the brief's delivery sequence. The daemon loads a named workflow definition, creates a worktree the run owns, and advances its agent steps in child sessions, handing each step's final reply to the next as plain text. A run keeps going without clients, can be cancelled and removed, and its record survives a restart. `oxctl` starts, follows, cancels, and removes runs.

**Brief:** `.agents/briefs/2026-11-14-001-user-defined-workflows.md`

## Related code

- `crates/oxd/src/daemon.rs`:
  - `Registry`, `Sent`, `list`, and `listing_changes` publish workspaces, worktrees, and sessions.
  - `new_session` and `add_session` plan, save, and start a session and its worktree.
  - `start_in_worktree` adds a worktree and rolls it back on failure.
  - `delete_session`, `archive_session`, `unarchive_session`, `forget`, `Daemon::new`, and `clean_up` treat a session's worktree as owned by that session.
  - `is_idle` and `close_all` govern daemon exit and shutdown.
  - `remove_workspace` refuses a workspace with sessions.
- `crates/oxd/src/session.rs`: `Session::create`, `start`, `prompt`, `set_config_option`, `cancel`, `events_after`, and `stop`, which the runner drives.
- `crates/oxd/src/worktree.rs`: `plan`, `create_branch`, `add`, `roll_back`, and `Worktree::remove(git, discard, stop)` with `apply_branch_rule`.
- `crates/oxd/src/store.rs`: the schema, `add_session` (which inserts a new worktree row with its session), `remove_session` (which deletes the session's worktree row), and `workspace_has_sessions`.
- `crates/oxd/src/conn.rs`: the list of methods spawned as their own tasks, so they finish when the client disconnects.
- `crates/oxd/src/main.rs` and `mcp.rs`: paths beside the socket (`<socket>.db`, `worktrees`), and `ox-mcp` resolved beside the daemon's executable.
- `crates/oxd/src/bin/oxd-fake-agent.rs`: the scripted agent. Its prompt's text blocks are joined with newlines and run as directives. Its `mode` option has no category.
- `crates/ox/src/lib.rs`: `Agent` maps `claude` and `pi` to their CLI and bridge names. `agents` and `new_session` use it.
- `crates/ox-protocol/src/messages.rs` and `client.rs`: `Method`, `Listing`, `ListingChange`, `Deleted`, and the client wrappers.
- `crates/oxctl/src/main.rs`: subcommands over `Client`.
- `ui/src/lib/protocol.ts`: mirrors `ListingChange`. `useWorkspaces`'s `apply` leaves its listing unchanged for change types it does not handle.
- `crates/oxd/tests/worktrees.rs`: the `Daemon`, `repo`, and `git` fixtures a new test file follows.
- `scripts/smoke-acp.py`: live phases against real bridges, one `SmokeRun` per agent.

## Decisions

- **Shared harness selection.** Move `Agent` from `crates/ox/src/lib.rs` to `ox-protocol` as `Harness { Claude, Pi }`, serialized `claude` and `pi`. It has `executable()` (`claude` or `pi`) and `bridge()` (`claude-code-acp` or `pi-acp`). The desktop's `agents` and `new_session` use it, and the UI's values are unchanged. `oxd` launches a step's agent as `[<directory of oxd's executable>/<bridge>]`, as it finds `ox-mcp`.
- **Workflow definitions.** New module `crates/oxd/src/workflow.rs` holds the definition types, deserialized with `serde-saphyr` and `deny_unknown_fields`:
  - `Workflow { name, steps: Vec<Step> }`
  - `Step { name, agent: AgentStep, prompt }`
  - `AgentStep { harness: Harness, model, effort, permission_mode: Option<String> }`

  `workflow::load(dir, workflow)` reads `<dir>/<workflow>.yaml`. It refuses with `INVALID_REQUEST` a name that is not a plain file name or starts with `.`, a file that does not parse (with the parser's message), and a workflow with no steps. A missing file is `NOT_FOUND` (`no workflow <name>`). YAML anchors and aliases work, as in the brief's example. Preconditions, postconditions, and shell steps are unknown fields until task 3.
- **Definitions directory.** `ox-protocol` gains `config_dir()`, beside `data_dir()` and following the same rule: `$XDG_CONFIG_HOME/ox` when `XDG_CONFIG_HOME` is an absolute path, else `~/.config/ox`, else `<temp dir>/ox` when there is no home directory. The daemon reads definitions from `config_dir()/workflows`, read from its own environment. `main.rs` passes that path to `Daemon::new`. A definition is read once, when a run starts.
- **Protocol.**
  - `RunInfo { id, workspace_id, workflow, name, input, worktree_id, state: RunState, error: Option<String>, steps: Vec<StepInfo> }`. `workflow` is the file name the run was started with, and `name` is the definition's `name`.
  - `StepInfo { name, state: StepState, session_id: Option<String>, output: Option<String> }`.
  - `RunState` is `running`, `succeeded`, `failed`, `cancelled`, or `interrupted`. `StepState` is `pending`, `running`, `succeeded`, `failed`, `cancelled`, or `interrupted`.
  - `error` says why a run failed: the worktree could not be added, the agent could not start, a config option was missing or refused, the turn ended with an error or a stop reason other than `end_turn`, or the step's session was deleted.
  - Methods: `run.start { workspace_id, workflow, input, env }` returns `RunInfo`. `run.cancel { run_id }` returns null. `run.delete { run_id, discard, env }` returns `Deleted`.
  - `Listing` gains `runs`. `ListingChange` gains `Run { run: RunInfo }` and `RunRemoved { run_id }`. `listing_changes` emits removals as runs, sessions, then worktrees, and then additions as workspaces, worktrees, sessions, then runs, so a run arrives after its worktree and sessions.
  - `Client` gains `start_run`, `cancel_run`, and `delete_run`. `conn.rs` spawns `run.start` and `run.delete` as their own tasks, beside `session.new` and `session.delete`.
- **Worktree ownership.** A run owns its worktree. Its child sessions refer to the worktree through `worktree_id` and do not own it. `Daemon::owned_worktree(session)` returns the session's worktree unless a run in the registry owns it. The following use it:
  - `delete_session` removes only an owned worktree. Deleting a child stops its agent and forgets the session.
  - `archive_session` and `unarchive_session` park and unpark a child without removing or restoring the worktree.
  - `forget` removes and closes only an owned worktree.
  - `Daemon::new` sends only owned worktrees to `unstarted`. A child session that never started is kept, `exited`, and is removed with its run.
- **Store.** New tables, created with `CREATE TABLE IF NOT EXISTS` after `sessions`, so existing databases gain them without a reset:
  - `runs (number INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, workspace_id TEXT NOT NULL REFERENCES workspaces(id), worktree_id TEXT NOT NULL REFERENCES worktrees(id), workflow TEXT NOT NULL, name TEXT NOT NULL, input TEXT NOT NULL, state TEXT NOT NULL, error TEXT)`
  - `run_steps (run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE, position INTEGER NOT NULL, name TEXT NOT NULL, state TEXT NOT NULL, session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL, output TEXT, PRIMARY KEY (run_id, position))`

  The store changes are:
  - `SavedRun` with `SavedStep`s.
  - `add_run(&SavedRun, &SavedWorktree) -> number` inserts the worktree, the run, and its pending steps in one transaction.
  - `runs()` lists the runs.
  - `set_run_state(id, state, error)` and `set_step(run_id, position, state, output)`.
  - `interrupt_runs()` marks `running` runs and steps `interrupted`.
  - `remove_run(id)` deletes the run's sessions, the run, and its worktree in one transaction.
  - `add_session`'s worktree parameter becomes `SessionWorktree<'_>`: `None`, `New(&SavedWorktree)`, or `Step { run_id, position, worktree_id }`. `Step` saves the session with the run's `worktree_id` and sets the step's `session_id` in the same transaction.
  - `remove_session` deletes the session's worktree row only when no run owns it.
  - `workspace_has_sessions` also counts runs. `remove_workspace`'s message becomes "the workspace has sessions or runs".
- **Live run.** New module `crates/oxd/src/run.rs`. `Run` holds:
  - `id`, `number`, `workspace_id`, `workflow`, `name`, `input`
  - `worktree: Arc<Worktree>`
  - `Mutex<RunRecord>`, with the state, the error, and each step's state, session ID, and output
  - `version: AtomicU64` and the daemon's `changed` sender
  - a `cancel: watch::Sender<bool>`
  - the runner's `JoinHandle`

  Each change saves through the store, bumps `version`, and sends `changed`, as `Worktree` does. Its methods are `info`, `versioned_info`, `version`, `running`, `session_ids`, `forget_session(id)`, `cancel`, and `close` (which aborts the runner).
- **Starting a run.** `Daemon::start_run` does the following:
  1. Loads the definition.
  2. Plans the worktree with no branch and no comparison ref, so it is on `ox/<worktree id8>` at `HEAD`.
  3. Creates the branch.
  4. Saves the run with `add_run`, registers the `Worktree` and the `Run`, and spawns the runner.
  5. Returns the run `running`.

  A refusal before the save changes nothing. A save that fails rolls back the branch, as `new_session` does. The runner keeps the client's `env` and the plan's `cwd` in memory, for its child sessions.
- **Runner.** `run::advance(run, daemon, workflow, git, plan, env)`:
  1. Adds the worktree. On failure it calls `roll_back` and fails the run with git's error. The worktree record stays until the run is removed.
  2. For each step in order:
     - Marks the step `running`.
     - Creates and starts its child session through `Daemon::add_step_session(run, position, harness, env)`, which saves the session with `SessionWorktree::Step` and calls `Session::start`.
     - Waits on `events_after` for `AgentStarted`.
     - For each of `model`, `effort`, and `permission_mode` that the step sets, finds the session's config option in the `model`, `thought_level`, or `mode` category. It fails the run when the agent has no such option, sets the option when its current value differs, and fails the run with the agent's error when the agent refuses the value.
     - Prompts with the step's prompt as one text block and the input as a second text block, omitted when the input is empty.
     - Waits for `TurnEnded`. A turn that ends with `stop_reason: end_turn` and no error succeeds.
     - Records the turn's final reply as the step's `output`: the text of the last agent message the turn started, where a message is the agent message chunks and whole-message updates that share a message ID. That output is the next step's input.
  3. Marks the run `succeeded` after the last step.

  `AgentExited` before the start, or `SessionClosed` at any point, fails the run. `Session::prompt` returns the seq of its `PromptStarted` event, so the runner reads only that turn's events.
- **Cancelling.**
  - `run.cancel` refuses a run that is not `running`, and otherwise sets `cancel`.
  - The runner waits on `cancel` beside each wait on the session. When `cancel` is set, the runner cancels the active step's turn with `Session::cancel`, marks the step and the run `cancelled`, and ends. Later steps stay `pending`, and the child agent keeps running, idle.
  - A cancel that arrives while the worktree is being added takes effect once the add finishes.
- **Daemon lifetime and restart.**
  - `is_idle` also requires that no run is `running`.
  - `close_all` closes runs before sessions.
  - `Daemon::new` calls `interrupt_runs`, loads the runs before the sessions, and gives each run its `Arc<Worktree>`. A run is never resumed.
- **Removing a run.** `Daemon::delete_run(id, discard, env)` does the following:
  1. Refuses a `running` run with "the run is running".
  2. Holds each child session's lifecycle lock.
  3. Calls `worktree.remove(git, discard, stop)`, where `stop` stops every child's agent.
  4. Calls `apply_branch_rule` and `recompare`.
  5. Calls `store.remove_run`.
  6. Removes the run, its sessions, and its worktree from the registry, and closes them.

  It returns `Deleted { kept_branch }`. A refusal for unsaved work leaves everything as it was.
- **Deleting a child session.** `forget` calls `forget_session` on the run that lists the session, which clears the step's `session_id`. The store clears it through `ON DELETE SET NULL`.
- **oxctl.** A `run` subcommand group:
  - `run start --workspace <id> <workflow> <input>` prints the run ID.
  - `run wait <run>` follows `watch` until the run is not `running`, prints its `RunInfo` as JSON, and exits non-zero unless it `succeeded`. On a run that has ended, it prints at once.
  - `run cancel <run>`.
  - `run rm <run> [--discard]`.
- **Fake agent.** Its `mode` option gets the `mode` category, as Claude's has. No existing test reads its category.
- **Test fixture.** `crates/oxd/tests/runs.rs` starts `oxd` from a directory under `CARGO_TARGET_TMPDIR` that holds hard links to `oxd`, `ox-mcp`, and `oxd-fake-agent`, the last named `claude-code-acp` and `pi-acp`. The daemon then resolves both harnesses to the fake agent through the production path. The directory is created once per test binary.
- **UI.** `protocol.ts` mirrors `RunInfo`, `StepInfo`, and the two new `ListingChange` variants. `useWorkspaces` ignores them until task 4.

## Check your assumptions

1. **Where workflow definitions live.** Changed from `workflows/` beside the daemon socket to the config directory.
   - **Decided:** `$XDG_CONFIG_HOME/ox/workflows/<name>.yaml`, by default `~/.config/ox/workflows/<name>.yaml`.
   - **Consequences:** Definitions live apart from daemon state, and the installed app and a `make dev` daemon share them. A run is started by the file name, such as `plan-work-review`. The file's `name` is the display name. The integration tests and the smoke run set `XDG_CONFIG_HOME` for `oxd` to their own directories.
   - **Alternative:** `workflows/<name>.yaml` beside the daemon socket, by default `~/.local/share/ox/workflows/`.
   - **Trade-offs:** The config directory is the conventional home for hand-edited files, at the cost of a second path rule and a shared set across daemons. Beside the socket would need no new path rule and would isolate daemons, but would put hand-edited files among daemon state.
2. **What cancelling does to the active step.** Kept.
   - **Decided:** The runner sends `session/cancel` for the step's turn and marks the step and run `cancelled` at once. The child agent stays running and idle. The brief says the user can continue any child session after the run ends.
   - **Consequences:** The child session can be prompted right after a cancel. Its turn ends on the agent's own `idle`, which the run does not wait for.
   - **Alternative:** Stop the child's agent process, as delete does, and mark the run cancelled once it has exited.
   - **Trade-offs:** Cancelling the turn keeps the agent warm and matches the session's own Cancel. Stopping the agent guarantees nothing more runs in the worktree, but continuing the session then needs a load.
3. **How a step's input and output are framed.** Kept.
   - **Decided:** The step's prompt and its input are sent as two text blocks, prompt first, with no added wording. The output is the text of the turn's last agent message. Earlier messages, thoughts, and tool output are not passed on.
   - **Consequences:** A prompt says what its input is, as the brief's example prompts do ("the task in your input"). A step whose agent ends with a short sign-off passes only that sign-off.
   - **Alternative:** One text block that labels the input, such as `Input:` followed by the text. Or pass every agent message of the turn.
   - **Trade-offs:** Two plain blocks keep the daemon from adding words to the user's prompt, and keep the fake agent's scripts usable. A label is clearer to a model when the prompt does not mention its input. Passing every message keeps more context, but includes the agent's narration.
4. **How model, effort, and permission mode reach the agent.** Kept.
   - **Decided:** Each value is set on the config option in the ACP `model`, `thought_level`, or `mode` category. A missing option or a refused value fails the run with the reason. Claude's options are `model`, `effort`, and `mode`. Pi's are `model` and `thinking`. Pi has no `mode` option.
   - **Consequences:** Values are the bridges' own IDs, such as `claude-opus-5-5`, `xhigh`, `manual`, and `openai-codex/gpt-6.1-terra`. A Pi step with `permission_mode` fails at that step, not when the definition loads.
   - **Alternative:** Validate each value against the harness when the definition loads.
   - **Trade-offs:** Checking at the step needs no per-harness tables in `oxd` and follows whatever the bridges offer. Checking at load catches typos before any step runs, but duplicates the bridges' option lists.
5. **New dependency for YAML.** Kept.
   - **Decided:** Add `serde-saphyr` 2 to `oxd`. `serde_yaml` is deprecated, and `serde-saphyr` is maintained (2.1.1, November 2026) and supports anchors and aliases.
   - **Consequences:** One new crate and its parser in `oxd`'s build.
   - **Alternative:** `serde_yaml_ng` or `serde_norway`, forks of `serde_yaml` last released in 2024.
   - **Trade-offs:** `serde-saphyr` is actively maintained, but it is a newer codebase. The forks have `serde_yaml`'s behaviour, but little recent maintenance.
6. **How child sessions look in the desktop before task 4.** Kept.
   - **Decided:** Child sessions are ordinary sessions in the listing. The desktop shows them in their workspace's session list, with no grouping, until the run view in task 4.
   - **Consequences:** A run adds one session row per started step to the sidebar. Deleting or archiving one from the app leaves the run's worktree in place.
   - **Alternative:** Hide sessions that belong to a run from the desktop's list now.
   - **Trade-offs:** Showing them needs no UI change and lets the user answer a child's permission requests from the app today. Hiding them keeps the list tidy, but leaves those requests answerable only through `oxctl` until task 4.
7. **Live verification with real models.** Kept.
   - **Decided:** Add a `workflow` phase to `scripts/smoke-acp.py`. It runs a two-step definition on the agent under test, with the Make model and the lowest effort, and checks that the second step's output contains a token from the launch input. The Claude steps also set `permission_mode: acceptEdits`.
   - **Consequences:** `make smoke-claude` and `make smoke-pi` take two more short model turns each, and exercise bridge resolution beside `oxd` and real config option IDs.
   - **Alternative:** Rely on the fake-agent integration tests alone.
   - **Trade-offs:** The live phase catches option IDs or values the real bridges refuse, which the fake agent cannot show, at the cost of model time. The tests alone are free and repeatable, but prove only the daemon's side.

## Test plan

- `crates/oxd/tests/runs.rs`, using the hard-linked fixture, a temporary repository workspace, and definitions written into `<temp>/ox/workflows`, with `XDG_CONFIG_HOME` set to `<temp>` for `oxd`:
  - **Handoff.** The launch input reaches step 1 and step 1's final reply reaches step 2. Step 1's prompt is `messages 1 "earlier"` followed by `text "text \""`, and the launch input is `text "handed-off\""`. Step 1's output is then `text "handed-off"`, and step 2's output, with the prompt `text "second "`, is `second handed-off`. Both child sessions run in the run's worktree. Each step's session reports the model, effort, and mode the definition set. The run ends `succeeded`.
  - **Cancel.** A run whose step 1 prompt is `wait` ends `cancelled` after `run.cancel`. Step 1 is `cancelled` and its session's turn ends `cancelled`. Step 2 is `pending` with no session, and the worktree remains.
  - **Client disconnect.** A run started by a client that then disconnects runs to `succeeded`, observed through a second client. Step 1 takes longer than the daemon's idle exit.
  - **Restart.** A succeeded run lists the same steps, outputs, and session IDs after a restart, and a child session's history is readable. A run whose step is `wait` is `interrupted` after a restart, with that step `interrupted`.
  - **Removal.** `run.delete` refuses a running run. With an untracked file in the worktree, it refuses with `UNSAVED_WORK` and changes nothing. With `discard`, it removes the worktree directory, the `ox/` branch, the child sessions, and the run, and a watch reports `RunRemoved`, `SessionRemoved`, and `WorktreeRemoved`.
  - **Child deletion.** Deleting step 1's session after the run keeps the worktree directory and step 2's session, and step 2's session loads. The run lists step 1 with no session.
  - **Failures.** An unknown workflow is `NOT_FOUND`. A definition with an unknown field is `INVALID_REQUEST`. A step whose turn ends with `fail boom` fails the run with that error and leaves the later steps `pending`. A step whose effort the agent refuses fails the run with the agent's error.
- `workflow.rs` unit tests: a two-step definition that uses an anchor and alias parses, and a definition with no steps, an unknown field, or a path in its name is refused.
- `store.rs` unit tests:
  - A run round-trips with its steps.
  - `remove_session` of a child keeps the run's worktree row.
  - `remove_run` removes the run's sessions, steps, and worktree.
  - `interrupt_runs` marks only running runs and steps.
  - `workspace_has_sessions` counts a run with no sessions.
- `messages.rs`: serialization tests for `run.start`, `run.cancel`, and `run.delete`, as for the other methods.
- `crates/ox`: `finds_executable_harnesses_on_path` uses `Harness`.
- The live `workflow` smoke phase described above, and `make e2e` for the desktop core's change.

## Implementation plan

1. `ox-protocol`: add `config_dir()` with a unit test like `data_dir`'s, `Harness`, `RunInfo`, `StepInfo`, `RunState`, `StepState`, the three methods, `Listing.runs`, and the two `ListingChange` variants. Update the `Listing` and `ListingChange` docs to cover runs and their order. Add the client wrappers and serialization tests.
2. `crates/ox/src/lib.rs`: replace `Agent` with `Harness`.
3. `oxd-fake-agent`: give `mode` the `mode` category, and update its module doc.
4. `store.rs`: add the tables, `SavedRun`, `SavedStep`, `SessionWorktree`, and the run methods. Change `add_session`, `remove_session`, and `workspace_has_sessions`. Add the unit tests.
5. `workflow.rs`: add the definition types, `load`, and the unit tests. Add `serde-saphyr` to `crates/oxd/Cargo.toml`.
6. `session.rs`: `prompt` returns its `PromptStarted` seq.
7. `run.rs`: add `Run` and `advance`.
8. `daemon.rs`, `conn.rs`, `main.rs`:
   - Registry and `Sent`: add `runs`.
   - `list` and `listing_changes`: include runs in the order Decisions gives.
   - Add `workflows_dir`, `owned_worktree`, `start_run`, `add_step_session`, `cancel_run`, and `delete_run`, and handle the three methods.
   - Apply `owned_worktree` in `delete_session`, `archive_session`, `unarchive_session`, `forget`, and `Daemon::new`.
   - Restore runs, and change `is_idle` and `close_all`.
   - Spawn `run.start` and `run.delete` in `conn.rs`.
   - Pass `config_dir().join("workflows")` from `main.rs`, and update its module doc.
9. `oxctl`: add the `run` subcommands.
10. `crates/oxd/tests/runs.rs`: add the fixture and tests.
11. `ui/src/lib/protocol.ts`: mirror the run types and listing changes.
12. `scripts/smoke-acp.py`: add the `workflow` phase. It uses its own Git repository workspace and a definition in `<artifacts>/config/ox/workflows`, with `XDG_CONFIG_HOME` set to `<artifacts>/config` in the environment that starts `oxd`, and removes the run with `--discard`.
13. Run `make check`, `make e2e`, `make smoke-claude`, and `make smoke-pi`.

## Documentation updates

- New `docs/workflows.md`, listed in `AGENTS.md`'s project documents:
  - where definitions live (`$XDG_CONFIG_HOME/ox/workflows/`, by default `~/.config/ox/workflows/`) and how they are named
  - the fields an agent step takes, and how model, effort, and permission mode reach the harness
  - how input and output pass between steps
  - what cancelling, removing a run, and removing a child session do
  - that a run interrupted by a daemon restart keeps its history and does not resume
- `docs/dev/architecture.md`:
  - A "Workflow runs" section: a run owns its worktree and child sessions, the daemon's runner advances steps on turn completion, steps launch bridges found beside `oxd`, a running run keeps the daemon alive, and a restart marks running runs interrupted.
  - Under "Persistence and recovery", the database also holds runs and their steps.
  - Under "Repository ownership", a worktree belongs to a session or a run.
- `docs/dev/glossary.md`:
  - Add **Workflow**, **Run**, **Step**, and **Child session**.
  - **Worktree** is created by Ox for a session or a run.
- `docs/worktrees.md`: under "Deleting and archiving", removing a run removes its worktree under the same rules, and deleting or archiving a run's child session leaves the run's worktree.
- `crates/oxctl/src/main.rs`: the about text and command docs cover runs.
