# The database is each session's event log

## Goal

Close the Structure item "A restored session is a second mode of `State`" in `.agents/todo.md`, and the Daemon bugs it underlies.

When the work is done, a session's events live only in the database. `State` holds no event history, so a created session and a restored one are the same kind of state; a restored one starts with its saved last seq. `Session::update` writes a transition's rows and events in one transaction before it publishes the new last seq. Subscribers and transcripts read events from the database by seq. Startup closes what a crash left open by querying the database for it, without reading any session's history.

### Bugs closed

- **Deleting a session during its first history read can hang a subscriber** (`conn.rs:276`, `state.rs:1150`). There is no history read. `append` always records an event, so closing any session records `SessionClosed`, and a subscription to a closed session ends with it.
- **A failed event write breaks replay after restart** (`store.rs`, `state.rs`). Subscribers read `seq > after_seq` from the database, so an event lost to a failed write leaves a gap that readers skip, during the run and after a restart. A subscription from a nonzero cursor no longer stalls. A subscription from zero still lacks the lost event; that event was never stored.
- **A crash-interrupted turn shows its error only once history is read** (`state.rs:778`). Startup ends the interrupted turn, cancels open requests and drops untaken steered prompts, so the error and its unseen outcome appear when `oxd` restarts. The `TODO` at `state.rs:778` goes. The other two bugs have no `TODO` comment in the code.

The design also closes the case the previous plan listed under "Interactions with the event log": a never-started session whose first prompt a stop left open now has that turn ended at startup instead of on its first history read. None of the other listed bugs is closed. The Scale item "Measure the cost of holding every session in memory" stays open; a session no longer keeps its history in memory once read.

## Related code

- `crates/oxd/src/state.rs` — `State` (`:606`) with `events: Option<Vec<Event>>`, `unlogged` and `next_request`; `State::of` (`:689`), `loaded` (`:716`), `fill` (`:725`, the crash recovery and the `TODO`), `load` (`:861`, decides replay by scanning `events`), `events` (`:930`), `subscribe` (`:941`), `events_after` (`:958`, the vector offset), `append` (`:1325`, records nothing for an unloaded session), `BATCH` (`:25`). The unit tests read `state.events()` through `kinds` (`:1669`) and test `fill` (`:2509-2650`).
- `crates/oxd/src/session.rs` — `history` (`:95`, `:542`), `load_if_exited` (`:298`), `events_after` (`:571`), `started` (`:600`, reads the last `AgentExited` from `events()`), `transcript` (`:655`), `apply` (`:842`, saves rows, then `store.log(events)`, then publishes `last_seq`).
- `crates/oxd/src/store.rs` — `Write`, the `writer` thread, `log`, `flush` (`:78-130`), `events` (`:131`), `sessions` (`:215`), `save_session` (`:355`), `insert_events` (`:578`), `write_events` (`:646`), `MIGRATIONS` (`:438`), the fixtures test (`:1266`).
- `crates/oxd/src/daemon.rs` — `Daemon::new` (`:118`), `flush` (`:247`), `add_imported` (`:1214`, builds a `SavedSession` from imported events).
- `crates/oxd/src/conn.rs:276` — `subscribable` waits for `history`; `forward` loops on `events_after`.
- `crates/oxd/src/main.rs:229` — shutdown flushes the event queue.
- `crates/oxd/tests/daemon.rs:2024` — `a_history_that_a_crash_left_open_ends_its_turn` inserts open events into a stopped daemon's database.
- `crates/oxd/tests/fixtures/schema-1.sql`, `schema-2.sql`.

## Decisions

- **`State` holds no history.** `events`, `loaded`, `fill`, `events()`, `events_after` and the `restored`-only branches go. `append` always assigns the next seq. It queues every event but `SessionClosed` in `unlogged`, and keeps `SessionClosed` as `State::session_closed() -> Option<&Event>`. Two facts that were read from the history become fields:
  - `needs_replay: bool`, whether the log holds neither an `AgentStarted` nor a `PromptStarted`. `load` uses it to decide replay. It comes from `SavedSession.needs_replay`, and `append` clears it when it records either event.
  - `exit_error: Option<String>`, the error of the last `AgentExited` recorded. `Session::started` reads it.
- **Request IDs continue from the saved last seq.** `State::of` sets `next_request` to `saved.last_seq + 1`. Each request's ID is at most the seq of its own `PermissionRequested` or `ElicitationRequested` event, so a restored session's new IDs are above every stored one, and nothing reads the history for them.
- **Rows and events commit together, before publishing.** `Store::save_session(id, &[RowChange], &[Event])` writes both in one transaction. Events of a session whose row is gone are still dropped, as `insert_events` does now, because a deleted session records its closing events after its rows are removed. `apply` calls it in place of `save_session` and `store.log`. The writer thread, `Write`, `Store::log`, `Store::flush`, `Daemon::flush` and the shutdown flush in `main.rs` go. `Store` holds its connection as `Mutex<Connection>`.
- **The published last seq is the last durable one.** After a successful write, or once the session is closed, `apply` publishes `state.last_seq()` on `Session.last_seq`. After a failed write it keeps the published value and only notifies, so `started` still wakes. The seqs of the lost events are not reused within the run. After a restart, `last_seq` is `MAX(seq)` of the stored events, and no stored event refers to a seq above it, so reuse after a restart is harmless.
- **Subscribers read from the database.** `Store::events_after(id, after_seq, limit)` returns events with `seq > after_seq` in order. `Session::events_after(after_seq)` does the following:
  1. Wait until the published last seq passes `after_seq`.
  2. Read up to `BATCH` events in `spawn_blocking`, as the history read does now.
  3. If the session is closed and the read is short of a batch, append its `SessionClosed` event.
  4. If the read is empty and the session is open, wait for the next change instead of reading again. This covers a deleted session between its row deletion and its close.

  `subscribable` in `conn.rs` stops awaiting history and becomes synchronous. `BATCH` moves to `session.rs`.
- **Transcripts read the history when asked.** `Session::transcript` reads `store.events(id)` in `spawn_blocking` and pages it, keeping nothing.
- **Startup recovery is a store query.** `Store::left_open() -> HashMap<String, LeftOpen>` returns, for each session whose log a stop left open, `LeftOpen { turn: bool, requests: Vec<(String, bool)>, steered: Vec<u64> }`:
  - `turn`: the session's last `prompt_started` or `turn_started` seq is above its last `turn_ended` seq.
  - `requests`: each request ID with a requested event and no resolved one, and whether it is a permission request.
  - `steered`: each `prompt_steered` seq with no `steered_prompt_taken` or `steered_prompt_dropped` naming it.

  `State::recover(LeftOpen)` appends what `fill` appends now, in the same order: the resolutions cancelling open requests, the `SteeredPromptDropped`s, then the erroring `TurnEnded`, which holds the queue. `Daemon::new` calls `Session::recover`, which is `update(|state| state.recover(left_open))`, for each session in the map. The write, the queue's paused row and the unseen error therefore land at startup.
- **An index on event type bounds startup.** Schema step 3, `add_events_by_type`, creates `events_by_type ON events (kind ->> '$.type', session_id, seq)`. Every `left_open` and `needs_replay` query filters on exactly that expression, so they read index entries of the few structural event types and never scan message chunks. `Store::sessions` computes `needs_replay` with a `NOT EXISTS` probe per session. A synthetic database of 2,000 sessions and 1M events (1 GB, 3% structural) gives the following costs:
  - Building the index: 1 s, once.
  - The per-session turn probes: 14 ms.
  - Grouping open requests: 30 ms.
  - One event write, committed in its own transaction: 26 µs.
  - Reading a 256-event batch: 81 µs.

  A new fixture, `schema-3.sql`, records the schema.
- **Imported sessions** get `needs_replay` from their events in `add_imported` and have nothing left open.
- **No protocol change.** Event and `SessionInfo` shapes are unchanged. Request IDs stay numeric strings, now continuing from the session's last seq after a restart instead of from the highest stored ID.

### Behavior that changes

- After a crash, an interrupted turn's error, its cancelled requests, the held queue and the session's red dot appear as soon as `oxd` restarts, not once the session's history is first read. This is bug 3's fix.
- A subscriber to a session being deleted gets `SessionClosed` once it has read what is still stored. Events that were not yet read when the deletion removed the history, including those the deletion itself records, are not delivered. The desktop drops a deleted session's view, so this is visible only to `oxctl attach`.
- After a restart, new request IDs start above the session's last seq, not at the highest stored ID plus one.
- An event write that fails also fails the row changes of the same transition, which are logged and lost together, and an archive or unarchive whose write fails answers the storage error as before. Before, the rows were written and only the events were lost.
- A process crash loses no published event, because events commit before they are published. Losing recent events now takes a system crash or power loss.

## Check your assumptions

None.

## Test plan

- `crates/oxd/src/state.rs`:
  - The tests read events through a test-only `State::events()` over `unlogged`, which tests never drain, in place of the stored history.
  - Closing a restored session records `SessionClosed` at `last_seq + 1`, and `session_closed()` returns it.
  - A restored session at last seq N gives its first permission request ID `N + 1`.
  - `recover` replaces the `fill` tests: it closes an open turn, open requests and untaken steered prompts in `fill`'s order, holds the queue for an open turn, and with nothing open appends nothing.
  - `load` requests replay exactly when `needs_replay` is set. Recording `AgentStarted` or `PromptStarted` clears it.
- `crates/oxd/src/store.rs`:
  - `save_session` writes rows and events together. An event insert that a trigger rejects leaves the rows unwritten too, and drops the events of a session with no row.
  - `events_after` returns events above the cursor across a gap in seqs, up to the limit.
  - `left_open` reports an open turn, an open permission request and elicitation, and an untaken steered prompt. It reports nothing for a closed history or an imported one.
  - Each `left_open` and `needs_replay` query's `EXPLAIN QUERY PLAN` searches `events_by_type`.
  - `sessions` gives `needs_replay` for a session with and without a prompt.
  - The fixtures test covers `schema-3.sql`. The writer, `log` and `flush` tests go.
- `crates/oxd/tests/daemon.rs`:
  - `a_history_that_a_crash_left_open_ends_its_turn` also asserts that, after the restart and before any subscription, the listing shows the session's unseen error. It also asserts that the events after `started` are already in the database when it is opened read-only.
  - A failed event write, following the to-do's reproduction:
    1. While the daemon runs, install a trigger rejecting `prompt_started` inserts, and send a prompt.
    2. A subscription from 0 receives every event but the lost one, up to `turn_ended`, without stalling.
    3. After a restart, a subscription from the second-to-last stored seq receives the last one at once.
  - A session restored after a restart is deleted while a client subscribes to it. The subscription ends with `SessionClosed`.
  - A never-started session whose first prompt a crash left open (inserted `prompt_started`, no `turn_ended`) has its turn ended at startup.
- Existing coverage stays: shutdown delivering the rest of every followed session, `a_deleted_session_loses_its_history`, replay on load, and the import tests.

## Implementation plan

1. `store.rs`: add step 3 `add_events_by_type` and `crates/oxd/tests/fixtures/schema-3.sql`. Add `SavedSession.needs_replay` (computed in `sessions`), `LeftOpen` and `left_open`, `events_after`, and the events argument of `save_session`. Remove `Write`, the writer thread, `write_events`, `log` and `flush`. Update the store tests.
2. `state.rs`: remove `events`, `loaded`, `fill`, `events()`, `events_after` and `BATCH`, and the `TODO`. Add `needs_replay`, `exit_error`, `session_closed`, `recover`, and `next_request` from the last seq. Make `append` unconditional. Update the unit tests.
3. `session.rs`: remove `history` and its awaits. Rewrite `apply` to save rows and events together and publish the durable seq. Rewrite `events_after` and `transcript` to read the store. Make `started` read `exit_error`. Add `recover`, and move `BATCH` here.
4. `daemon.rs`: in `Daemon::new`, recover each session `left_open` names. Set `needs_replay` in `add_imported`. Remove `flush`, and rewrite `Daemon::new`'s doc.
5. `conn.rs`: `subscribable` without the history wait. `main.rs`: remove the shutdown flush and its comment.
6. Add and update the tests above.
7. Update the documentation below.

## Documentation updates

- `docs/dev/architecture.md`, "Persistence and recovery":
  - Conversation events are saved with the session's row changes, in the step that records them, before clients are told. A failed write is logged and leaves a gap in saved history.
  - Saved history is read from the database when a client asks for it.
  - At startup, `oxd` ends turns a crash interrupted, cancels unresolved requests and drops steered prompts the agent had not taken in, found by event type without reading histories.
- `spec/sessions.md:92`: if a crash leaves a turn unfinished, the session shows it ended with "oxd stopped during the turn", as an unseen error, as soon as the service restarts, and unanswered requests are cancelled. The most recent output may be lost after a system crash or power loss.
- `.agents/todo.md`: check off the Structure item and the three Daemon bugs under Bugs closed.
