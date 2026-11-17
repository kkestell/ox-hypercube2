# Review: session state ownership and branch-move coordination

## Scope and coverage

Reviewed the staged changes for
`.agents/plans/2026-11-17-006-session-state-owner.md`, including the daemon,
session state machine, session lifecycle, row persistence, connection dispatch,
regression tests, and associated documentation. The plan links no brief; its
goal, decisions, and explicitly changed behavior define the review's requirements.
The work log is `.agents/work/2026-11-17-006-session-state-owner.md`.

Traced the callers and dependencies around agent startup and resume, queued and
peer prompts, checkout reservations and their release, focus and subscriptions,
parking, archive/unarchive/delete, workspace removal, listing snapshots, and
stored history recovery. Checked that session transitions record their row
changes, saves precede publication, and lifecycle waiters reject closed sessions.
The staged task-list changes were reviewed without including its unrelated
unstaged additions or the unstaged historical-review edit.

Coverage limits:

- The failed-worktree regression asserts the reference invariant on every
  listing batch it observes; it does not force the old publication interleaving.
  The registry/state lock ordering was also checked by inspection.
- Real-agent smoke checks and desktop end-to-end checks were not run. This
  change preserves the agent launch arguments, ACP exchanges, and desktop
  protocol; the state and lifecycle behavior is covered with the fake agent
  and real Git integration tests.
- The documented case of importing a session during a checkout move remains
  deferred by the local `TODO` in `add_imported`, as recorded in the work log.
  Event-log restructuring and the other deferred daemon bugs are outside scope.

## Summary

No findings.

The implementation follows the plan's approved behavior changes. Never-started
sessions retain their saved history across restart; moving sessions refuse
immediate prompts and loads while holding queued prompts; workspace removal
waits for the sessions' lifecycle operations and rechecks membership and archive
state; failed worktree creation clears the session's worktree reference in the
same registry update that removes the worktree.

## Checks run

- `make check-rust`: passed. Formatting, workspace clippy with warnings as
  errors, and all 402 Rust tests passed. This includes the new state/row-change
  tests and the restart, branch-move queue, workspace-removal/unarchive, and
  failed-worktree listing regressions.
- `git diff --cached --check`: passed.
- Inspected the complete staged test diff and its synchronization. New waits
  observe agent exit, Git reaching a FIFO, request/listing responses, a queued
  prompt's start, or listing batches through the failed session's exit. No new
  wait relies on a fixed delay.

## Verdict

No fixes are required. The staged implementation is consistent with the plan,
with the coverage limits and deferred case stated above.
