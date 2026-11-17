# Review: 2026-11-12-010-archive-sessions

## Round 1

- Task: `2026-11-12-010-archive-sessions`
- Independence: I did not write the implementation; this was an independent review by session `review-010`.
- Reviewed change: `/Users/kyle/projects/ox`, branch `main`, the original uncommitted `git diff HEAD` above `ced1463`; plan `.agents/plans/2026-11-12-010-archive-sessions.md`; work log `.agents/work/2026-11-12-010-archive-sessions.md`.
- Scope: ordinary sequential archive, history, unarchive, load, prompt, delete, and graceful-restart paths; concurrent requests and crash windows excluded as requested.
- Recovery: the review and reproduction below completed before the server restart. This record was recovered on November 17 after the checkout advanced to `f2baeb8`; cited lines and verdict describe the original snapshot, not the current working tree. The original plan and work log are no longer present in this checkout.

### Verdict

changes-required: archiving a session whose agent failed to start reports success, but normal daemon startup subsequently deletes that archive and can delete the branch archive promised to keep.

### Findings

1. **R1.1 — P2: Preserve archived failed-start sessions during startup cleanup.** `crates/oxd/src/daemon.rs:78–86`, interacting with the new archive operation at `crates/oxd/src/daemon.rs:584–592`.
   Create a session whose agent fails during initialization, archive the resulting exited session, then close and reopen the app after the daemon exits. Archive succeeds even though `agent_session_id` is absent, but startup deletes such a session immediately when it has no worktree (`79–81`), or schedules it for cleanup when it has one (`84–86`). That cleanup applies the deletion branch rule and forgets the session (`118–127`, `632–635`), deleting the archive's events and potentially its branch. This violates the plan's explicit support for archiving never-started sessions and its branch-retention guarantee. Exclude archived sessions from both automatic-deletion paths and add a regression covering restart with and without a worktree.

### Verification

- Reused: `make check-rust`, passed, from the original work log's Verification table.
- Reused: `make check-scripts`, passed, from the same table.
- Reused: `make smoke-claude`, passed, from the same table; reported archive-history and unarchive-continuation checks.
- Run: `python3 - <<'PY' … PY` (inline, isolated protocol reproduction against the then-current `target/debug/oxd`). Needed to resolve whether startup cleanup actually removes an otherwise successfully archived session. Created a temporary repository, created one failed-start workspace session and one failed-start worktree session using `/nonexistent/review-agent`, waited for each `agent_exited` event, archived each, then gracefully stopped and restarted the daemon. Before restart both appeared with `archived: true`, and branch `ox/0318f5d2` existed; afterward the session listing was empty, unarchive returned `NOT_FOUND`, and the branch was absent. Artifacts: `/private/var/folders/7p/jvwxkftn4fd31tzmkdphttn00000gn/T/opencode/ar-6cw3luey`. The first attempt stopped before restart because the reproduction incorrectly supplied parameters to the unit-variant `session.list`; correcting the request enabled the completed reproduction above.
- No code was changed, staged, or committed. No broad checks, smoke checks, or e2e checks were rerun. The existing tests cover the started-session round trip, unsaved-work refusal/discard, interrupted turns, Git restoration refusals, and archived deletion; they do not cover the failed-start archive/restart path.

### Limitations

- This is a historical review of the original unit-010 snapshot, not a review of later commits or today's unrelated changes.
- The original work log disclosed that tests were not run with the implementation undone; no such mutation was attempted during this read-only review.
