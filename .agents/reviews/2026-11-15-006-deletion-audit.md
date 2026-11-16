# Review of code to delete or simplify

## Scope and coverage

The whole codebase, reviewed for code that can be deleted or simplified: `crates/oxd`, `crates/claude-code-acp`, `crates/pi-acp`, `crates/ox`, `crates/ox-protocol`, `crates/oxctl`, `crates/ox-mcp`, `ui/src`, `ui/e2e` and `scripts/`. Four agents audited the daemon, the ACP bridges, the UI and desktop core, and the test suites. Each checked its findings by reading the code and searching for callers. No brief applies.

An earlier pass found 145 lines of smaller leftovers in the same areas. Those are fixed in the working tree:
- `Kind::Unknown` in the Claude bridge's schedules
- `turn_ended`'s unused return value
- `Run::set_step`'s state argument
- `Client::daemon_version`
- the triplicated diff in `listing_changes`
- the duplicate status update test
- the `.step` CSS, unused tokens, command types and exports
- the UI prop and guard duplication
- two stale docs

The findings below are what remains.

Gaps:

- The findings have not been checked a second time against the code. Line numbers are approximate and may have moved.
- Line counts are estimates.
- Items that need a fresh database require deleting existing dev and user databases, because the schema has no migrations.

## Summary

| ID  | Severity | Category     | Fix            | Title                                                                  |
| --- | -------- | ------------ | -------------- | ---------------------------------------------------------------------- |
| F1  | Medium   | Dead code    | Not fixed      | The removed branch picker's daemon and protocol support remains        |
| F2  | Medium   | Simplicity   | Not fixed      | Run steps and commands are split across two tables and four writers   |
| F3  | Medium   | Testing      | Not fixed      | oxd integration tests copy their helpers between files                 |
| F4  | Medium   | Architecture | Not fixed      | The two bridges duplicate their wiring, pipes, store and test harness  |
| F5  | Medium   | Dead code    | Needs decision | The Claude bridge lists and imports sessions oxd never asks for        |
| F6  | Medium   | Simplicity   | Needs decision | The bridges share a locked registry that one process per session never needs |
| F7  | Medium   | Simplicity   | Needs decision | The daemon checks form answers the UI already constrains               |
| F8  | Medium   | Dead code    | Needs decision | Both bridges implement `session/close`, which oxd never sends          |
| F9  | Medium   | Simplicity   | Needs decision | Startup cleanup of never-started worktrees                             |
| F10 | Low      | Testing      | Not fixed      | Duplicate tests in oxd, the client, smoke and e2e                      |
| F11 | Low      | Dead code    | Needs decision | Both bridges fall back for clients without forms                       |
| F12 | Low      | Dead code    | Needs decision | The form renders field types no bundled agent produces                 |
| F13 | Low      | Simplicity   | Needs decision | The bridges keep maps of sessions while each process runs one          |
| F14 | Low      | Simplicity   | Not fixed      | Repeated session registration, git runners and constructors in oxd     |
| F15 | Low      | Simplicity   | Not fixed      | Three identical stream types in the client and three stream maps in the core |
| F16 | Low      | Dead code    | Needs decision | The workbench's bottom panel has never had content                     |
| F17 | Low      | Simplicity   | Not fixed      | Duplicated derivations in the UI                                       |
| F18 | Low      | Simplicity   | Not fixed      | `session.list` and `workspace.list` serve only oxctl                   |

Pure simplifications total about 1,500 lines. Behaviour changes total about 1,500 more.

## F1. The removed branch picker's daemon and protocol support remains

Medium · Dead code · Not fixed · `crates/oxd/src/worktree.rs`, `crates/oxd/src/branches.rs`, `crates/oxd/src/daemon.rs`, `crates/ox-protocol/src/messages.rs`

Commit `12f5760` removed the existing-branch and new-branch dialogs and `useBranches.ts`. Commit `b2e37d2` removed the oxctl flags that used them. The protocol, client and daemon support stayed, along with about 12 tests that exercise only these paths. About 700–900 lines.

What stays without a caller:

- `Checkout::Worktree.branch` and `WorktreeBranch`. The UI and oxctl always send `branch: None`. With them go `resolve_existing`, `check_new`, `check_branch_name`, start-point handling, upstream tracking setup in `create_branch` and `roll_back`, and every `Kind` but New. `created_branch` would always be true.
- In `workspace.branches`: `fetch`, `local`, `remote`, `fetch_error` and `LocalBranch`. The app calls it with `fetch: false` (`crates/ox/src/lib.rs:255`) and reads only `head` (`SessionList.tsx` `newTask`). Removing them also removes `branches::fetch`, `FETCH_TIMEOUT`, `Daemon::fetch`/`fetched`, the `fetches` map and `FetchOutcome`. `branches::list` shrinks to resolving `head`.
- `BranchMove.force` and `Client::move_branch`. The app calls only `fast_forward_branch` with `force: false`.
- `worktree.set_compare`: `Client::set_compare`, the daemon handler, `Store::set_compare` and `Worktree::set_compare`.

### Evidence

No caller in `ui/`, `crates/ox`, `crates/oxctl`, `crates/ox-mcp`, `ui/e2e` or `scripts/smoke-acp.py`. The oxd audit and the UI audit found this independently. Commit `12f5760`'s message says "nothing in the app fetches any more".

### Fix

Delete the listed code and the tests that cover only it. Among those tests are `a_branch_only_on_a_remote_is_tracked`, `requests_for_one_repository_share_its_fetch`, the forced move test and the fetch comparison tests, with their helpers `clone_with_remote`, `on_existing` and `fetches`. Make `source_branch` required in `branch.move`. Drop the existing-branch store column. Update the "except one on an existing local branch" sentence in `docs/worktrees.md`.

## F2. Run steps and commands are split across two tables and four writers

Medium · Simplicity · Not fixed · `crates/oxd/src/store.rs`, `crates/oxd/src/run.rs`

Steps and commands have their own tables, `run_steps` and `run_commands`. Four field-level writers save them: `save_command`, `set_run_state`, `start_step` and `set_step`. On top of those sit:
- a three-level read in `runs()`
- SQL in `interrupt_runs`
- a `SessionWorktree::Step` variant that writes `run_steps` from inside `add_session`

`run.rs` mirrors each writer with a save-then-update-memory method. About 250–300 lines.

### Fix

Store a run's steps as one `steps` JSON column, as the `events` table stores event kinds, and have `Run::update` write the whole record. Interrupting runs at startup becomes a map in `Daemon::new`. `remove_run` deletes the sessions in the run's worktree, which matches the architecture doc's definition of a child session. Existing databases must be deleted.

## F3. oxd integration tests copy their helpers between files

Medium · Testing · Not fixed · `crates/oxd/tests/daemon.rs`, `crates/oxd/tests/runs.rs`, `crates/oxd/tests/worktrees.rs`

About 250 lines are duplicated:
- `ExitWatch` exists twice with identical bodies (`daemon.rs` ~221–288, `worktrees.rs` ~350–418).
- The daemon start, client and restart code is the same in `runs.rs` ~98–150 and `worktrees.rs` ~125–190, apart from `bin_dir`, `XDG_CONFIG_HOME` and an `env` overlay.
- `git`/`git_command`, `repo`, `next_changes` (3 copies), `refusal` (2), `info`/`listed_session` (3), `text` (2), and the `TIMEOUT`/`OXD`/`FAKE_AGENT` constants are repeated across files.
- Twelve hand-written "subscribe, loop until `TurnEnded`" blocks in `worktrees.rs` and `runs.rs` repeat `daemon.rs`'s `until`.

### Fix

Move the helpers into `crates/oxd/tests/common/mod.rs`, with one daemon builder that takes those options. Mark the module `#![allow(dead_code)]`, since each test binary uses only part of it.

## F4. The two bridges duplicate their wiring, pipes, store and test harness

Medium · Architecture · Not fixed · `crates/claude-code-acp`, `crates/pi-acp`

These parts are near-identical in both crates:
- `main.rs` handler wiring and `respond` (about 120 lines each)
- the pipe helpers `drain`, `Stderr`, `read_stderr`, `read_buffered`, `write_input` and `translate_lines` (about 90 lines each)
- `store_dir_from`
- `validation.rs`
- store `transaction` and `lock`
- first-prompt titles, `invalid` and `store_error`
- the protocol test helpers `send`, `receive`, `response`, `until_method`, `is_state` and `finish` (about 80 lines each)

About 200–250 lines.

### Fix

Move the shared parts into a new workspace crate both bridges depend on. Keep the drain logic as it is; commit `d421e4b` fixed lost output with it.

## F5. The Claude bridge lists and imports sessions oxd never asks for

Medium · Dead code · Needs decision · `crates/claude-code-acp/src/bridge.rs:170`, `crates/claude-code-acp/src/history.rs`, `crates/claude-code-acp/src/store.rs:78`

oxd sends six ACP methods: `initialize`, `session/new`, `session/resume`, `session/prompt`, `session/set_config_option` and `session/cancel`. It only resumes IDs its own `session/new` returned. The bridge still implements `session/list`, with cursor pagination, and resumes native Claude sessions it never created, through the `history.find` branch in `open`. Supporting this needs:
- `History::list`
- the title scan in `read_entry`
- `HistoryEntry`'s title and `updated_at`
- `Store::list`
- `Session.updated_at` and `touch`
- the `updated_at` session info update in `turn_recorder`
- the `base64` dependency

All of it arrived in the initial commit `c392317`. About 330 lines.

### Fix

Delete it with the tests `lists_and_resumes_claude_history`, `listed_sessions_can_be_loaded` and the `session/list` part of `an_unusable_store_fails_session_requests`. Rewrite `resume_replays_a_claude_session_from_its_transcript` to create its session through `session/new`. Update the `session/list` row in `docs/dev/acp-v2.md`.

## F6. The bridges share a locked registry that one process per session never needs

Medium · Simplicity · Needs decision · `crates/claude-code-acp/src/store.rs`, `crates/pi-acp/src/store.rs`

Both bridges keep every session in one `acp-sessions.json`, under an exclusive file lock with read-modify-write transactions. The Claude store also moves a corrupt registry aside (`store.rs:165–190`) and caches MCP servers per process (`mcp_servers`, `with_servers`). oxd runs one bridge process per session, so a `<dir>/<id>.json` written atomically needs no lock or transaction. The MCP servers can be a field on the bridge. Corrupt-registry recovery came with `c392317`, with no observed failure behind it. About 250 lines.

### Fix

Write one file per session. Delete `bridges_sharing_a_store_keep_each_others_sessions`, `a_corrupt_store_is_set_aside`, `unreadable_store_entries_survive_updates`, most of `failed_registry_reads_and_writes_fail_session_new`, and Pi's `changes_retain_other_sessions_across_stores`. Sessions in an existing `acp-sessions.json` are no longer found.

## F7. The daemon checks form answers the UI already constrains

Medium · Simplicity · Needs decision · `crates/oxd/src/state.rs`

`check_content`, `check_value`, `check_range` and `kind_name` (about 165 lines) check a form answer against its schema. The `Respond` branch and the test `content_must_fit_the_schema` (80 lines) go with them. `Elicitation.tsx` already enforces required fields, lengths, minimums and `maxItems`. Only `oxctl respond` could send an answer that breaks the schema, and the agent would then receive it unchecked. About 250 lines.

### Fix

Delete the content check and keep `check_form`, which refuses forms the UI cannot render. Reword `docs/dev/acp-v2.md:63`.

## F8. Both bridges implement `session/close`, which oxd never sends

Medium · Dead code · Needs decision · `crates/claude-code-acp/src/bridge.rs:282`, `crates/claude-code-acp/src/runner.rs`, `crates/pi-acp/src/pi.rs:203`

oxd ends a session by killing its process group, as `docs/dev/acp-v2.md` says. Code that exists only for `session/close`:
- **Claude bridge:**
  - the handler, `close_session`, `Claude::stop`, and `closing` with its branches (`runner.rs:143, 287, 452–480`)
  - the `active` set (`bridge.rs:51`) and the filter in `require`, which only refuse sessions this connection never opened
- **Pi bridge:** the handler, `close_session`, `Pi::stop` and `closing` (`pi.rs:53, 293, 317`)

About 200 lines.

### Fix

Delete it with `close_stops_claude_and_the_session`, `closing_a_session_stops_pi` and the close half of `replacement_bridge_and_close_retain_native_identity`. Update the `session/close` row in `docs/dev/acp-v2.md`.

## F9. Startup cleanup of never-started worktrees

Medium · Simplicity · Needs decision · `crates/oxd/src/daemon.rs`

At startup oxd removes worktrees in which no agent ever started. The code involved is `Daemon::clean_up`, `unstarted` and `Unstarted`, `Registry.cleaning` and its idle check, the restore branch in `Daemon::new`, and the spawn in `main.rs`. In practice the cleanup covers sessions whose agent failed to start, which the tests build with a missing command. Without it, those sessions stay listed as exited with their error until the user deletes them. `docs/dev/architecture.md` documents the cleanup. About 200 lines.

### Fix

Delete it with the tests at `worktrees.rs` ~1976 and ~2026, the `unstarted` helper and the two tests around ~2125–2175. Update `docs/dev/architecture.md`.

## F10. Duplicate tests in oxd, the client, smoke and e2e

Low · Testing · Not fixed · `crates/oxd/tests`, `crates/ox-protocol/src/client.rs`, `scripts/smoke-acp.py`, `ui/e2e/specs`

About 290 lines:

- **`daemon.rs`:**
  - Delete `turn_continues_after_its_client_disconnects` and keep `reattach_mid_turn`, adding its stop reason assertion.
  - Fold `a_deleted_session_is_not_saved` into `a_deleted_session_loses_its_history`.
  - Fold `agent_that_cannot_start` into `only_an_exited_session_that_started_can_be_loaded`.
  - Fold `watch_starts_with_the_workspaces_and_sessions` into `watch_sends_only_sessions_whose_info_changed`.
  - Make `decline_reaches_the_agent` a case of `cancel_withdraws_an_elicitation`.
- **`runs.rs`:**
  - Delete `a_command_running_at_a_restart_is_interrupted`, covered by `runs_survive_a_restart_and_running_ones_are_interrupted` and the store's interruption test.
  - Delete `the_daemon_lists_its_workflows`, covered by `workflow.rs` and the New Task e2e test.
- **`worktrees.rs`:**
  - Fold `an_existing_branch_that_is_not_there_is_refused` and `a_new_branch_is_refused_when_its_name_is_taken` into the refusal table in `a_request_git_cannot_satisfy_is_refused_with_nothing_created`, unless F1 deletes them.
  - Delete `uncommitted_changes_stay_in_the_workspace`, which tests `git worktree add`.
- **`client.rs`:** `dropping_a_watch_unsubscribes` and `dropping_a_directory_watch_unsubscribes` test the same `Drop for Unsubscribe` as `file_snapshots_route_and_drop_unsubscribes`.
- **`smoke-acp.py`:** `check_archive` repeats `check_load_after_restart` for the bridge, and `archiving_removes_the_worktree_and_keeps_the_branch_and_the_conversation` for oxd.
- **e2e:**
  - The repair test in `run.e2e.ts` repeats `runs.rs`'s `a_repair_turn_can_fix_a_failed_postcondition`.
  - "stops a running turn" in `session.e2e.ts` is part of "shows the agent's reply while the turn runs".
  - "has no Archived group until a session is archived" in `archive.e2e.ts` belongs at the start of the next test, which has the same setup.

### Fix

Delete or merge each pair as listed.

## F11. Both bridges fall back for clients without forms

Low · Dead code · Needs decision · `crates/claude-code-acp/src/runner.rs:185`, `crates/claude-code-acp/src/updates.rs:578`, `crates/pi-acp/src/pi.rs:374`

oxd always advertises `elicitation.form`. The bridges still have code for clients that can't show forms:
- a `forms` flag and capability check in each `bridge.rs`
- `StartRequest.ask_user_question` with `--disallowedTools AskUserQuestion`
- the `!self.forms` branch in the Claude bridge's updates
- Pi's `Updates.forms`, `forms()` and its `.then()` gate

About 110 lines.

### Fix

Delete it with `clients_without_forms_disallow_ask_user_question` and its `claude_arguments` helper, `clients_without_forms_get_no_dialogs` and `session_with_capabilities`. Update the `elicitation.form` caveat in `docs/dev/acp-v2.md`.

## F12. The form renders field types no bundled agent produces

Low · Dead code · Needs decision · `ui/src/features/composer/Elicitation.tsx`, `ui/src/lib/protocol.ts`

The bundled forms come from `claude-code-acp/src/questions.rs` and `pi-acp/src/dialogs.rs`. Between them they produce only three field kinds: string choices (`oneOf`), text with a default, and multi-select (`anyOf`). The form also handles fields nothing produces:
- number, integer and boolean controls
- `format` inputs, with `inputTypes` and `localDateTime`
- `pattern` checking (`patternError`)
- `minLength` and `maxLength`
- plain `enum` choices

All of it came with the first form commit, `a542b25`. `docs/dev/acp-v2.md:63` also says patterns are not validated, but the UI validates them. About 100 lines, plus fake-agent and e2e coverage.

### Fix

Delete the unused controls and their `Field` variants, and shrink the fake agent's form and the e2e constraint test to the three kinds. Correct `acp-v2.md`. F7's decision applies to the daemon's check of the same fields.

## F13. The bridges keep maps of sessions while each process runs one

Low · Simplicity · Needs decision · `crates/claude-code-acp/src/bridge.rs`, `crates/pi-acp/src/bridge.rs`

The Claude bridge holds a `claudes` map and an `active` set, and Pi holds a `sessions` map. Each could be one `Mutex<Option<…>>` holding the open session. That removes the repeated `session_not_found` lookups and the `claudes.remove(id)` on a repeated resume (`bridge.rs:258`). `docs/dev/architecture.md` already says one bridge process runs per session. A second `session/new` on the same connection would be refused. About 60 lines.

### Fix

Hold one session per bridge process.

## F14. Repeated session registration, git runners and constructors in oxd

Low · Simplicity · Not fixed · `crates/oxd/src/daemon.rs`, `crates/oxd/src/run.rs`, `crates/oxd/src/worktree.rs`, `crates/oxd/src/session.rs`

About 110 lines across five repetitions:
- `Daemon::add_session` and `add_step_session` each build a `SavedSession`, save it, call `Session::create` and register it.
- `new_session` and `start_run` repeat plan, `create_branch`, and roll back if not saved.
- `start_in_worktree` and `run::advance` repeat add, `roll_back`, and building the error message.
- `Repository::text`, `commit`, `run` and `run_with_input` copy `Git`'s methods with `--git-dir` prepended.
- `Session::create`, `restore` and `new` pass the same six arguments through.
- Claude's `config.rs` has five unit tests (53 lines) pinning `XDG_DATA_HOME` and temp-dir handling that oxd never relies on.

### Fix

Write one registration helper, one worktree plan-and-rollback helper, and one `Session` constructor taking a built `State`. Give `Repository` a `Git` plus a fixed argument prefix. Reduce the config tests to one.

## F15. Three identical stream types in the client and three stream maps in the core

Low · Simplicity · Not fixed · `crates/ox-protocol/src/client.rs`, `crates/ox/src/lib.rs`, `crates/ox/src/files.rs`

In the client, `Watch`, `DirectoryWatch` and `FileWatch` are the same struct (a receiver plus `_unsubscribe`) with the same `next()`. `watch`, `watch_directory` and `watch_file` repeat the same register-then-send body. In the desktop core, `App.views`, `App.directories` and `App.files` are each a `HashMap<u32, Task>` keyed by channel ID. `unview_session`, `unwatch_directory` and `unwatch_file` differ only in which map they use. About 80 lines.

### Fix

One generic `Stream<T>` with one private opening helper, keeping `Subscription` for its `SessionClosed` handling. In the core, one `streams` map and one `unwatch(channel_id)` command, with `files.rs` folded into `lib.rs`.

## F16. The workbench's bottom panel has never had content

Low · Dead code · Needs decision · `ui/src/components/Workbench.tsx`, `ui/src/styles/layout.css`

`<section className="panel bottom">` has been empty since the first commit, `eaf2b6d`. It comes with:
- a toggle
- a horizontal sash
- `minContentHeight`
- the `center` ref
- the bottom clamp branch
- the `.panel.bottom`, `.sash.horizontal` and `body.resizing.horizontal` styles

No doc or test mentions it. About 35 lines.

### Fix

Delete the panel and its toggle, sash and styles.

## F17. Duplicated derivations in the UI

Low · Simplicity · Not fixed · `ui/src/App.tsx`, `ui/src/features/sessions/SessionList.tsx`, `ui/src/features/composer/Composer.tsx`, `ui/src/features/runs/RunView.tsx`

Two things are worked out in more than one place:
- A session's parent run is computed separately in `App.tsx` (`parent`) and in `SessionList.tsx` (`runOfWorktree`, `children`).
- The open permission request or form, rendered inside `.composer .stack`, appears in both `Composer.tsx` and `RunView.tsx`.

About 30 lines.

### Fix

Compute the parent run once in `useWorkspaces`, or have the daemon put a `run_id` on `SessionInfo`. Render the request through one component.

## F18. `session.list` and `workspace.list` serve only oxctl

Low · Simplicity · Not fixed · `crates/ox-protocol/src/messages.rs`, `crates/ox-protocol/src/client.rs`, `crates/oxd/src/daemon.rs`, `crates/oxctl/src/main.rs`

Only oxctl's `ls`, `compare` and `workspace ls` use them. `oxctl run ls` already reads the first `watch` batch. About 40 lines.

### Fix

Have the three commands read the first `watch` batch, and delete both methods.

## Checks run

- The four audits checked callers with `grep` across `crates/`, `ui/src`, `ui/e2e` and `scripts/`, and traced history with `git log -S`.
- They checked these areas and recommend keeping them:
  - bridge replay, used by oxd's resume
  - the bridge drain machinery, which fixed an observed bug
  - Pi dialog timeouts
  - fake-agent directives, all of which some test uses
  - `messages.rs` wire-format tests, the only Rust guard on the JSON the UI reads
  - the `state.rs` and `store.rs` unit tests, which reach cases the integration tests cannot
  - text coalescing in the core and in the UI, which `performance.md` requires
  - background file viewers, which `architecture.md` requires
- After the earlier pass's fixes: `make check` passed and `make e2e` passed 13 of 13 spec files.

## Verdict

About 3,000 lines, roughly 7% of the codebase, can be deleted or simplified. F1 alone is 700–900 lines of unreachable branch-picker support. F1–F4, F10, F14, F15, F17 and F18 change nothing a user sees. F5–F9, F11–F13 and F16 drop behaviour no current client uses, or change a documented behaviour, and each needs a decision before it is fixed.
