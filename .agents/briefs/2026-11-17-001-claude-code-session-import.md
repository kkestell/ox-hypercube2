# Claude Code session import

## Problem

Claude Code sessions run outside Ox are invisible in Ox. When a user adds a workspace, the conversations they already had with Claude Code in that directory are missing, and so are the ones they later run in a terminal.

## Outcome

- Adding a workspace imports the Claude Code sessions that were started in the workspace directory. They appear as archived sessions with their full conversation history, readable without unarchiving.
- Each workspace has an "Import Claude Code sessions" action that imports any such sessions not already in Ox. This covers workspaces added before this feature and terminal sessions run since.
- Imported conversations show when each message actually happened, and the sessions sort by when they started.
- The import runs in the background after the workspace is added, newest sessions first, so recent sessions appear quickly.
- An imported session behaves like any other archived Claude session: unarchiving it lets the user continue the conversation.

## Decisions

- **Import with full history.** An imported session without its conversation is worthless. Turned down: importing titles only and recovering history when the session is unarchived.
- **Read the transcripts directly.** The import reads Claude's transcript files with the transcript reading Ox already does and never starts Claude, so it stays fast.
- **Import on add, plus a manual action.** Existing workspaces get the import, and the user chooses when to pull in newer terminal sessions. Turned down: importing only on add, which leaves existing workspaces out, and keeping in sync automatically, which brings in terminal sessions unasked and adds machinery.
- **Never re-import deleted sessions.** Ox remembers the sessions the user deleted, so deletion stays permanent. Turned down: re-importing them, which brings every deleted session back on the next import.
- **Skip headless runs.** Only interactive sessions are imported. Automated `claude -p` runs carry no conversation worth keeping and can vastly outnumber real sessions. Turned down: importing everything.
- **Keep the original times.** Imported events carry the times recorded in the transcript. Turned down: stamping them with the import time, which shows every message at the same moment.
- **Only sessions started in the workspace directory.** These resume exactly as Ox sessions do. Turned down: including subdirectories, which would need a working directory Ox sessions do not use today.

## Out of scope

- Pi sessions.
- Sessions started in subdirectories of the workspace or in other checkouts.
- Handling very large numbers of sessions in one workspace. The to-do list records the daemon and client limits to revisit.
