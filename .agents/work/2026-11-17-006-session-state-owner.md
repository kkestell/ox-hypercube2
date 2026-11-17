# Each session owns its state and refuses turns during a branch move

## Plan

`.agents/plans/2026-11-17-006-session-state-owner.md`

## Summary

Everything about a session that changes now lives in `State` behind one lock. `Session::update` applies the change, starts a queued prompt, notes the unseen outcome, writes the rows the transition recorded with `Store::save_session`, logs the events, and then publishes. Startup restores every saved session with `State::restored`. Sessions in a moving checkout refuse turns and loads themselves, and their queues wait. Workspace removal takes each session's lifecycle guard and runs in its own task. The plan's goal is met, and the three `TODO`s it names are gone.

## Departures from the plan

- `a_worktree_that_cannot_be_added_is_undone` (`crates/oxd/tests/worktrees.rs`) asserted that a never-started session is gone after a restart. The plan did not list it. It now expects the session back as it was before the restart, `exited` with no worktree, which is decision 2.
- The two Structure items in `.agents/todo.md` exist only in the working tree's uncommitted additions, not in `HEAD`. They are checked off in the working tree but not staged. The five other items are checked off and staged without the unrelated uncommitted lines.
- `add_imported` creates a session without knowing about a move under way in its checkout, so it does not refuse loads until the move ends. `CheckoutMove` marks only the sessions present when the move is reserved, as the plan says. This is unlikely in practice, so it carries a `TODO` in `add_imported`.

## Decisions

- `Session::lifecycle()` returns an `OwnedMutexGuard`, so `remove_workspace` can hold the guards of a list of sessions and start over cleanly.
- `remove_workspace` checks for an unchanged set of sessions by checking the count of the workspace's sessions and that every guarded session is still registered. A guarded session cannot be deleted while the guard is held.
- `AgentTask.commands` is an `Option` instead of a dead channel for a session that never launched.
- `take_unsaved` keeps the last change of each kind in the order of those last changes.
- `State` pushes `Title` and `Unseen` only when the value changes, and `Config` on adoption only when the agent reports a non-empty config. This matches the old diff-based saves.
- `Session::archive` and `unarchive` return the session's info read under the guard. `Daemon::archive_session` and `unarchive_session` are thin wrappers that build the `Git`.
- `invalid_state` and `storage` in `daemon.rs` are `pub(crate)` so the session's archive checks and `commit` keep the same error messages.

## Checks run

- `make check-rust`: passed, with 402 tests. It covers formatting, clippy with warnings as errors, and workspace tests.
- `cargo fmt --all --check` and `cargo clippy -p oxd` passed after the comment-only `TODO` was added.

## Manual verification

1. Each new and changed integration test was run on its own.

   ```sh
   cargo build -p ox-mcp
   cargo test -p oxd --test worktrees -- --exact removing_a_workspace_waits_for_an_unarchive_under_way
   cargo test -p oxd --test worktrees -- --exact a_queued_prompt_waits_for_a_branch_move_in_its_checkout
   cargo test -p oxd --test daemon -- an_archived_session_that_never_started workspaces_and_sessions_survive_a_restart
   ```

   All passed. The workspace-removal test first hung because `WorkspaceRemove` was still answered inline, which blocked the connection's later listing. Answering it in its own task (plan step 5) fixed it.

2. The new regression tests were checked against the old code by reading it, not by running them. Under the old code, the restarted archived never-started session is deleted, so `info` returns `None`. `queue_add` during a move is refused. `workspace.remove` completes inline during the blocked unarchive, so the listing that follows shows no workspace.

## Follow-up work

- A session imported during a branch move in its checkout does not refuse loads until the move ends. See the `TODO` in `add_imported`.
