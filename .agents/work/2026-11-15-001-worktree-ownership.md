# Separate worktree ownership from agent execution

## Plan

`.agents/plans/2026-11-15-001-worktree-ownership.md`

## Summary

A worktree is now its own database row, live `Worktree` object, and listing entry. A session refers to it by `worktree_id`. The worktree owns its comparison, its removal (`Worktree::remove`, the one unsaved-work check for delete, archive, and startup cleanup), restore, and branch rule. `Session` keeps agent execution only, and its agent launches through `Session::start` once `Daemon::start_in_worktree` has added the worktree. Clients compare through `worktree.set_compare` and `worktree.refresh`, and the UI looks worktrees up by ID. The plan's goal is met, except for the dev database step below.

## Departures from the plan

- `SavedWorktree` is flattened to `id`, `path`, `branch`, `base`, `created_branch`, `compare`, and `common_dir` rather than wrapping `WorktreeInfo` and adding `id`. `WorktreeInfo` now carries the live `comparison`, which a saved row does not have.
- The `messages.rs` listing test now covers the `worktree` and `worktree_removed` changes and `SessionInfo.worktree_id`. The plan named only the method serialization tests.
- `a_comparison_is_refreshed_set_and_cleared_on_request` used to check that `session.set_compare` refuses a session with no worktree. That request no longer exists, so the test now checks that `worktree.set_compare` refuses an unknown worktree ID with `NOT_FOUND`.
- `crates/oxd/tests/daemon.rs` folds `worktree` and `worktree_removed` changes in its listing `apply`, because the match must be exhaustive.
- The `oxctl new --worktree` help text now says the branch is named for the worktree.
- Step 10's deletion of `target/dev/oxd.sock.db` did not run. The permission system denied it while `make dev` was running with an `oxd` that had the database open. Until that file and its `-wal` and `-shm` files are deleted, the new `oxd` refuses to start against it under `make dev`.

## Decisions

- `Worktree` publishes a change only when its `WorktreeInfo` changes, as `Session` does, so a recomparison with the same result sends nothing to watches.
- `worktree.set_compare` no longer takes the session's lifecycle lock. A worktree's ref and comparison are independent of its session's agent, and a set during a delete only updates a row that is about to go.
- Worktrees in `Listing` and in a batch of changes come in registry order. Clients key them by ID.
- `oxctl compare` on a session with no worktree prints nothing, as it did before.
- The failed-add integration test gets the branch name from `worktree_id`, because the worktree may leave the listing before a new watch reads it.

## Checks run

- `make check`: passed (rustfmt, clippy `-D warnings`, workspace tests including 64 `worktrees` and 66 `daemon` integration tests, Biome, tsc, ruff).
- `make e2e`: 11 of 12 spec files passed, including `worktree`, `merge`, and `archive`. `stress.e2e.ts` failed on "streams a long reply into a long transcript without stalling": 368 ms against a 100 ms budget, then 385 ms and 404 ms when rerun alone in this checkout. The same sources built in a separate worktree at `/tmp` passed the stress spec three times (21–22 ms). HEAD built there measured 24–36 ms. This checkout differs by the running `make dev` session (vite, and a dev app executing the same `target/debug/ox` path the e2e app uses). The mechanism is not established.
- `make smoke-claude`: passed.
- `make smoke-pi`: failed twice at the native-command-with-arguments check, the first time with the model's answer cut to `OX_NATIVE_COMMAND_`, the second with the model refusing to run the skill. Every daemon check before that step passed both times.

## Manual verification

1. Isolating the stress result from this checkout's environment.

   ```sh
   git worktree add --detach /tmp/ox-baseline HEAD
   ln -s "$PWD/ui/node_modules" /tmp/ox-baseline/ui/node_modules
   # Copy crates/ and ui/src/ from this change, then touch the copied .rs files so cargo rebuilds them.
   cd /tmp/ox-baseline && cargo build -p oxd -p ox-mcp -p oxctl
   cd ui && npm run tauri -- build --debug --no-bundle --features webdriver
   npx wdio run e2e/wdio.conf.ts --spec e2e/specs/stress.e2e.ts
   ```

   The change passed with a 21–22 ms streaming stall. HEAD measured 24–36 ms.

## Follow-up work

- Rerun `make e2e` with `make dev` stopped to confirm the stress spec in this checkout.
- Delete `target/dev/oxd.sock.db`, `oxd.sock.db-wal`, and `oxd.sock.db-shm`, and the default `~/.local/share/ox/oxd.sock.db` on each machine, before running the new `oxd` against them.
