# Review of worktree ownership

## Scope and coverage

Reviewed the staged implementation of `.agents/plans/2026-11-15-001-worktree-ownership.md`: `ox-protocol` messages and client, `oxctl compare`, `oxd`'s `daemon.rs`, `session.rs`, `worktree.rs`, `branches.rs`, `store.rs` and `conn.rs`, the `daemon` and `worktrees` integration tests, the UI's listing, session list, Merge button and local merge code, and the changed documentation. Also read the code that depends on the changed types: the desktop core's watch relay in `crates/ox/src/lib.rs`, `remove_workspace`, the `useWorkspaces` reconnect path, and `docs/dev/performance.md`. Checked against task 1 of the brief `.agents/briefs/2026-11-14-001-user-defined-workflows.md` and the plan's decisions.

The change delivers task 1. A worktree is its own row, live object and listing entry. A session refers to it by `worktree_id`. The worktree owns its comparison, removal, restore and branch rule. `Worktree::remove` is the one unsaved-work check for delete, archive and startup cleanup, and New Task works through the existing screens.

Gaps:

- No manual New Task run with a real agent, as plan assumption 4 decided. `make e2e` covers New Task with the fake agent.

## Summary

| ID | Severity | Category      | Fix       | Title                                                                    |
| -- | -------- | ------------- | --------- | ------------------------------------------------------------------------ |
| F1 | Low      | Concurrency   | Not fixed | A failed worktree add unlists the worktree before its session drops it   |
| F2 | Low      | Documentation | Not fixed | Performance doc leaves worktrees out of watch versioning and UI identity |
| F3 | Low      | Comments      | Not fixed | Session's lifecycle lock doc says the session adds its worktree          |

## F1. A failed worktree add unlists the worktree before its session drops it

Low · Concurrency · Not fixed · `crates/oxd/src/daemon.rs:754`

When `worktree::add` fails, `start_in_worktree` removes the worktree from the registry, which signals watches, and only then calls `session.fail_start`, which clears the session's `worktree_id`. A watch that reads the listing between the two steps sends a session whose `worktree_id` names a worktree it does not list. This contradicts the `ListingChange` doc, which says a session's worktree arrives before the session. The next change corrects it, and the UI treats a missing worktree as none, so nothing visible goes wrong.

### Evidence

Traced `start_in_worktree`: `self.update(|registry| registry.worktrees.remove(..))` at line 754 sends `changed`, then `session.fail_start(message)` at line 756 changes the session's info. `listing_changes` reads both under the registry lock, at whatever moment the watch wakes.

### Fix

Not fixed. Calling `session.fail_start` before removing the worktree from the registry would keep the two consistent.

## F2. Performance doc leaves worktrees out of watch versioning and UI identity

Low · Documentation · Not fixed · `docs/dev/performance.md:9`

The Daemon section says a watch sends every workspace and session once, then only those that changed, and that each session counts the changes to its `SessionInfo`. Worktrees are now listing entries with their own version counter, which `Sent.worktrees` tracks in the same way. The UI section's list of entries that keep their identity also covers only workspaces and sessions, though `useWorkspaces` now keeps worktree identity too.

### Fix

Not fixed.

## F3. Session's lifecycle lock doc says the session adds its worktree

Low · Comments · Not fixed · `crates/oxd/src/session.rs:56`

The `lifecycle` field doc reads "Held while the session adds its worktree and launches its agent". `Daemon::start_in_worktree` now adds the worktree while holding the session's lifecycle guard, and the session only launches its agent.

### Fix

Not fixed.

## Checks run

- `make check`: passed (rustfmt, clippy, workspace tests including the `daemon` and `worktrees` integration suites, Biome, tsc, ruff).
- `make e2e`, with `make dev` stopped and the old databases deleted: 11 of 12 spec files passed, including `worktree`, `merge` and `archive`. `stress.e2e.ts` failed on "streams a long reply into a long transcript without stalling" with a 409 ms stall against a 100 ms budget.
- Isolating the stress failure: built HEAD and the staged change in separate temporary worktrees and ran `stress.e2e.ts` alternately. HEAD stalled 22, 19, 20, 391, 22, 21, 21 and 21 ms, failing once. The change stalled 22, 408, 386, 20, 20, 468, 18, 22, 21 and 21 ms, failing three times. HEAD shows the same intermittent stall of about 400 ms, and the stress workspace has no worktree sessions, so the change does not cause the failure.
- `make smoke-claude`: passed.
- `make smoke-pi`: passed.

## Verdict

The change delivers task 1 of the brief as planned, with no High or Medium findings. F1 is a brief listing inconsistency after a failed worktree add, and F2 and F3 are stale documentation and comments. All three are recorded and left unfixed. The stress spec's streaming stall is intermittent and also occurs at HEAD.
