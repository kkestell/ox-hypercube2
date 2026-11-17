# Claude Code session import

## Plan

`agents/plans/2026-11-17-002-claude-code-session-import.md`

## Summary

Adding a workspace in the app imports the interactive Claude Code sessions started in its directory, and the workspace heading's Import Claude Code Sessions action imports any new ones and reports the count. Imported sessions are archived, keep their transcript times, and resume without replay once unarchived. The plan's goal is met.

## Departures from the plan

- `oxd::import` runs the bridge through a new `ox_protocol::bridge_mode`, which `describe` now also uses, in place of a second copy of `describe`'s process handling.
- The resume test checks that the fake agent's replay is missing from the history, not that `--no-resume-replay` rejected a request. An imported session's command is the bare bridge path, so the test cannot pass the fake agent a flag. The test seeds `fake-agent-a.jsonl` with a line that a replay would record before `agent_started`. With the old load rule restored, the test fails.
- `Updates::take_listed` and the connection accessor `panic!` with the plan's messages from a `match` in place of `expect`, because `Output` is an enum, not an `Option`.

## Decisions

- `History::sessions` returns the first `timestamp` as text, and `sessions::sessions` parses it. A transcript whose head has no timestamp is left out, with a `TODO`.
- In `--transcript`, an entry that comes before any timestamped entry takes the first timestamp in the transcript.
- `Store::import_session` inserts the `archived` row only when `SavedSession::archived` is set, so the flag means what it says.
- `Session::agent_session_id()` was added for the known-session check.
- The fake agent's `--sessions` lists the fixture in file order, and a missing `fake-transcripts.json` panics.

## Checks run

- `make check-rust`: passed.
- `make check-ui`: passed.
- `make e2e`: passed, 2 spec files.
- `make smoke-claude`: passed.

## Manual verification

1. Ran the bridge modes against this machine's real transcripts.

   ```sh
   target/debug/claude-code-acp --sessions "$PWD"
   target/debug/claude-code-acp --transcript "$PWD" <id>
   target/debug/claude-code-acp --transcript "$PWD" 00000000-0000-0000-0000-000000000000
   ```

   23 sessions were listed newest first in about 1 s. A transcript printed Claude's title and timed updates. The unknown ID exited 1 with `session … not found`.

2. Imported this repository's sessions into a scratch daemon.

   ```sh
   target/debug/oxd --socket /tmp/oxdimport/oxd.sock --idle-exit-ms 60000
   # over the socket: hello, workspace.add <repo>, then workspace.import_claude_sessions twice
   sqlite3 /tmp/oxdimport/oxd.sock.db "select title, created_at from sessions order by created_at desc limit 5"
   ```

   The first import answered `{"imported": 24}` in 2.6 s, and the second answered `{"imported": 0}`. The sessions were archived, carried Claude's titles and start times, and 3084 events were saved.

## Follow-up work

- `spec/workspaces.md` describes an Archived group with a count that starts collapsed. The app instead hides archived sessions behind Show Archived, so imported sessions only show up after Show Archived is chosen.
