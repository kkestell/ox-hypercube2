# Queued and steered prompts

## Goal

While an agent works, Enter queues the composer's prompt in the daemon and
Cmd+Enter steers it into the running turn. The daemon sends queued prompts one
per turn, holds the queue after Stop or an error, and keeps it across restarts.

**Brief:** `.agents/briefs/2026-11-16-005-queued-and-steered-messages.md`

## Related code

- `crates/oxd/src/state.rs` — The session state machine: `Request`, `Change`,
  `Live`, `Turn`, `Phase::after`, `State::request`, `State::fill`. The queue,
  steering and the drain rule live here.
- `crates/oxd/src/session.rs` — `Session::prompt`, `Session::update` (where
  every change lands, and where the drain will send), the agent task's
  `Command::Prompt` handler that answers `PromptRejected`.
- `crates/oxd/src/store.rs` — SQLite schema and `SavedSession`; side tables
  such as `session_unseen` are the pattern for the queue.
- `crates/oxd/src/daemon.rs` — Method dispatch (`Method::SessionPrompt` at
  `daemon.rs:395`), session restore at startup.
- `crates/ox-protocol/src/messages.rs` — `Method`, `SessionInfo`, `EventKind`.
- `crates/ox-protocol/src/client.rs` — `Client::prompt` and siblings.
- `crates/claude-code-acp/src/runner.rs` — `Claude::prompt` writes stream-json
  user messages; `Output::started` answers a prompt when Claude takes it in;
  `Output::result` ends turns and drops untaken prompts on interrupt.
- `crates/pi-acp/src/pi.rs` — `Pi::prompt` sends `streamingBehavior:
  "followUp"` and answers on Pi's response; `Pi::abort`.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — Scripted agent for daemon tests.
- `crates/ox/src/conversation.rs` — Folds events into UI items;
  `Item::RequestResolved` is the only removal today; `in_turn` hides user
  messages that echo prompts.
- `crates/ox/src/lib.rs` — Tauri commands `prompt`, `cancel`.
- `crates/oxctl/src/main.rs` — `oxctl prompt`.
- `ui/src/features/composer/Composer.tsx`, `CommandInput.tsx`, `useDrafts.ts`
  — Composer, key handling, per-session drafts.
- `ui/src/features/conversation/useConversations.ts`, `Conversation.tsx` —
  Item reducer and transcript rows.
- `ui/src/lib/protocol.ts`, `ui/src/test/core.tsx` — UI types and the fake
  desktop core.
- `scripts/smoke-acp.py` — Live checks against real Claude and Pi.

## Decisions

### Protocol: a prompt during a turn is a steer

ACP v2 defines the `session/prompt` answer as acknowledging insertion into the
conversation, not turn completion, so a prompt sent during a turn is a steer
with no protocol extension. The daemon owns queueing; the bridges never hold a
prompt for after the turn.

- `claude-code-acp` sends every user message with `"priority": "next"`. The
  installed Claude CLI (2.1.326) folds `next` messages into the running turn
  at its next opportunity, and runs one left over when the turn ends as its
  own turn. Its default priority depends on a feature flag, so it is always
  explicit.
- Claude may stop a turn that has shown no output when a `next` message
  arrives, emitting `{"type":"system","subtype":"turn_preempted",
  "preempted_by_uuid":…}` before the turn's aborted `result`, then running the
  follow-up. The bridge treats that `result` as a continuation: it keeps its
  untaken prompts, fails open tools, and reports no `idle`.
- `pi-acp` sends `streamingBehavior: "steer"` (ignored while idle). A prompt
  answered with disposition `queued` waits in a FIFO, `State::steering`, and is
  answered and echoed as a `user_message` when Pi emits a `message_start` whose
  message has role `user`. Pi delivers steering messages in order, and delivers
  any left at the end of a run before settling.
- On cancel, `pi-acp` sends `clear_queue` before `abort` (Pi's `abort` otherwise
  continues queued messages) and fails every held steering prompt with
  `request_cancelled`. Claude's interrupt already uses `cancel_queued: true`.

### Daemon protocol

- `session.prompt` sends now: on an idle session it starts a turn
  (`PromptStarted`), during a turn (state `running`, `awaiting_permission` or
  `awaiting_input`) it steers (`PromptSteered`). Either resumes a paused queue.
- `session.queue_add { session_id, prompt }` appends and returns
  `{ "prompt_id": … }`.
- `session.queue_edit { session_id, prompt_id, prompt }` replaces a queued
  prompt in place; an unknown ID (already sent or removed) is `not_found`.
- `session.queue_remove { session_id, prompt_id }` removes one; an unknown ID is
  `not_found`.
- `session.queue_send { session_id, prompt_id }` removes it and sends it as
  `session.prompt` does, atomically; a refused send leaves it queued.
- `SessionInfo.queue: Vec<QueuedPrompt>`, with
  `QueuedPrompt { id: String, prompt: Vec<ContentBlock> }`. The paused flag is
  daemon-internal: the composer's behavior does not depend on it.
- New events:
  - `PromptSteered { prompt: Vec<ContentBlock> }` — a prompt sent during a
    turn.
  - `SteeredPromptTaken { prompt: u64 }` — the agent inserted the steered
    prompt recorded at seq `prompt`.
  - `SteeredPromptDropped { prompt: u64 }` — the agent refused it or dropped
    it on Stop, or exited before taking it in.

### Session state (`state.rs`)

- `State` gains `queue: Queue { prompts: Vec<QueuedPrompt>, paused: bool }`
  and a `queue_changed` flag, read and cleared by `take_changed_queue()` the way
  `take_unlogged` works. `Phase::after` takes `&mut Queue` beside
  `next_request`.
- `Live` gains `steered: Vec<u64>`, the seqs of steered prompts the agent has
  not answered. `Live::quiet` also requires it to be empty.
- `Request::Prompt` steers when `live.turn` is `Some`, starts a turn when the
  state is `Idle`, and otherwise keeps today's `invalid_state` error. Both paths
  clear `paused`. New requests `QueueAdd(QueuedPrompt)`,
  `QueueEdit(QueuedPrompt)`, `QueueRemove { id }`, `QueueSend { id }`.
  `QueueAdd` and `QueueEdit` check the prompt against the agent's prompt
  capabilities when it has them, and are refused only on a closed session.
  The session generates the ID with `uuid`.
- `Change::PromptAccepted { prompt: u64 }` is new; the agent task records it
  on a successful `session/prompt` answer. For a steered seq it removes the seq
  from `steered` and records `SteeredPromptTaken`; for the turn's prompt it
  changes nothing. `Change::PromptRejected` for a steered seq removes it and
  records `SteeredPromptDropped`. `Live::end` records `SteeredPromptDropped`
  for every seq left in `steered`. A steered prompt still pending at a normal
  turn end stays in `steered`; the agent starts its own turn with it.
- Pausing: `Change::Cancelled` and any `TurnEnded` other than a normal end set
  `paused`. A normal end is `error: None` with stop reason `end_turn` or none,
  the same turns the conversation marks `TurnCompleted`.
- The drain, `State::start_queued() -> Option<(Vec<ContentBlock>, u64)>`: when
  the phase is `Live`, no turn, no pending request, `steered` is empty, the
  queue is not paused and not empty, it pops the front, records `PromptStarted`
  and sets `Turn::Prompted(seq)`. `Session::update` calls it after every change
  and sends `Command::Prompt`. This covers turn ends, `queue_add` on an idle
  session, and `session.prompt` resuming a queue. Add a `TODO` that the drain
  does not check `refuse_moving` for a branch move in progress.
- `State::fill` records `SteeredPromptDropped` for every `PromptSteered` left
  without a taken or dropped event.
- `State::restored` and `State::unstarted` take the saved `Queue`.

### Persistence

New table, created with `IF NOT EXISTS` so existing databases open:

```sql
CREATE TABLE IF NOT EXISTS session_queues (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    paused     INTEGER NOT NULL,
    prompts    TEXT NOT NULL  -- JSON array of QueuedPrompt
);
```

`Store::set_session_queue` upserts it, written inline from `Session::update`
when `take_changed_queue()` returns a queue, like `set_session_unseen`.
`SavedSession` gains `queue: Queue`, read with a `LEFT JOIN`.

### Conversation (`crates/ox/src/conversation.rs`)

- `Item::Removed { key }` replaces `Item::RequestResolved { request_id }`;
  resolved requests send `Removed { key: "request:<id>" }`. `coalesce` drops
  the position of a removed key.
- `Item::Steered { key, text, attachments }` for `PromptSteered`, keyed by its
  seq. `SteeredPromptTaken` yields `Removed` for the steered key and an
  `Item::Prompt` keyed by the taken event's seq, so the prompt shows where it
  landed. `SteeredPromptDropped` yields `Removed`. The fold keeps each steered
  prompt's text and attachments by seq.
- User messages from the agent show only before the first `AgentStarted`:
  after it, every `user_message` echoes a prompt the session recorded (both
  bridges emit them only for prompts they were sent), and replay always
  precedes `AgentStarted`. A `started` flag replaces `in_turn`. This keeps a
  steered prompt that Claude runs as its own turn from showing twice.

### UI

- `QueuedPrompts` (new, `ui/src/features/composer/QueuedPrompts.tsx`) renders
  `session.queue` inside `.stack`, directly above `.entry`: each row shows the
  prompt's text and attachment names, with Edit (`Pencil`), Send Now
  (`ArrowUp`) and Delete (`Trash2`) icon buttons shown on hover, each with an
  `aria-label`. Text comes from text blocks, attachments from resource links
  (name for display, `file://` URI decoded to a path for editing).
- Keys: `CommandInput`'s `onSubmit` takes `steer: boolean`, true for
  Cmd+Enter. Idle: either sends with `prompt`. Running, awaiting permission or
  input: Enter invokes `queue_add`, Cmd+Enter invokes `prompt`. `canSend`
  allows those states; starting, exited and a running parent run stay disabled.
- Editing: Edit loads the queued prompt into the composer and holds the
  current draft. Enter saves with `queue_edit` and restores the held draft. A
  failed save shows its error and stays in edit mode. The draft gains an
  optional `editing: { promptId, held }` field in `useDrafts`.
- While running, the form's one button is Send (queues) when the draft has
  content and Stop when it is empty.
- Escape stops the viewed session's running turn (or its running parent run,
  as Stop does), from a window `keydown` listener the composer adds while
  running. It skips events already handled (`defaultPrevented`), so the
  command menu's Escape still only dismisses the menu.
- Transcript: `rows()` sets `steered` items aside and renders them after all
  other rows, as `.prompt.steered` with `opacity: var(--dimmed-opacity)`.
  `useConversations` handles `removed` by key.
- Tauri commands `queue_add(session_id, text, attachments) -> String`,
  `queue_edit(session_id, prompt_id, text, attachments)`,
  `queue_remove(session_id, prompt_id)`, `queue_send(session_id, prompt_id)`.
  They and `prompt` share one function that builds the content blocks.

### oxctl

`oxctl prompt` keeps its name and now steers during a turn, matching the
daemon's single send-now method. A `queue` subcommand group:
`queue add SESSION TEXT` prints the ID, `queue ls SESSION` prints `id\ttext`
lines, `queue edit SESSION PROMPT TEXT`, `queue rm SESSION PROMPT`,
`queue send SESSION PROMPT`.

### Fake agent

`oxd-fake-agent` tracks whether a turn is running. A prompt that arrives during
a turn is held, not run as a script. The new directive `take` inserts the held
prompts in order: it echoes each as a `user_message` and answers it. When a
turn ends with held prompts, the agent starts a turn of its own for them
(`running`, insert, `idle` with `end_turn`), as Claude does. Cancelling answers
held prompts with `request_cancelled` before the turn's `idle`.

## Check your assumptions

1. **What happens to a steered prompt the agent had not taken when the user
   presses Stop.** *Changed: discarded, the simple option.*
   - **Decided:** It is discarded: its dimmed entry leaves the conversation.
     Both bridges drop untaken prompts on cancel, so the agent never sees it.
   - **Consequences:** The text is gone after Stop.
   - **Alternative:** Put it back at the front of the held queue.
   - **Trade-offs:** Discarding needs no requeue logic; returning keeps the
     text.

2. **How Edit treats a queued prompt.** *Changed: replaced in place, with no
   removal or hold.*
   - **Decided:** Edit loads the prompt into the composer; saving replaces it
     in place with `session.queue_edit`.
   - **Consequences:** If the queue sends the prompt while it is being
     edited, saving shows an error.
   - **Alternative:** Take it out of the queue while editing.
   - **Trade-offs:** Replacing in place is the least code; taking it out
     prevents the race but complicates putting it back.

3. **What counts as a turn that "ends with an error" and pauses the queue.**
   - **Decided:** Every end other than `end_turn` without an error: errors,
     agent exits, and also token limits, request limits and refusals.
   - **Consequences:** A turn that hits its token limit holds the queue the
     same as a failure.
   - **Alternative:** Pause only on Stop, errors and exits; keep sending after
     token limits, request limits and refusals.
   - **Trade-offs:** Pausing on every abnormal end avoids sending into a turn
     that stopped short; continuing keeps long queues moving past soft limits.
   - *Kept.*

4. **Where a steered prompt that lands mid-turn sits in the Worked grouping.**
   - **Decided:** It is a prompt like any other, so the work before it shows
     as individual rows rather than collapsing into a Worked group when the
     turn completes.
   - **Consequences:** A steered turn shows its pre-steer work expanded.
   - **Alternative:** Group the work before the steered prompt into its own
     Worked group.
   - **Trade-offs:** The decided version needs no change to `rows()`; grouping
     keeps long turns compact but adds a second grouping rule.
   - *Kept.*

5. **The composer's buttons while the agent works.** *Changed: one button,
   Send with a draft and Stop without; Escape always stops.*
   - **Decided:** One button: Send (queues) when the draft has content, Stop
     when it is empty. Escape stops the running turn.
   - **Consequences:** Stop is on Escape while a draft exists.
   - **Alternative:** Stop and Send side by side.
   - **Trade-offs:** One button keeps the layout unchanged; two keep Stop
     clickable while typing.

## Test plan

- `state.rs` unit tests:
  - A prompt during a turn records `PromptSteered`; its acceptance records
    `SteeredPromptTaken`.
  - Queued prompts start one turn each, in order, after normal turn ends;
    `queue_add` on an idle, unpaused session starts a turn at once.
  - Cancel and an abnormal end pause the queue; `session.prompt` and
    `queue_send` resume it.
  - A rejected steered prompt, and one left at exit or in `fill`, records
    `SteeredPromptDropped`.
  - A steered prompt pending at a normal turn end blocks the drain until the
    agent's own turn for it ends.
  - `queue_edit` replaces in place; `queue_edit` and `queue_remove` of an
    unknown ID are refused.
- `store.rs`: a saved queue and its paused flag come back with the session.
- Daemon integration tests (`crates/oxd/tests/daemon.rs`), synchronized on
  events:
  - Queued prompts are sent after the turn ends with no client connected.
  - A steered prompt is taken by the fake agent's `take` inside the turn.
  - Stop holds the queue and drops an untaken steered prompt.
  - A held queue survives a daemon restart.
- `conversation.rs`: steered item; taken moves it to the taken event's
  position; dropped removes it; user messages after `AgentStarted` are
  hidden, replayed ones before it are shown; `coalesce` with `Removed`.
- `claude-code-acp` protocol tests: user messages carry `priority: "next"`;
  a `turn_preempted` followed by an aborted result keeps the turn running and
  answers the follow-up when it starts.
- `pi-acp` protocol tests: a steered prompt is answered at the user
  `message_start`; cancel sends `clear_queue` before `abort` and fails the
  held prompt.
- UI tests: Enter while running invokes `queue_add`, Cmd+Enter invokes
  `prompt`; queue rows invoke `queue_send` and `queue_remove`; Edit then Enter
  invokes `queue_edit` and restores the held draft; while running, the button
  is Send with a draft and Stop without, and Escape invokes `cancel` but only
  closes an open command menu; steered items render dimmed after the other
  items and leave on `removed`.
- End-to-end (`ui/e2e/specs/session.e2e.ts`): one case that queues prompts
  during a turn, edits one, deletes one, sends one now, and sees the edited
  one start the next turn; it covers the four new Tauri commands.
- Smoke (`scripts/smoke-acp.py`): `check_steer` steers a turn running a slow
  shell command with `oxctl prompt` and expects `SteeredPromptTaken` before
  `TurnEnded`; `check_queue` queues a prompt during a turn and expects it to
  start the next turn. Run `make smoke-claude` and `make smoke-pi`.

## Implementation plan

1. `ox-protocol`: `QueuedPrompt`, `SessionInfo.queue`, the three event kinds,
   the four methods and `session.prompt` docs; `Client::queue_add`,
   `queue_edit`, `queue_remove`, `queue_send`.
2. `oxd/src/state.rs`: `Queue`, `Live::steered`, the requests and changes,
   pausing, dropping, `start_queued`, `fill`, constructors; unit tests.
3. `oxd/src/store.rs`: `session_queues`, `set_session_queue`,
   `SavedSession.queue`; test.
4. `oxd/src/session.rs`: `PromptAccepted` on a successful answer; drain and
   queue save in `update`; `queue_add`/`queue_edit`/`queue_remove`/
   `queue_send`; `info_of` fills `queue`.
5. `oxd/src/daemon.rs`: dispatch the methods (with `refuse_moving` for
   `queue_add` and `queue_send`); restore the queue at startup.
6. `oxd-fake-agent`: held prompts, `take`, own turn for leftovers, cancel;
   update the module doc. Daemon integration tests; adjust tests that expect a
   prompt during a turn to be refused or run concurrently.
7. `claude-code-acp/src/runner.rs`: `priority: "next"`, `turn_preempted`;
   tests.
8. `pi-acp/src/pi.rs`: steer, `State::steering`, user `message_start`,
   `clear_queue` on abort, failing held prompts on exit; tests.
9. `crates/ox/src/conversation.rs`: `Removed`, `Steered`, taken/dropped,
   `started`; tests. `crates/ox/src/lib.rs`: Tauri commands.
10. `oxctl`: `prompt` docs, `queue` group.
11. UI: `protocol.ts`, `useConversations.ts`, `Conversation.tsx` and
    `conversation.css`, `CommandInput.tsx`, `useDrafts.ts`, `Composer.tsx`,
    `QueuedPrompts.tsx` and `composer.css`, `test/core.tsx` fixtures,
    `mock/scenarios.ts`; UI tests; the end-to-end case.
12. `scripts/smoke-acp.py`: `check_steer`, `check_queue`.

## Documentation updates

- `docs/dev/architecture.md`: `oxd` owns each session's queue; persistence
  lists the queue table; session lifecycle describes steering, the drain rule
  and pausing.
- `docs/dev/acp-v2.md`: `session/prompt` caveat — during a turn, Claude folds
  the prompt in at its next opportunity (`priority: "next"`) and Pi steers it;
  `session/cancel` caveat — both bridges drop untaken prompts.
- `docs/dev/glossary.md`: **Queued prompt** and **Steered prompt**.
- `spec/conversations.md`: Compose and send (Enter queues and Cmd+Enter steers
  while the agent works, the queue list and its actions, steered prompts in
  the conversation).
- `spec/requests.md`: Escape stops a turn; Stop holds the queue.
- `spec/sessions.md`: the queue belongs to the session and survives restarts.
- `spec/cli.md`: `prompt` steers during a turn; the `queue` commands.
- `README.md`: add queueing and steering to the features if it lists composer
  features.
