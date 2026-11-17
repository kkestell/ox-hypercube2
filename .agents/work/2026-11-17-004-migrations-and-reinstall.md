# Database migrations and separate app and daemon reinstalls

## Plan

`.agents/plans/2026-11-17-004-migrations-and-reinstall.md`

## Summary

The store has a versioned schema (`PRAGMA user_version`, `MIGRATIONS`, step 0
is today's tables), refuses newer databases, and opens a `schema-1.sql`
fixture covering every `EventKind` variant. The protocol version is an FNV-1a
hash of `ox-protocol`'s source, and the app names `make install` on a
mismatch. `daemon.stop` and `oxd --stop` stop an idle daemon and refuse a busy
one. `make install` quits the app, stops oxd, replaces and reopens the app;
`make install-ui` swaps only the app around the installed helpers. The code
goal is met; the make targets have not been run against the real install.

## Departures from the plan

- The fixture's `SessionUpdate`s are the ACP v2 variants the bridges send:
  user, agent message and thought chunks and whole messages, `tool_call_update`
  (pending with title, kind, and raw input; in progress; completed with
  content), `state_update`, `available_commands_update`,
  `config_option_update`, and `session_info_update`. ACP v2 has no
  `tool_call`, `plan`, or `current_mode_update` variant, and the bridges send
  no `plan_update`.
- `sqlite3 .dump` does not write `user_version`, so `schema-1.sql` starts with
  `PRAGMA user_version=1;` added by hand. The rest is the dump unchanged.

## Decisions

- The fixture was generated once by a throwaway test that built a database
  through `Store`'s own API, deserializing hand-written event JSON into
  `EventKind`, so every row is this build's serialization. The generator is
  not kept.
- `session_unseen` stores a fixed name (`finished`/`error`), not JSON, so
  `Unseen`'s serde is not schema; the store doc lists `EventKind`,
  `QueuedPrompt`, and the session command.
- `oxd --stop` treats a refused or missing socket, and a connection that
  closes before answering (a daemon already exiting), as "oxd is not running".
  Other connect errors print and exit 1.
- The busy-session refusal reads `oxd has busy sessions: <name> (<state>), …`,
  sorted.
- The fixture test lists each fixture's expected counts and asserts the set of
  `schema-*` files matches that list, so a new fixture must add its counts.

## Checks run

- `make check` — passed (fmt, clippy, workspace tests, UI checks, scripts).

## Manual verification

1. A copy of the real database (version 0, 95 sessions) opens with the new
   oxd, is stamped version 1, and `oxd --stop` stops it.

   ```sh
   d=$(mktemp -d); sqlite3 ~/.local/share/ox/oxd.sock.db ".backup '$d/oxd.sock.db'"
   target/debug/oxd --socket $d/oxd.sock --idle-exit-ms 600000 &
   sqlite3 $d/oxd.sock.db "PRAGMA user_version; select count(*) from sessions"
   target/debug/oxd --stop --socket $d/oxd.sock; echo $?
   target/debug/oxd --stop --socket $d/oxd.sock; echo $?
   ```

   Observed `ready`, `1`, `95`; the first stop exited 0 with the socket gone;
   the second printed `oxd is not running` and exited 0.

2. `make help` lists `install-ui`; `make -n install-ui` shows the quit,
   replace, and open steps.

## Follow-up work

- The plan's hand checks of `make install` and `make install-ui` on the real
  install were not run: they quit the Ox app this session runs in. The first
  install needs the plan's rollout steps (sessions idle, quit Ox,
  `pkill -f 'Ox.app/Contents/MacOS/oxd'`), since the installed oxd predates
  `daemon.stop`.
