# Agent messages in the run view

## Goal

The run view shows, under each agent step, the status updates its agent posted through the `ox` tool `post_status_update`. Replace them with the step's agent messages: every agent message its child session's agent writes, oldest first, rendered as Markdown. Delete the status update tool and everything that carries a status update from the agent to the run view.

## Related code

- `crates/ox-mcp/src/main.rs` — the `ox` MCP server. `post_status_update` and `PostStatusUpdate` go; `set_title` and the tool router stay.
- `crates/ox-protocol/src/messages.rs` — `Method::SessionPostStatusUpdate`, `EventKind::StatusUpdate`, and their serialization tests.
- `crates/ox-protocol/src/client.rs` — `Client::post_status_update`.
- `crates/oxd/src/daemon.rs` — dispatches `SessionPostStatusUpdate`.
- `crates/oxd/src/session.rs` — `Session::post_status_update`.
- `crates/oxd/src/state.rs` — `Request::StatusUpdate`, its arms in `State::request`, its name in the test helper's event kind names, and `a_live_session_records_status_updates_and_an_exited_one_refuses_them`.
- `crates/oxd/src/instructions.rs` — tells a Claude step's agent to call `post_status_update`.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — the `mcp-status` directive. `messages <n> <JSON string>` already sends separate agent messages.
- `crates/oxd/tests/runs.rs` — `a_step_s_agent_posts_status_updates_to_its_child_session` and `a_step_s_agent_is_told_its_step`.
- `crates/ox/src/conversation.rs` — `Item::StatusUpdate`. Agent messages are already `Item::Text`, keyed `text:<message id>`, appended chunk by chunk.
- `ui/src/lib/protocol.ts` — the `status_update` item.
- `ui/src/features/conversation/Conversation.tsx` — `ItemView`'s `status_update` case; `Markdown` renders `text` items.
- `ui/src/features/runs/RunStatus.tsx` — `RunStatus` filters each child's conversation to `status_update` items; `StepStatus` lists them with a hover time and follows the latest while the step runs.
- `ui/src/styles/run-view.css` — `.run-status-updates` and its rules.
- `ui/mock/scenarios.ts` — `runStatus` builds status update items.
- `ui/e2e/specs/run.e2e.ts` — "follows the active step's latest status update and keeps completed updates visible".

## Decisions

- **Agent messages are the `text` items of the child's conversation.** `RunStatus` passes `StepStatus` the child conversation's `text` items whose text is not blank, in conversation order. Thoughts, tool calls, prompts and errors stay in the child's session view. The blank filter drops a message that has no text yet, as `ItemView` does for a blank thought. No new item type or event: the desktop core already folds agent messages into `Item::Text`.
- **Rendering.** Each message is an `li` holding `<Markdown text={item.text} />`, keyed by the item's `key`. The `li` has no `time`: `Item::Text` carries no time and `Event` has none. The step's start time stays.
- **Names.** In `RunStatus.tsx`, `StatusUpdate` becomes `AgentMessage` (`Extract<Item, { type: "text" }>`), `isStatusUpdate` becomes `isAgentMessage`, the `updates` prop `messages`, `followUpdates` `followMessages`, and `revealUpdate` `revealMessage`. The class `run-status-updates` becomes `run-status-messages`. Doc comments say "agent messages".
- **Styles.** Delete `.run-status-updates time` and `.run-status-updates li:hover time`. `.run-status-updates p` becomes `.run-status-messages p` and keeps `line-height` so a message's first line lines up with its dot; it drops `margin: 0` so `.markdown`'s block spacing applies between a message's paragraphs. Rename the other `.run-status-updates` selectors and update the comment on `li` ("Each message ... a dot on the connector's centre line").
- **Removal is complete.** With the tool gone, nothing posts a status update, so the method, event kind, request, item and fake agent directive all go. `call_tool` in the fake agent stays for `mcp-title`.
- **Instructions.** `instructions::text` drops the `post_status_update` paragraph. A step's agent is still told its step and that its final reply is the next step's input. The `ox_tools` parameter stays for `set_title`.

## Check your assumptions

1. **Which of the agent's output the timeline shows.** Kept.
   - **Decided:** Agent message text only, every message. "All agent responses" reads as what the agent says, and the run view stays a readable timeline.
   - **Consequences:** Tool calls, thoughts and permission outcomes show only in the child's session view. A step that works silently through tools shows nothing under it until it writes a message.
   - **Alternative:** Also list each tool call's title as a line under the step.
   - **Trade-offs:** Messages only keeps the timeline short and in the agent's words; adding tool calls shows progress during silent stretches but turns the run view into a second transcript.

2. **How much of each message shows.** Kept.
   - **Decided:** The whole message, as Markdown.
   - **Consequences:** A step's final reply, such as a full plan from a planning step, shows in full under that step, and a run view with several such steps gets long.
   - **Alternative:** Show each message clamped to a few lines, expanding on click.
   - **Trade-offs:** Full messages need no new UI and read in place; clamping keeps the run view compact but adds an expand control and hides the final reply by default.

3. **Agents are no longer told to narrate.** Kept.
   - **Decided:** The step instructions drop the status update paragraph and add nothing in its place.
   - **Consequences:** The timeline shows whatever messages the agent writes on its own. Claude usually writes short text between tool calls, but a step may show only its final reply.
   - **Alternative:** Replace the paragraph with an instruction to write a sentence on what it is doing whenever it moves on to a new part of the step.
   - **Trade-offs:** No instruction keeps agents' output unchanged and the prompt shorter; an instruction gives a steadier timeline but adds text to every step's transcript and to its final turn's context.

4. **Per-message times.** Kept.
   - **Decided:** Drop them. Events carry no time, so an agent message has none to show.
   - **Consequences:** Hovering an entry no longer shows its `HH:MM`. Each step still shows its start time.
   - **Alternative:** Add a timestamp to `Event` and the `events` table, and carry it on `Item::Text`.
   - **Trade-offs:** Dropping them deletes code; timestamps keep the hover time but change the event schema and every event the daemon records.

5. **Existing databases.** Kept.
   - **Decided:** Remove `EventKind::StatusUpdate` outright. The store has no migrations; a schema change means deleting the database (`Store::open`).
   - **Consequences:** A database holding a session that posted a status update fails to load that session's events. The development and installed databases need deleting after this change.
   - **Alternative:** Keep `EventKind::StatusUpdate` so old events still parse, and fold it to nothing.
   - **Trade-offs:** Removing it leaves no dead variant; keeping it spares existing databases at the cost of a variant nothing produces.

## Test plan

- `crates/oxd/tests/runs.rs`: delete `a_step_s_agent_posts_status_updates_to_its_child_session`. In `a_step_s_agent_is_told_its_step`, drop the `` `post_status_update` `` expectation and the doc comment's "and to post status updates".
- `crates/oxd/src/state.rs`: delete `a_live_session_records_status_updates_and_an_exited_one_refuses_them`.
- `crates/ox-protocol/src/messages.rs`: delete the serialization tests for `session.post_status_update` and `EventKind::StatusUpdate`.
- `ui/e2e/specs/run.e2e.ts`: the status update test becomes "follows the active step's latest agent message and keeps completed messages visible". Its script uses the directive lines `messages 1 "Reading the code"` and `messages 1 "Writing the fix"` in place of `mcp-status`, with the same `shell` waits. It asserts `.run-status-messages li` holds the two messages in order, the step's start time, no start time on `Later 1`, the latest message inside the scrolled view, and both messages still listed once the run succeeds. It no longer checks per-entry times.

## Implementation plan

1. `crates/ox-mcp/src/main.rs`: delete `PostStatusUpdate`, `post_status_update`, and its line in the module docs.
2. `ox-protocol`: delete `Method::SessionPostStatusUpdate`, `EventKind::StatusUpdate`, `Client::post_status_update`, and their tests.
3. `oxd`: delete the `daemon.rs` dispatch, `Session::post_status_update`, `Request::StatusUpdate` with its `State::request` arms, its event kind name, and its test. Drop the paragraph from `instructions.rs`.
4. `oxd-fake-agent.rs`: delete the `mcp-status` directive and its line in the module docs.
5. `crates/oxd/tests/runs.rs`: the changes in Test plan.
6. `crates/ox/src/conversation.rs`: delete `Item::StatusUpdate`, its `apply` arm, and its `key` arm.
7. `ui/src/lib/protocol.ts`: delete the `status_update` item. `Conversation.tsx`: delete its `ItemView` case and comment.
8. `RunStatus.tsx` and `run-view.css`: the changes in Decisions.
9. `ui/mock/scenarios.ts`: `runStatus` builds `text` items (`{ type: "text", key: "text:1", text: "...", replace: false }`) and its run is named "Follow workflow agent messages"; give one message two paragraphs.
10. `run.e2e.ts`: the test in Test plan. Delete the development database (`target/dev/oxd.sock.db`), then run the checks.

## Documentation updates

- `docs/workflows.md`, "Agent steps": delete the paragraph on status updates. "Running a workflow": every step "with its start time once it has started and the messages its agent wrote, oldest first. While the run is active, the view scrolls as needed to show the active step's latest message."
- `docs/dev/architecture.md`, "ox-mcp": its tool is `set_title`, which sets the session title.
- `docs/dev/glossary.md`: **Run view** becomes "each step with its start time and its agent's messages". Delete **Status update**.
