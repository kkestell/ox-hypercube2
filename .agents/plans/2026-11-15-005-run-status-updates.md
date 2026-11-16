# Step start times and agent status updates

## Goal

The run view in `ui/src/features/runs/` reads three things from `statusUpdates.mock.ts`: each step's status updates, its start time, and its duration. Replace the first two with data from `oxd` and drop the third. Each step gets a start time, set when it leaves `pending`. A child session's agent gets an `ox` MCP tool that posts a status update: a sentence or two on what it is doing and what it plans to do next. Each update is timestamped and saved in the child's history. The run view shows a step's updates, newest last, from that child's conversation, and shows the step's start time as `HH:MM`.

## Related code

- `crates/ox-protocol/src/messages.rs`: `Method`, `StepInfo`, `Event`, `EventKind`. `Event` has only `session_id`, `seq` and `kind`.
- `crates/ox-protocol/src/client.rs`: `Client::set_title`, the model for a new acknowledged session method.
- `crates/oxd/src/store.rs`: the `run_steps` table, `add_run`, `runs`, `set_step`, and the `time` and `time_value` helpers. `events` saves each `EventKind` as JSON, so a new event kind needs no schema change.
- `crates/oxd/src/run.rs`: `advance` moves a step out of `pending` in one place, `run.set_step(position, StepState::Running, None)`. `Run::set_step` saves and records the state.
- `crates/oxd/src/daemon.rs`: `now()` (private), `handle` dispatch for `SessionSetTitle`, `Daemon::new` restoring sessions next to the runs that own their worktrees, `new_session` and `add_step_session` calling `Session::create`.
- `crates/oxd/src/session.rs`: `Session::set_title` and `prompt`, which change the state through `update`; `Session::run` passes `ox_server.for_session(&id)` to agents that advertise `session.mcp.stdio`.
- `crates/oxd/src/state.rs`: `Request` and `State::request`, which records a client's request as an event while the agent is `Live` and refuses it otherwise; `fill` ignores event kinds it does not track.
- `crates/oxd/src/mcp.rs`: `OxServer::for_session` builds `ox-mcp --socket <path> --session <id>`.
- `crates/ox-mcp/src/main.rs`: `set_title` under `#[tool_router(server_handler)]`; each call opens a `Client`.
- `crates/oxd/src/bin/oxd-fake-agent.rs`: the `mcp-title` directive and `call_set_title`, which speak MCP to the session's `ox` server.
- `crates/pi-acp/src/bridge.rs`: refuses `session/new` and `session/resume` with MCP servers. `docs/dev/acp-v2.md` says Pi sessions have no `ox` server.
- `crates/ox/src/conversation.rs`: `Conversation::apply` folds each event into the `Item`s the UI renders, keyed by seq.
- `ui/src/features/conversation/useConversations.ts`: inserts items by key in arrival order. `RunView` already views every child through it.
- `ui/src/features/conversation/Conversation.tsx`: `ItemView` switches on every `Item` type.
- `ui/src/features/runs/RunStatus.tsx`: `RunStatus` and `StepStatus` call `mockStepStart` and `mockStatusUpdates`. `RunHeader.tsx` reads `run.created_at`, `run.name`, and the worktree's `branch`, `compare` and `comparison`; `RunView.tsx` reads `run.error` through `runNotice`. All of these come from `oxd` already.
- `ui/src/lib/protocol.ts`: mirrors `StepInfo` and `Item`.
- `crates/oxd/tests/runs.rs`, `crates/oxd/tests/daemon.rs` (`agent_sets_title_through_ox_mcp_server`), `ui/e2e/specs/run.e2e.ts`.

## Decisions

- **Start time.** `StepInfo` gains `started_at: Option<u64>`, milliseconds since the Unix epoch, unset while the step is `pending`. `run_steps` gains `started_at INTEGER`. `Store::start_step(run_id, position, started_at)` sets the step `running` and saves the time. `Run::start_step(position)` saves and records both, and replaces the `set_step(position, StepState::Running, None)` call in `advance`. `Run::set_step` then only records success. `now()` in `daemon.rs` becomes `pub fn now()` for `run.rs` and `session.rs`. No finish time is stored: the run view shows an interrupted or failed step through `runNotice`.
- **Status updates are session events.** `EventKind::StatusUpdate { text: String, posted_at: u64 }`. The update belongs to the child session that posted it, so its step is the step whose `session_id` is that session. The timestamp is carried in the event kind, so `Event` and the `events` table stay as they are. Updates are saved, replayed, delivered to subscribers, and deleted with the session's other events.
- **Daemon method.** `session.post_status_update { session_id, text }` (`Method::SessionPostStatusUpdate`), with `Client::post_status_update`. `Session::post_status_update(text)` trims the text, refuses an empty one with `invalid_request`, and records `Request::StatusUpdate { text, posted_at: now() }` through `update`. `State::request` records it as `EventKind::StatusUpdate` while the agent is `Live`, in a turn or not, and refuses it with `invalid_state` otherwise, as it refuses a prompt. Answers `null`.
- **Offering the tool to every session.** Every session's `ox` server offers `post_status_update`. `OxServer::for_session(session_id)` needs no extra argument. The run view reads updates only from child sessions, so ordinary sessions' updates are recorded but not shown.
- **The tool.** `Ox` holds a `tool_router: ToolRouter<Ox>` field and uses `#[tool_router]` on its impl with `#[tool_handler(router = self.tool_router)] impl ServerHandler for Ox {}`. The tool is always registered. Tool `post_status_update`, parameter `text`, description: "Post a status update on the workflow step you are running, shown to the user in the run view: a sentence or two on what you are doing now and what you will do next. Post one when you start the step and each time you move on to a new part of it." Success returns `Status update posted.`; a daemon error returns a tool error with its message. Claude's bridge already allows every `mcp__ox` tool without a permission request.
- **Item.** `crates/ox/src/conversation.rs` gains `Item::StatusUpdate { key, text, posted_at }`, keyed by the event's seq, so a conversation lists a step's updates in the order they were posted. `protocol.ts` mirrors it as `{ type: "status_update"; key: string; text: string; posted_at: number }`. `ItemView` renders nothing for it: the session view already shows the `post_status_update` tool call with its text.
- **Run view wiring.** `RunView` passes `conversations` to `RunStatus`. `StepStatus` takes `updates`, the `status_update` items of its child's conversation, and `started`, `step.started_at`. The `now` prop and `statusUpdates.mock.ts` are deleted; the list keys updates by `key` and shows `posted_at` with `clock`. `RunHeader.tsx` is unchanged. `RunView.tsx` changes only to pass `conversations`, and `RunStatus.tsx` only its data sources and types.
- **Fake agent.** New directive `mcp-status <text>` calls the `ox` server's `post_status_update` with the rest of the line and sends the result's text, or the error, as one chunk, like `mcp-title`. `call_set_title` becomes `call_tool(server, name, arguments)`, shared by both.

## Check your assumptions

1. **Which harnesses can post status updates.**
   - **Decided (kept):** Claude steps only. `pi-acp` refuses sessions with MCP servers, and `oxd` gives the `ox` server only to agents that advertise `session.mcp.stdio`.
   - **Consequences:** A Pi step's section in the run view never shows updates, only its heading, start time, and the run's error.
   - **Alternative:** Give Pi the tool too, through a Pi extension that `pi-acp` loads and that calls `session.post_status_update`, or by passing the `ox` server to Pi's built-in MCP support.
   - **Trade-offs:** Claude only needs no bridge work. Covering Pi makes the run view the same for every step, but adds a tool path to `pi-acp` and a new `make smoke-pi` check.
2. **Which sessions get the tool.**
   - **Decided (changed):** Every session gets the tool. Ordinary sessions' updates are recorded, and the run view shows only child sessions' updates for now.
   - **Consequences:** Agents in ordinary sessions see the tool and may post updates that no view shows yet. No `child` flag is needed on `Session` or `ox-mcp`.
   - **Alternative:** Give only child sessions the tool, which needs a `child` flag from the daemon to `ox-mcp`.
   - **Trade-offs:** Every session needs no flag and keeps the tool available when ordinary sessions gain a view. Child-only keeps ordinary conversations free of status calls.
3. **What makes an agent post updates.**
   - **Decided (kept):** The tool's description asks the agent to post one when it starts the step and each time it moves on to a new part. The step's prompt is sent unchanged.
   - **Consequences:** How often updates appear depends on the model following the description; a step can finish with none.
   - **Alternative:** The runner adds an instruction to post status updates to each agent step's prompt.
   - **Trade-offs:** The description keeps prompts exactly as the workflow defines them. A prompt instruction makes updates more reliable, but adds text the workflow author did not write and shows it in the child's conversation.
4. **Where updates are stored.**
   - **Decided (kept):** As events in the child session's history, which the run view already follows for every child.
   - **Consequences:** Updates last as long as the child session. Deleting a child session removes its step's updates from the run view, as it already unsets the step's `session_id`. The run view reads them from each child's full conversation.
   - **Alternative:** A `run_status_updates` table keyed by run and step, sent in `StepInfo`.
   - **Trade-offs:** Events need no table, no listing change, and no new delivery path. A table keeps updates after a child is deleted and lets the run view show them without following the children, but adds storage and sends every update to every watch.

## Test plan

- `store.rs`: `a_run_round_trips_with_its_steps` starts a step with `start_step` and reads back its `running` state and `started_at`, with `None` for the other steps.
- `state.rs`: a `StatusUpdate` request on a live session records `EventKind::StatusUpdate` with its text and time; on an exited session it is refused with `invalid_state` and records nothing.
- `messages.rs`: serialization tests for `session.post_status_update` and `EventKind::StatusUpdate`, as for the other methods and events.
- `crates/oxd/tests/runs.rs`:
  - A step whose prompt is `mcp-status Reading the code` gets the chunk `Status update posted.`, and its child's events include `StatusUpdate` with text `Reading the code` and a `posted_at` between the step's `started_at` and now.
  - `a_failed_step_fails_the_run`: the failed step's `started_at` is set, between the run's `created_at` and now, and the later step's is `None`.
- `crates/oxd/tests/daemon.rs`: in an ordinary session, `mcp-status` gets `Status update posted.`, and the session records a `StatusUpdate` that no run view shows.
- `ui/e2e/specs/run.e2e.ts`: a run whose first step posts two status updates, then waits, shows both updates under that step in order with their `HH:MM` times, the step's start time, and no start time on the pending step after it. Once the step succeeds, its section is collapsed and shows no updates until its heading is opened.

## Implementation plan

1. `ox-protocol`: add `started_at` to `StepInfo`, `EventKind::StatusUpdate`, `Method::SessionPostStatusUpdate`, and `Client::post_status_update`, with their docs and serialization tests.
2. `store.rs`: add the `started_at` column, insert and read it in `add_run` and `runs`, add `start_step`, and extend the round-trip test.
3. `daemon.rs`: make `now` public. `run.rs`: add `Run::start_step` and call it in `advance`.
4. `state.rs`: add `Request::StatusUpdate` and its arm in `State::request`, with the unit test. `session.rs`: add `Session::post_status_update`. `daemon.rs`: dispatch `SessionPostStatusUpdate`.
5. `mcp.rs`: `for_session(session_id)` unchanged. `ox-mcp`: the router field and `post_status_update`.
6. `oxd-fake-agent.rs`: `call_tool` and the `mcp-status` directive. Add the `runs.rs` and `daemon.rs` tests.
7. `crates/ox/src/conversation.rs`: `Item::StatusUpdate`.
8. `ui/src/lib/protocol.ts`: `started_at` on `StepInfo` and the `status_update` item. `Conversation.tsx`: `ItemView` renders nothing for it.
9. `RunView.tsx` and `RunStatus.tsx`: the wiring in Decisions. Delete `statusUpdates.mock.ts`.
10. `run.e2e.ts`: the status update test. Delete the development database (`target/dev/oxd.sock.db`), then run `make check` and `make e2e`.

## Documentation updates

- `docs/workflows.md`, "Running a workflow": the run view lists every step with its start time once it has started, and the status updates its agent posted, newest last; a completed step is collapsed until its heading is opened. "Agent steps": a Claude step's agent can post status updates through the `ox` tool `post_status_update`.
- `docs/dev/architecture.md`, "ox-mcp": every session's server offers `post_status_update`, which records a timestamped status update in the session's history. "Workflow runs": a step's `started_at` is set when it leaves `pending`.
- `docs/dev/acp-v2.md`, `session.mcp.stdio` row: Claude sessions get the status update tool; Pi sessions get no MCP servers, so they post none.
- `docs/dev/glossary.md`: **Run view** becomes the desktop view of a run: each step with its start time and its agent's status updates. Add **Status update**: a sentence or two a child session's agent posts on what it is doing and will do next, shown under its step in the run view.
- `crates/ox-mcp/src/main.rs`: the module docs list `post_status_update`. `oxd-fake-agent.rs`: the module docs list `mcp-status`.
