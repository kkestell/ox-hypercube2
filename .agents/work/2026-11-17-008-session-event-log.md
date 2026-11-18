# The database is each session's event log

## Plan

`.agents/plans/2026-11-17-009-session-event-log.md`

## Summary

`State` holds no history. `Session::update` commits a transition's row changes and events in one transaction, then publishes the last durable seq. Subscribers and transcripts read events from the database by seq. At startup, `Store::left_open` finds what a crash left open through the new `events_by_type` index (schema 3), and `Session::recover` closes it. The batch writer, `flush`, `fill`, `history` and `loaded` are gone. The plan's goal is met: the three Daemon bugs are fixed and the `TODO` at `state.rs:778` is removed.

## Departures from the plan

- A subscriber to a deleted session still gets the events that close it. The plan accepted losing them, but `delete_ends_the_turn_and_its_permission_request` showed that this was observable behavior. `State` keeps the closing change's events in memory, as `closing()`, in place of the planned `session_closed()`. Once every stored event has been read, `Session::events_after` gives each closing event above the last seq read, so a subscriber that is caught up sees what it saw before. A subscriber that is behind when the history is deleted still misses the stored events it had not read.

## Decisions

- A created session's `SavedSession` has `needs_replay: true`. Its first `AgentStarted` or `PromptStarted` clears the flag.
- The `closing` snapshot is taken before the database read. Closing events are written in the change that closes the session, under the state lock, so the snapshot cannot skip stored events.
- When `events_after` reads nothing for an open session, it waits for the next change. This covers a deleted session between its row deletion and its close.
- Schema 3's fixture is `schema-2.sql` with the version set to 3 and the index added, in the same dump style.

## Checks run

- `make check-rust`: passed. It covers formatting, clippy with warnings as errors, and the workspace tests, including 102 unit tests and 86 daemon integration tests in `oxd`.

## Manual verification

1. Startup recovery and the event write cost, on a synthetic database of 2,000 sessions and 1M events (1 GB, 3% structural), measured while planning with `/tmp/oxbench/bench.py`, `q2.py` and `q3.py`:
   - Index build: 1 s.
   - Per-session turn probes: 14 ms.
   - Grouping open requests: 30 ms.
   - One event write committed in its own transaction: 26 µs.
   - Reading a 256-event batch: 81 µs.

   `the_startup_queries_search_events_by_type` checks that each startup query uses the index and never scans `events`.

2. The to-do's reproduction of a failed event write is automated as `an_event_a_write_lost_is_skipped_by_every_subscriber`:

   ```sh
   cargo build -p ox-mcp && cargo test -p oxd --test daemon an_event_a_write_lost
   ```

   It passed.
