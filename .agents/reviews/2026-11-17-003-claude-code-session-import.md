# Claude Code session import

## Scope and coverage

Reviewed the staged changes for `.agents/plans/2026-11-17-002-claude-code-session-import.md`: the bridge's `--sessions` and `--transcript` modes (`crates/claude-code-acp`), `bridge_mode` and the new method in `ox-protocol`, the daemon's import, store, load rule and fake agent (`crates/oxd`), the Tauri command, the session list's menu and add flow, and the spec, architecture and README changes. Callers of the changed code were traced, including workspace removal, session deletion, restore at startup and the listing. The bridge modes were also run against this machine's real transcripts for this repository (24 sessions). Checked against the brief `.agents/briefs/2026-11-17-001-claude-code-session-import.md`.

Gaps:

- `make e2e` and `make smoke-claude` were not rerun after the fixes. The work log reports both passing on the reviewed revision, and the fixes do not touch the bridge or the e2e flows.

## Summary

| ID | Severity | Category      | Fix   | Title                                                          |
| -- | -------- | ------------- | ----- | -------------------------------------------------------------- |
| F1 | Medium   | Intent        | Fixed | Imported sessions keep a workspace from ever being removed     |
| F2 | Low      | Documentation | Fixed | Spec says imported sessions appear while archived ones are hidden |
| F3 | Low      | Documentation | Fixed | Spec gives `startedAt` from the first entry, not the first timed one |

## F1. Imported sessions keep a workspace from ever being removed

Medium · Intent · Fixed · `crates/oxd/src/daemon.rs:578`, `ui/src/features/sessions/SessionList.tsx`

Adding a workspace now imports its Claude Code sessions as archived sessions. Removing a workspace was refused while any session belonged to it, archived ones included, and the menu disabled Remove Workspace for the same reason. Archived sessions are hidden until Show Archived is on. So after adding a directory that has Claude Code sessions, the workspace looked empty, Remove Workspace was disabled, and the only way out was to delete each imported session one at a time. Those deletions were then remembered, so adding the directory back imported nothing. The brief and plan did not address this.

### Evidence

`Daemon::remove_workspace` refused when `Store::workspace_has_sessions` found any row, and `SessionList.tsx` enabled Remove Workspace only when `workspaceSessions.length === 0`, counting archived sessions. `Store::remove_session` records the agent session ID of every deleted session.

### Fix

The user chose to let a workspace whose sessions are all archived be removed after a confirmation that names the count, deleting those sessions without remembering them.

- `Store::remove_workspace` deletes the workspace's sessions (cascading their events, archive rows and queues) and their worktree rows with the workspace in one transaction, without writing `deleted_agent_sessions`. `workspace_has_sessions` became `workspace_has_unarchived_sessions`, which still counts the rows of sessions being created.
- `Daemon::remove_workspace` refuses only while a session that is not archived belongs to the workspace, then drops the removed sessions and their worktrees from the registry and closes them. Branches are kept. A `TODO` names the unlikely case of an archived session being unarchived or deleted during the removal.
- Remove Workspace is enabled when every session is archived. When there are archived sessions, the app asks `Remove this workspace and delete its N archived sessions?` before removing it.
- `workspace.remove`'s doc and `spec/workspaces.md` (the remove step and the rules) describe the new behavior.
- Tests: the store test covers deleting the sessions and worktrees without remembering them, and `workspace_has_unarchived_sessions`. The daemon test now archives the session and removes the workspace. A new import test removes the workspace and imports the session again after adding it back. Two UI tests cover the confirmation and the disabled item.

## F2. Spec says imported sessions appear while archived ones are hidden

Low · Documentation · Fixed · `spec/workspaces.md:15`

The new spec step said each imported session "appears as soon as it is read". Imported sessions are archived, and the app hides archived sessions until Show Archived is on, so nothing appears in the default view.

### Fix

The step now says each session appears as soon as it is read while the workspace shows archived sessions.

## F3. Spec gives `startedAt` from the first entry, not the first timed one

Low · Documentation · Fixed · `spec/claude.md:27`

The spec said `startedAt` is the time of the transcript's first entry. `head` in `crates/claude-code-acp/src/history.rs` takes the first entry that has a `timestamp`, and transcripts often begin with entries that have none.

### Fix

The spec now says the time of the transcript's first entry that has one.

## Checks run

- `crates/claude-code-acp --sessions` and `--transcript` against this repository's real transcripts: 24 sessions, titles present, timestamps monotonic within each transcript.
- `make check-ui`: passed, 80 tests.
- `make check-rust`: passed.

## Verdict

The import delivers the brief's outcome and keeps its decisions. F1 was a real trap in normal use: adding a directory with Claude Code sessions left a workspace that could not be removed. It is fixed as the user decided. F2 and F3 corrected the spec. No other defects were confirmed.
