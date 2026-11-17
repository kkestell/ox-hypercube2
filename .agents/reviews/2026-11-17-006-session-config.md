# Ox records each session's config

## Scope and coverage

The staged changes for `.agents/plans/2026-11-17-005-session-config.md`,
checked against the plan, its work log
(`.agents/work/2026-11-17-005-session-config.md`), and the goal that a
session's model, effort and mode are what the user picked: Ox records them,
shows the record in the composer, and tells the agent, and nothing the agent
reports changes what the composer shows. The plan links no brief. Reviewed:
`ox-protocol` (`SessionInfo.config`, `session.set_config`, `Client`), `oxd`
(`session.rs`, `state.rs`, `store.rs` with schema step 2 and `schema-2.sql`,
`daemon.rs`, `conn.rs`, the fake agent and the integration tests), `pi-acp`'s
effort check, `ox-mcp`'s listing, the Tauri command, the composer
(`controls.ts`, `Composer.tsx`, `DraftComposer.tsx`, `App.tsx`) and its tests,
`scripts/smoke-acp.py`, and the architecture and ACP docs. The order in which
two `session.set_config` answers settle was traced through the SDK's ordered
response barrier and single-future task actor.

The implementer's departures hold up. The composer's guard (a change builds on
the config last sent until the session is listed again) is needed because the
model picker stays open for the effort after a model is chosen; its comment
overstated it (F5). The smoke script's `set_option` mirrors the composer
closely enough for a smoke check. The adoption test in `tests/import.rs`
covers the resume path, and `set_config_replaces_the_config_and_the_next_prompt_runs_with_it`
covers a new session adopting the agent's config. Ignoring
`config_option_update`s in the resume-while-busy test is right, since a resume
now sends every setting; the same cause made another resume test flaky (F4).

Proportionality: of the +1056/−724 code lines, the growth comes from what the
plan asks for rather than from waste. The new persisted record accounts for
most of it (the `schema-2.sql` fixture, 93 lines; the store column, migration,
and tests, about 130), with Pi's clamped-effort refusal (37 with its test) and
the composer's config-based options (30). Against that, the change removes the
daemon's option tracking (`ConfigOptionsSet`, `option_values`,
`Resume.options`, the `Live` option handling), the composer's
`startingConfig`, the fake agent's two config flags, and `ox-mcp`'s
`current_value`. The one avoidable addition was a JSON column reader added
beside two copies of the same code (F6); with that and the migration rewrite,
the review's fixes take 26 lines out of `store.rs`.

Gaps:

- `make smoke-claude` and `make smoke-pi` were not run. The staged script
  sends `protocol_version=1`, which `oxd` has refused since 89866ec, as the
  work log records.
- The composer was not exercised in the running app; its behavior was checked
  through the UI tests and `make e2e`.

## Summary

| ID | Severity | Category    | Fix   | Title                                                                     |
| -- | -------- | ----------- | ----- | ------------------------------------------------------------------------- |
| F1 | High     | Performance | Fixed | Schema step 2 takes 40 s on the real database, past the app's 5 s start   |
| F2 | Medium   | Correctness | Fixed | Changing one setting replaces other recorded values the config file lacks |
| F3 | Low      | Correctness | Fixed | An exited session's pickers are enabled and every change fails            |
| F4 | Low      | Testing     | Fixed | `a_held_queue_survives_a_restart` depends on event timing after a resume  |
| F5 | Low      | Comments    | Fixed | The composer guard's comment says it lasts until the config changes       |
| F6 | Low      | Simplicity  | Fixed | `json_value` is added beside two inline copies of the same code           |

## F1. Schema step 2 takes 40 s on the real database, past the app's 5 s start

High · Performance · Fixed · `crates/oxd/src/store.rs:545`

`add_session_config` found each session's latest option report with a
correlated subquery: for every row of `events`, it scanned that row's session
for the greatest seq whose JSON is an `agent_started` or
`config_option_update`. That parses JSON for every pair of events in a
session, quadratic in session length. The migration runs in `Store::open`,
before `oxd` reports ready, and the app waits `START_TIMEOUT` (5 s,
`crates/ox-protocol/src/client.rs:28`) for that report. The first launch after
this change therefore fails with "oxd did not become ready within 5 s" for any
database with long sessions, including the user's own.

### Evidence

On a backup of `~/.local/share/ox/oxd.sock.db` (116 sessions, 20,978 events,
longest session 1,285 events), the staged `oxd` took 40 s to report ready, and
the query alone took 20.7 s in `sqlite3`. `EXPLAIN QUERY PLAN` shows a full
scan of `events` with a correlated subquery per row.

### Fix

The query is now one grouped scan,
`SELECT session_id, kind, MAX(seq) FROM events WHERE <is a report> GROUP BY session_id`,
relying on SQLite taking a bare column from the row that has the `MAX`. On a
fresh backup of the same database, `oxd` reports ready in 0.3 s, and every
session's backfilled config matches what the staged query produced.

## F2. Changing one setting replaces other recorded values the config file lacks

Medium · Correctness · Fixed · `ui/src/features/composer/controls.ts:223`

`changedConfig` built the next config with
`draftOptions(harness, { [configId]: value }, current)`. `draftOptions` keeps a
value only when the config file lists it, and otherwise takes the first listed
choice. A session whose recorded model, effort, or mode the config file no
longer lists therefore lost it whenever another setting changed. A session
recorded at model `other/one` and mode `plan` that changed its mode to `code`
was sent the config file's first model. The composer shows such values on
purpose (`sessionOptions` adds them as their IDs), and they arise whenever the
config file's lists change, as in 8210bf4, or when a session adopts its
agent's defaults. The goal is that the recorded values are what the user
picked.

### Evidence

The new test "keeps recorded settings the config does not list when another
changes" fails on the staged code: the mode change sends a listed model
instead of `other/one`.

### Fix

`changedConfig` now replaces only the changed setting. On a model change, it
still takes the effort the draft rule picks from the new model's levels, or
drops the effort when the model lists none, which is the plan's stated purpose
for using the draft rule. Every other setting keeps its recorded value, and
model, effort, and mode stay in that order. This departs from the plan's
wording (`draftOptions` applied to the whole config) but keeps its intent.

## F3. An exited session's pickers are enabled and every change fails

Low · Correctness · Fixed · `ui/src/features/composer/Composer.tsx:299`

Before this change, an exited session listed no options, because
`info_of` reported `config_options` only while the agent was live. It now
lists its config, so the composer shows the pickers, enabled only when the
session is not `starting`. `Session::set_config` refuses an exited session
(`session is exited`), so every choice made on a session whose agent has
exited shows that error.

### Evidence

The new test "shows an exited session's config disabled" fails on the staged
code.

### Fix

The pickers are disabled while the session is starting or exited, and the
composer's doc comment says so.

## F4. `a_held_queue_survives_a_restart` depends on event timing after a resume

Low · Testing · Fixed · `crates/oxd/tests/daemon.rs:488`

The test takes the first event after the resumed agent starts as the queued
prompt's `prompt_started`. A resume now sends the agent every setting of the
session's config without waiting, and the fake agent reports each with a
`config_option_update`, which can arrive before the turn starts.

### Evidence

The first `make check-rust` run after the fixes failed this test with
`expected prompt_started, got Update(ConfigOptionUpdate(...))`. The test is
outside the code the fixes touched.

### Fix

The test finds the turn's `prompt_started` event instead of taking the first
event. It passed 15 runs on its own and 3 runs of the `daemon`, `agents`, and
`import` suites.

## F5. The composer guard's comment says it lasts until the config changes

Low · Comments · Fixed · `ui/src/features/composer/Composer.tsx:275`

`SessionComposer` builds a change on the config it last sent while
`sent.current.base === session.config`. That is a reference comparison, and
every listing of the session replaces its `SessionInfo`, so the guard ends at
the next listing of anything in the session, not when the config changes. A
change made while another's answer is pending, after a listing that changed
something else, such as the session's state, builds on the listed config and
can undo the pending change. That needs two config changes within one round
trip to the agent, around an unrelated listing.

### Fix

The comment now states when the guard ends and names that case.

## F6. `json_value` is added beside two inline copies of the same code

Low · Simplicity · Fixed · `crates/oxd/src/store.rs:625`

The change adds `json_value` to read the `config` column, while `sessions`
parsed the command and `events` parsed each event with the same inline
`serde_json::from_str` and `FromSqlConversionFailure` mapping, and the new
migration added a third copy.

### Fix

`sessions` reads the command, `events` reads each event, and the migration
reads each report through `json_value`. The migration's report match loses its
error arm.

## Checks run

- `make check-rust`: first run failed `a_held_queue_survives_a_restart` (F4);
  after the fix it passed (fmt, clippy with `-D warnings`, `ox-mcp` build,
  workspace tests).
- `cargo test -p oxd --test daemon --test agents --test import`, three runs:
  passed (80, 12 and 6 tests).
- `cargo test -p oxd --test daemon a_held_queue_survives_a_restart`, 15 runs:
  passed.
- `make check-ui`: passed (88 tests). The two new composer tests failed
  against the staged `controls.ts` and `Composer.tsx` and pass with the fixes.
- `make e2e`: passed (2 spec files).
- Migration on backups of the real database: staged `oxd` ready in 40 s;
  fixed `oxd` ready in 0.3 s at schema version 2, with identical backfilled
  configs.

## Verdict

The change delivers the goal: the session's config is recorded, listed,
shown, sent at start and resume, and no agent report changes it. Its growth
follows from the persisted record and Pi's effort check that the plan asks
for. F1 would have stopped the app from starting after the upgrade and F2
silently changed settings the user had not touched; both are fixed, along
with the exited-session pickers (F3), a timing-dependent test (F4), a
misleading comment (F5) and duplicated column parsing (F6).
