# Add the desktop run view

## Goal

Deliver task 4 of the brief's delivery sequence on top of the runs of `.agents/plans/2026-11-15-002-workflow-runs.md` and `.agents/plans/2026-11-15-003-workflow-shell-steps-and-checks.md`. Each workspace's New Task menu lists the workflows and starts a run with an input. The sidebar shows each run as a row with its child sessions nested beneath it on tree lines. Selecting a run opens the run view: a timeline of each started step's checks, commands, and child conversation, where the user answers the children's permission requests and forms. Selecting a child opens its ordinary session view, which takes over sending prompts once the run has ended.

**Brief:** `.agents/briefs/2026-11-14-001-user-defined-workflows.md`

## Related code

- `ui/src/App.tsx`: holds the viewed session ID in `sessionStorage`, and renders `SessionList`, `FileTree`, `SessionContent` with `Conversation` and `Composer`, and `MergeButton`.
- `ui/src/features/sessions/SessionList.tsx`: workspace sections, the New Task popover menu, `SessionRow` with its status dot, branch status and native context menu, and the Archived group.
- `ui/src/features/sessions/useWorkspaces.ts`: `apply` ignores `run` and `run_removed` changes today.
- `ui/src/features/sessions/useSessionActions.tsx`: delete, archive and unarchive with the unsaved-work and kept-branch dialogs, and the `busy` set.
- `ui/src/features/conversation/useConversation.ts`: one session's items, viewed through `view_session`.
- `ui/src/features/conversation/Conversation.tsx`: the scroll container that follows the end, `ItemView`, and `ToolCall` with `ToolStatus`.
- `ui/src/features/composer/Composer.tsx`, `Permission.tsx`, `Elicitation.tsx`: the request panels in the composer's `.stack`, and the entry with Send and Stop.
- `ui/src/features/files/`: `FileTree`, `FileViewer`, `SessionContent`, `treeState.ts`, `viewerState.ts` and `useFileViewers.ts` key their state by session ID and root it at the session's `cwd`.
- `ui/src/components/Dialog.tsx`: the modal dialog. `.text-field` is the text area style the elicitation form uses.
- `ui/src/styles/controls.css`: `.status` dots: grey by default, `running` green and breathing, `awaiting_*` and `attention` amber, `failed` red, `exited` hollow. `sessions.css` dims an exited session's row.
- `ui/src/styles/sidebar.css` and `tokens.css`: `.sidebar-row`, `--sidebar-row-height`, `--sidebar-row-padding`, `--icon-size`, `--status-dot-size`, `--tree-indent`.
- `ui/src/lib/protocol.ts`: mirrors `RunInfo`, `StepInfo`, `CommandInfo` and the run listing changes.
- `crates/ox/src/lib.rs`: Tauri commands. `App.view` holds one conversation view, which `view_session` replaces. `watch` clears the directory and file watches, which are keyed by channel ID. `deletion` maps a `Deleted` answer.
- `crates/oxd/src/workflow.rs`: `load` and `parse`. `crates/oxd/src/daemon.rs`: `workflows_dir`, method dispatch, `new_session`, `add_session`, `add_step_session`, `start_run`.
- `crates/oxd/src/run.rs`: the runner sends a step's prompt, and a repair prompt only after a postcondition fails, then runs every postcondition again.
- `crates/oxd/src/store.rs`: the `sessions` and `runs` tables, `SavedSession`, `SavedRun`, `add_session`, `add_run`, `sessions`, `runs`. The store has no migrations.
- `crates/ox-protocol/src/messages.rs` and `client.rs`: `Method`, `SessionInfo`, `RunInfo`, client wrappers.
- `crates/oxctl/src/main.rs`: `RunCommand` has `start`, `wait`, `cancel` and `rm`.
- `crates/claude-code-acp/src/bridge.rs` and `crates/pi-acp/src/bridge.rs`: both title a session with its first prompt's text, so a child session's title is its step's prompt.
- `~/src/hypercube/internal/ui/web/sidebar/session-item.tsx`: `Branch`, the tree-line pattern to copy, and its hollow child dots; `session-tree.tsx` lists the rows flat.
- `ui/e2e/wdio.conf.ts`, `ui/e2e/helpers.ts` (`resetDaemon`, `stopDaemon`, `openMenu`), `ui/e2e/specs/archive.e2e.ts` (`sessionAction`, which answers a native context menu through Tauri's menu IPC), and `crates/oxd/tests/runs.rs` (`bin_dir`, which hard-links the fake agent as the bridges).

## Decisions

- **Listing workflows.** `workflow::list(dir) -> Vec<WorkflowInfo>` reads every `*.yaml` file in the workflows directory whose name does not start with `.`, sorted by file name. A missing directory lists nothing. `ox-protocol` gains `WorkflowInfo { workflow, name, error: Option<String> }`: `workflow` is the file name without `.yaml`; `name` is the definition's `name`, or `workflow` when the file does not parse; `error` is the parse message, as `load` gives it. New method `workflow.list` (`Method::WorkflowList`) answers the list, with `Client::list_workflows`.
- **Creation times.** `SessionInfo` and `RunInfo` gain `created_at: u64`, milliseconds since the Unix epoch, set when the daemon creates the session or the run and saved in a new `created_at INTEGER NOT NULL` column of `sessions` and `runs`. `SavedSession` and `SavedRun` carry it. The sidebar orders a workspace's runs and sessions together by it.
- **Desktop commands.** `crates/ox/src/lib.rs` gains:
  - `workflows() -> Vec<WorkflowInfo>`
  - `start_run(workspace_id, workflow, input) -> RunInfo`, with `utf8_env()`
  - `cancel_run(run_id)`
  - `delete_run(run_id, discard) -> Deletion`, through `deletion`, with `utf8_env()`

  `App.view` becomes `views: Mutex<HashMap<u32, Task>>`, keyed by channel ID like `directories`. `view_session(session_id, channel)` adds a view, `unview_session(channel_id)` ends one, and `watch` clears them all as it clears the directory and file watches.
- **Conversations.** `useConversations(sessionIds, connection)` replaces `useConversation`. It holds each session's items by key, views each session on its own channel, ends a session's view with `unview_session` when its ID leaves the list, and returns a `Map` from session ID to `Item[]`. A batch for one session copies only that session's map, and the other sessions' arrays keep their identity. The session view calls it with the one viewed session.
- **Child sessions.** A session is a child of the run whose `worktree_id` equals its own, the rule the daemon's `owned_worktree` uses. The sidebar, the run view and the composer find a child's run this way, and a child's step is the step whose `session_id` is the child's ID.
- **Sidebar.**
  - A workspace lists its runs and its sessions that are not children, ordered by `created_at`. Archived sessions that are not children stay in the Archived group. "Remove Workspace" is enabled only when the workspace has no sessions and no runs.
  - New `RunRow`: a filled status dot, the run title, and a native context menu with "Cancel Run" (enabled while the run is running) and "Delete Run" (enabled once it has ended). Clicking it views the run.
  - **Run title**: `runTitle(run)` is `run.input` with runs of whitespace collapsed to one space, or `run.name` when the input is blank. The row truncates it with an ellipsis like a session title.
  - The run's children follow its row as `SessionRow`s, in `created_at` order, archived or not. A child row has a hollow status dot, its step's name as its title, and the session's context menu. It renders a `tree-branch` span before its dot, copying hypercube's `Branch`: a vertical line on the centre line of the run's dot, starting just below that dot for the first child, and a stroke across to the child's status box; the last child's line turns a corner with `--radius-sm`. The branch span is `--icon-size` wide, so a child's dot lines up with the start of the run's title. Lines use `--border` and `--border-width`. Child titles use `--text-xs`, as hypercube's level-2 titles do. An exited child dims its contents but not its tree line.
  - The rows stay one flat list of `li`s, as hypercube's tree is flat.
- **Status dots.** Colour gives the state and fill gives the level: a run's dot is filled and a child's is hollow. The colours reuse the session dot classes:

  | Class | Colour | Run | Child |
  | --- | --- | --- | --- |
  | `attention` | amber | a child awaits permission or input | its session awaits permission or input |
  | `running` | green, breathing | running | its step is running, or its session is in a turn |
  | `succeeded` | green | succeeded | its step succeeded |
  | `failed` | red | failed | its step failed |
  | none | grey | cancelled or interrupted | its step was cancelled or interrupted |

  The rows go top to bottom; the first that applies wins. `controls.css` gains `.status.succeeded` and `.status.hollow`, which shares `.status.exited`'s transparent fill. Under reduced motion, `.status.running` keeps `--busy-dot-opacity`, so a running dot still differs from a succeeded one. A row is dimmed by an `exited` class from its session's state instead of `:has(.status.exited)`, because a child's dot no longer carries the session state. `runStatus(run, children)` and `childStatus(step, session)` return the class and its label for the dot's `aria-label` and `title`.
- **Starting a run.** For a workspace in a Git working tree, the New Task menu lists the workflows below New Task and Session in Checkout, after a `.menu-separator`. It reads them with `workflows` each time the menu opens, from the popover's `toggle` event. Each item is the workflow's `name`; one with an `error` is disabled, with the error as its `title`. Choosing one opens `StartRunDialog`: a `Dialog` titled with the workflow's name, a `.text-field` text area for the input, and Cancel and Run buttons. Command-Return also runs. Run calls `start_run` and views the new run.
- **Viewing.** `viewed` holds a session ID or a run ID. `App` looks the ID up among the runs, then the sessions.
  - A run renders `RunView` inside `SessionContent`, with the "Cancel Run" button (lucide `Square`) in the content header's actions while it runs.
  - The right sidebar shows the run's worktree, rooted at the worktree's path.
  - The file tree, file viewer and `SessionContent` key their state by the viewed ID; their `sessionId` props become `viewId`. `useFileViewers` takes `{ id, cwd }` for each session and each run, and `forgetDeletedTrees` and `forgetDeletedViewers` keep the IDs of both.
- **Run view.** `RunView` calls `useConversations` with the children's IDs in step order. It lists only the steps that have started, in order, as `StepSection`s inside the same following scroll container as the session view, extracted from `Conversation` as `Transcript`. A `StepSection` is memoized on its step and its items. It holds a heading with the step's hollow dot and name, then `stepTimeline(step, items)`:
  1. the preconditions,
  2. a shell step's command,
  3. the child's first turn: its items before the second `prompt` item,
  4. the postconditions up to and including the first that failed, or all of them,
  5. when one failed, the repair turn up to the third `prompt` item, then the remaining postconditions,
  6. the child's later turns.

  This follows the runner, which prompts a step once and repairs it once after a failed postcondition. `ItemView` renders the items. `CommandView` renders a command as a tool call: a status (running dot, `Check`, `X`, or grey dot for cancelled and interrupted), the heading "Precondition", "Command" or "Postcondition" with the command's first line as a code span, and details holding the whole command, `Exited with status <n>` when there is an exit code, and the output. The output's label reads "Output (last 64 KiB)" when `truncated`. After the steps, a failed run shows its `error` as a `turn-error` paragraph, and an interrupted run shows "The run was interrupted when oxd stopped" as an `agent-exited` paragraph.
- **Requests in the run view.** Below the timeline, `RunView` shows the oldest open permission request or form among the children, in step order, in a `.composer` `.stack` without the entry, using `Permission` and `Elicitation` with that child's session ID. Answering in either view resolves the request in both, because both follow the daemon's `request_resolved`.
- **Composer of a child.** While the child's run is running, the composer's text entry and Attach Files are disabled and its Stop button calls `cancel_run`. The config pickers stay enabled. Once the run has ended, the composer works as for any session, and an exited child loads when selected, as today.
- **Deleting a run.** `useSessionActions` gains `cancelRun(runId)` and `deleteRun(runId, discard)`, which share `busy` and the dialogs: an `unsaved_work` answer asks "Discard and Delete", and a kept branch shows "Branch kept".
- **oxctl.** `run ls` prints each run as `<id>\t<state>\t<name>`, from the first message of a watch.
- **End-to-end fixture.** `wdio.conf.ts`'s `onPrepare` hard-links `ox`, `oxd`, `ox-mcp` and `claude-code-acp` from `target/debug` into `target/debug/e2e-bin/`, with `oxd-fake-agent` as `pi-acp`, and launches the app from there. Every daemon of the run, including those the app restarts, then runs `pi` steps with the fake agent, while desktop sessions keep the real Claude bridge. Test workflows use `harness: pi`. `XDG_CONFIG_HOME` is `<OX_E2E_DIR>/config`, and `defineWorkflow(name, yaml)` writes `config/ox/workflows/<name>.yaml`. `resetDaemon` removes every run with `run rm --discard`, cancelling and waiting for a running one first, before deleting sessions. `sessionAction` moves from `archive.e2e.ts` to `helpers.ts` as `rowAction(row, text)`.

## Check your assumptions

1. **Where runs sit among sessions in a workspace.** Kept.
   - **Decided:** Runs and top-level sessions form one list ordered by creation, using a new `created_at` on `SessionInfo` and `RunInfo`. Sessions and runs are numbered in separate tables and carry no time today, so nothing else orders them against each other.
   - **Consequences:** A new run appears at the bottom, like a new session. The `sessions` and `runs` tables gain a column, and because the store has no migrations, existing databases must be deleted, losing their sessions, runs and conversations.
   - **Alternative:** List a workspace's runs above its sessions, with no protocol or schema change.
   - **Trade-offs:** Creation order keeps the newest work in one predictable place at the cost of a database reset and a field on two types. Grouping keeps existing databases but puts a new run or a new session in the middle of the list.
2. **What the auto-generated run title is made from.** Kept.
   - **Decided:** The run's input with whitespace collapsed, falling back to the workflow's name for an empty input. Both bridges title a session from its first prompt's text, so this matches how sessions are titled today, and a child session's own title is its step's prompt, which names no task.
   - **Consequences:** The title is available as soon as the run starts. A long input shows its first words, cut off by the row's ellipsis.
   - **Alternative:** Have an agent write the title: the first agent step's agent calls the `ox` MCP `set_title`, which also titles the run, or a separate low-cost model turn summarises the input.
   - **Trade-offs:** The input costs nothing and never fails, but reads poorly for inputs that are not task summaries, such as a pasted brief path. An agent's title reads better, but needs a model turn, arrives later, and depends on the agent choosing to set it.
3. **What a child session's composer allows during a run.** Kept.
   - **Decided:** While the run is running, the child's text entry and attachments are disabled, and Stop cancels the run. The brief leaves free-form messages during a run out of scope, and stopping only the child's turn would end the run `failed` instead of `cancelled`.
   - **Consequences:** The user cannot prompt a child until its run ends. Stop in a child's composer cancels the whole run, including later steps. Model and mode pickers stay usable.
   - **Alternative:** Leave the composer as it is for children, so the user can prompt a child between turns and Stop cancels only the turn.
   - **Trade-offs:** Disabling keeps the runner's turn tracking intact and makes Stop mean one thing. Leaving it open lets the user steer an agent mid-run, but a prompt sent between the runner's turns would be taken as the step's reply, and Stop would fail the run.
4. **Which steps the run view lists.** Kept.
   - **Decided:** Only steps that have started, the same rule the user chose for the sidebar.
   - **Consequences:** The run view does not show which steps come next. A run that fails a precondition shows that step alone.
   - **Alternative:** List every step, with pending steps as grey headings.
   - **Trade-offs:** Started steps stay true once workflows gain loops or branches, where the next step is not known. Listing every step shows progress through a linear workflow, but would have to change for branching ones.
5. **What a child row is called.** Kept.
   - **Decided:** The step's name, such as "Plan" or "Review". A child's session title is its step's prompt text, set by its bridge.
   - **Consequences:** Child rows read as the workflow's steps. An agent's own `set_title` call does not show in the sidebar for a child.
   - **Alternative:** The session's title, falling back to the step's name.
   - **Trade-offs:** The step's name is short and always meaningful. The session title follows any title an agent chooses, but usually repeats the step's prompt.
6. **How end-to-end tests run workflow steps.** Kept.
   - **Decided:** The e2e app runs from `target/debug/e2e-bin/`, where `pi-acp` is the fake agent, and test workflows use the `pi` harness. The daemon finds a step's bridge beside its own executable, and the app restarts the daemon from beside itself after every disconnect, so restarting the daemon from a spec would race the app.
   - **Consequences:** Every spec runs the app from that directory. A desktop Pi session in e2e would run the fake agent; no spec starts one today.
   - **Alternative:** Give `oxd` a setting for the bridges' directory that tests set.
   - **Trade-offs:** Hard links change only the test fixture, as `runs.rs` already does. A setting is explicit, but adds a production option that only tests use.

## Test plan

- `workflow.rs` unit test: `list` returns definitions sorted by file name, with a parsing definition's `name`, and a broken one's file name and error. It skips other extensions and names starting with `.`, and a missing directory lists nothing.
- `crates/oxd/tests/runs.rs`: `workflow.list` through the client lists a defined workflow from the daemon's workflows directory.
- `store.rs`: the existing session and run round-trip tests include `created_at`.
- `messages.rs`: a serialization test for `workflow.list`, as for the other methods, and the `RunInfo` and `SessionInfo` round-trips include `created_at`.
- `ui/e2e/specs/run.e2e.ts`, with `defineWorkflow`, `rowAction`, and the fake agent:
  - **Start and follow.** With a session started before, a workflow of an agent step `First`, a shell step, and an agent step `Second` with a passing postcondition is chosen from the New Task menu and run with an input. The new run is viewed, and its row sits between that session and one started after. Its title is the input. `First` and `Second` appear under it as child rows with tree branches as each starts, and the shell step has no row. The run view shows each step's heading, its prompt, the shell command and the postcondition. When the run ends, the run's dot is `succeeded` and filled, and the children's dots are hollow. Screenshots of the sidebar and the run view go to `OX_E2E_DIR`.
  - **A request answered from either view.** A run whose two steps each run `permission`: the first is answered in the run view and its child's timeline shows `allowed`. The second is answered in its child's session view, and the run view drops the request and shows `allowed`.
  - **Repair order.** A step whose postcondition fails once and whose repair turn fixes it shows, in order: its prompt, the failed postcondition, the repair prompt, and the passing postcondition.
  - **During and after the run.** While a step runs `wait`, its child's entry is disabled, and Stop ends the run `cancelled`. Afterwards, the child's prompt is sent from its composer, and the reply shows in the session view and, after the step's checks, in the run view. "Delete Run" removes the run row and its child rows.
  - **Failed precondition.** A run whose first step's precondition fails has no child rows, and the run view shows the failed precondition with its output and the run's error.
- `make check` and `make e2e`. The `make e2e` stress spec ("streams a long reply into a long transcript without stalling") fails before this change and is not part of it.

## Implementation plan

1. `ox-protocol`: add `WorkflowInfo`, `Method::WorkflowList`, `Client::list_workflows`, and `created_at` on `SessionInfo` and `RunInfo`, with their docs and serialization tests.
2. `store.rs`: add the `created_at` columns, carry them in `SavedSession` and `SavedRun`, and extend the round-trip tests.
3. `oxd`: add `workflow::list` and its unit test. Set `created_at` in `new_session`, `add_step_session` and `start_run`, and give it to `Session` and `Run` for their infos. Answer `workflow.list` in `daemon.rs`. Add the `runs.rs` test.
4. `oxctl`: add `run ls`.
5. `crates/ox/src/lib.rs`: add `workflows`, `start_run`, `cancel_run`, `delete_run` and `unview_session`, key `views` by channel, and clear them in `watch`.
6. `ui/src/lib/protocol.ts`: mirror `WorkflowInfo` and `created_at`.
7. `useWorkspaces`: keep `runs`, applying `run` and `run_removed` like sessions.
8. `useConversations`: replace `useConversation`. Extract `Transcript` from `Conversation` and export `ItemView`.
9. Files: rename `sessionId` to `viewId` in `FileTree`, `FileViewer`, `SessionContent`, and the tree and viewer state, and give `useFileViewers` and the forget functions the runs' IDs and roots.
10. Styles: `.status.succeeded`, `.status.hollow`, the reduced-motion opacity, the `exited` row class, `.tree-branch`, child title size, and `.menu-separator`, with any new sizes as tokens in `tokens.css`.
11. `SessionList`: the ordered list of runs and sessions, `RunRow`, child rows, `runTitle`, `runStatus`, `childStatus`, the workflows in the New Task menu, and `StartRunDialog`.
12. `useSessionActions`: `cancelRun` and `deleteRun`.
13. `features/runs/RunView.tsx`: `RunView`, `StepSection`, `CommandView`, `stepTimeline`, and the request panel. `App.tsx`: view runs, the Cancel Run header button, and the run's file tree.
14. `Composer`: take the child's run and apply the composer rule in Decisions.
15. E2E: the `e2e-bin` fixture and `XDG_CONFIG_HOME` in `wdio.conf.ts`; `defineWorkflow`, `rowAction` and the run removal in `resetDaemon` in `helpers.ts`; `archive.e2e.ts` uses `rowAction`; add `run.e2e.ts`.
16. Delete the development database, then run `make check` and `make e2e`. Compare the sidebar and run view screenshots with hypercube's screenshot and with the session rows and conversation for size, colour, spacing and alignment.

## Documentation updates

- `docs/workflows.md`, "Running a workflow": starting a run from a workspace's New Task menu with an input; the run's row, titled with its input, and its child sessions nested beneath it as their steps start; the run view's timeline of started steps with their checks, commands and conversations; answering a child's requests from either view; Cancel Run and Delete Run; a child's composer, which sends prompts once the run has ended. The opening of "Defining a workflow" says the desktop lists every definition, and a definition that does not parse with its error.
- `docs/dev/architecture.md`, "Desktop and command-line clients": the core keeps one conversation view per channel, so the run view follows every child at once. "Workflow runs": a session is a child of the run that owns its worktree.
- `docs/dev/performance.md`: "Desktop core" covers each viewed conversation; "UI" says the run view keeps each child's items and the other steps' sections unchanged when one child's conversation changes.
- `docs/dev/glossary.md`: add **Run view**, the desktop view of a run: the timeline of its started steps' commands and child conversations.
- `docs/dev/testing.md`: `make e2e` runs the app from `target/debug/e2e-bin/`, where `pi-acp` is the fake agent, so test workflows run `pi` steps with it, and its workflows directory is under `OX_E2E_DIR`.
- `crates/oxctl/src/main.rs`: `run ls` in the command docs.
