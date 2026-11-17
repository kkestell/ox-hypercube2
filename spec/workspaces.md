# Workspaces and navigation

Users add local project directories as workspaces and browse their sessions in the desktop app's primary sidebar.

## Workflows

### Add or remove a workspace

1. Choose Add Workspace and select a directory. Cancelling the directory chooser makes no change.
2. The workspace appears under its directory's final name. Hovering its heading shows its full path.
3. Open the workspace heading's context menu and choose Remove Workspace to remove an empty workspace. This does not delete its directory.

### Browse sessions

1. Sessions appear under their workspace in creation order; workspaces appear in the order added.
2. Select a session to view its conversation. Each row shows its title, or New session when no title is set, and its current state. Hovering the row shows the session's directory.
3. Archived sessions appear in a separate Archived group with a count. The group starts collapsed and appears only when it contains sessions.
4. The selected session remains selected across a reload of the desktop view. Selecting a resumable exited session follows [resuming a session](sessions.md#resume-a-session).

### Arrange the desktop

- The primary sidebar starts visible. Toggle Primary Side Bar hides or shows it without removing workspaces or sessions.
- Drag the primary sidebar's divider to resize it. Its minimum width is 200 pixels. Resizing reserves at least 200 pixels of width for the main content when the window has enough space.

### Reconnect

- The app connects to the session service when opened, starting it when necessary.
- After losing an established connection, the app reconnects and reloads the workspace list, session list, and selected conversation.
- If a connection attempt fails, the app shows Cannot reach oxd with the reason and offers Retry or Cancel. Retry tries again; Cancel leaves the app without a working connection.
- Operation failures are shown as error dialogs. Discard confirmations and kept-branch notices follow [session management](sessions.md).

## Rules

- A workspace must be an existing directory. Adding a missing path, a non-directory, or an already-added path fails.
- A workspace cannot be removed while any session belongs to it, including exited and archived sessions. Remove Workspace is disabled in that case.
- Removing or hiding a workspace's presentation is not an instruction to delete project files.
- Changes made by another connected client appear in the app's lists.

## Related areas

- [Sessions](sessions.md): starting and managing the sessions shown here.
- [Git tasks](git.md): task creation and branch-status indicators.
