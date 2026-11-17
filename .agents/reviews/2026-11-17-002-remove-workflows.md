# Remove workflows

## Scope and coverage

The review covered the uncommitted removal of workflow runs: the daemon's runner, definitions, shell steps and run storage; the run methods and types in `ox-protocol`; the `ox-mcp` child mode; the desktop core's run commands, focus and conversation item times; the sidebar, run view, composer, conversation hook, styles, mocks and tests in the UI; the smoke script; and the docs. It checked against the intent the user gave: workflows deleted entirely, shared code returned to its one remaining use, no run-shaped generalizations, and no stale wording. Event times in the daemon's `events` table stay, because dropping the column would break existing databases. There was no brief or plan. Two independent reviewers read the Rust side and the UI and docs side, and each finding below was confirmed before it was fixed.

Gaps:

- The live checks (`make smoke-claude`, `make smoke-pi`) were not run; they spend paid model requests, and the change only removes `check_workflow` from them.
- The sidebar was not looked at in the running app. The change removes run and step rows and the tree line styles, and simplifies the row height and exited-row dimming rules.

## Summary

| ID  | Severity | Category      | Fix   | Title                                                         |
| --- | -------- | ------------- | ----- | ------------------------------------------------------------- |
| F1  | Medium   | Correctness   | Fixed | Deleting a former run's session fails on an existing database |
| F2  | Low      | Testing       | Fixed | No test asserts a session's removal precedes its worktree's   |
| F3  | Low      | Simplicity    | Fixed | Prompt sends return a sequence number no caller reads         |
| F4  | Low      | Simplicity    | Fixed | Fake agent keeps two directives only run tests used           |
| F5  | Low      | Simplicity    | Fixed | Instructions module reduced to a flag around a constant       |
| F6  | Low      | Simplicity    | Fixed | Helper and visibility kept for removed callers                |
| F7  | Low      | Simplicity    | Fixed | Composer's send condition keeps a run-only case               |
| F8  | Low      | Simplicity    | Fixed | Menu styles for workflow items match nothing                  |
| F9  | Low      | Comments      | Fixed | Comments still describe runs                                  |
| F10 | Low      | Documentation | Fixed | Docs still describe runs, and edited text is left unwrapped   |

## F1. Deleting a former run's session fails on an existing database

Medium · Correctness · Fixed · `crates/oxd/src/store.rs:394`

A database created before this change keeps its `runs` and `run_unseen` tables, whose rows point at the worktree each run's sessions shared. After the change, those sessions load as ordinary sessions in one shared worktree. Deleting one removes the worktree from disk, then `remove_session` deletes the worktree row, which the `runs` row still references, so the delete fails on the foreign key. The other sessions of that run are left without a checkout. The user's database holds two such runs with four sessions.

### Evidence

On a copy of the user's database, the statements `remove_session` runs, executed for a run's session with `PRAGMA foreign_keys = ON`, failed with `FOREIGN KEY constraint failed (19)`.

### Fix

The user chose no code change, keeping the store's rule that a schema change has no migrations. Before installing the new build, the user cancels the running run and deletes both runs in the currently installed Ox, which removes their sessions and worktrees through the old code. The empty run tables left behind reference nothing.

## F2. No test asserts a session's removal precedes its worktree's

Low · Testing · Fixed · `crates/oxd/tests/worktrees.rs:414`

`ListingChange` and `Daemon::listing_changes` promise that a batch lists a session's removal before its worktree's. Only the deleted `runs.rs` test asserted removal order.

### Fix

`a_worktree_is_listed_before_its_session_and_removed_with_it` now waits for the batch that removes the session and its worktree and asserts the session's removal comes first.

## F3. Prompt sends return a sequence number no caller reads

Low · Simplicity · Fixed · `crates/oxd/src/session.rs:531`

`Session::prompt`, `queue_send` and `send_prompt` returned the seq of the event that records the prompt, which the runner used to wait for its step's turn. Both remaining callers discard it.

### Fix

They return `Result<(), ErrorBody>`.

## F4. Fake agent keeps two directives only run tests used

Low · Simplicity · Fixed · `crates/oxd/src/bin/oxd-fake-agent.rs`

The `next` directive, added for workflow repair turns, and the `messages` directive were used only by the deleted `runs.rs`.

### Fix

Both directives, their documentation, the `next` field of the agent's state, and the prompt handling that consulted it are removed.

## F5. Instructions module reduced to a flag around a constant

Low · Simplicity · Fixed · `crates/oxd/src/session.rs:35`

Without step roles, `instructions::text(ox_tools)` returned one fixed text when its flag was set.

### Fix

The module is removed. `session.rs` holds the text as `OX_INSTRUCTIONS` and gives it to agents that have the `ox` tools.

## F6. Helper and visibility kept for removed callers

Low · Simplicity · Fixed · `crates/oxd/src/daemon.rs:869`, `crates/oxd/src/worktree.rs:28`

`Daemon::bridge` was extracted so step sessions could share it, and now has one caller. `LOCAL_ENV_VARS` was `pub(crate)` only for `shell.rs`.

### Fix

`bridge` is inlined into `request_session`, and `LOCAL_ENV_VARS` is private.

## F7. Composer's send condition keeps a run-only case

Low · Simplicity · Fixed · `ui/src/features/composer/Composer.tsx:87`

`canSend` was `(idle || (running && !disabled)) && hasContent`. A composer could be running and disabled at once only in a running run's child. `disabled` now means exited, which is never running.

### Fix

`canSend` is `(idle || running) && hasContent`, and the doc comment says so.

## F8. Menu styles for workflow items match nothing

Low · Simplicity · Fixed · `ui/src/styles/controls.css:141`, `ui/src/styles/tokens.css:101`

The run view commit added `.menu-separator`, its `--menu-separator-margin` token, `.menu button:disabled`, and `:enabled` on the menu hover rule, for the workflow items in the New Session menu. No menu has a separator or a disabled item now.

### Fix

The rules and the token are removed, and the hover rule is `.menu button:hover` again.

## F9. Comments still describe runs

Low · Comments · Fixed · several files

- `crates/oxd/src/conn.rs:312`: the watch sent "every workspace, worktree, session and run".
- `crates/oxd/tests/common/mod.rs:78`: the fake bridges were for "steps".
- `crates/oxd/tests/common/mod.rs:88` and `ui/e2e/wdio.conf.ts:6`: `XDG_CONFIG_HOME` was described as a configuration directory, which oxd read for workflows. It stays, because git reads its global ignore and attributes files from there and `GIT_CONFIG_GLOBAL` does not cover them.
- `ui/src/features/files/useFileViewers.ts:13`, `treeState.ts:21` and `viewerState.ts:50`: the views were "each session and each run".

### Fix

Each comment now describes the current code. The two `XDG_CONFIG_HOME` comments name the Git isolation it provides.

## F10. Docs still describe runs, and edited text is left unwrapped

Low · Documentation · Fixed · `docs/dev/performance.md:9`, `docs/dev/architecture.md:170`

`performance.md` still described runs in the watch and "each child of the viewed run" in the desktop core, and `architecture.md` still said a workspace cannot be removed while "runs" belong to it. Several passages this change edited were left with over-long or ragged lines: `architecture.md`'s repository ownership paragraph, and doc comments in `messages.rs`, `store.rs`, `daemon.rs`, `tests/common/mod.rs` and `Composer.tsx`.

### Fix

The passages describe sessions only and are rewrapped.

## Checks run

- `make check-rust`: passed, 356 tests.
- `make check-ui`: passed, 76 tests.
- `make e2e`: passed, 2 spec files.
- `make check-scripts`: passed.
- `cargo test -p oxd --test worktrees a_worktree_is_listed_before_its_session_and_removed_with_it`: passed.
- F1's foreign key failure, reproduced on a copy of the user's database with `sqlite3`.

## Verdict

The removal is complete and introduces no bug in new databases. F1 affects databases that hold runs, and the user resolves it by deleting those runs before upgrading. F2 restores the one contract only run tests asserted. F3 through F10 remove what the runs left behind in shared code, styles, comments and docs.
