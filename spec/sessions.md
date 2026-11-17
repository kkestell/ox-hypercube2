# Sessions

Sessions let users keep independent agent conversations in their projects, continue them later, or release resources without losing the conversation.

## Workflows

### Create a session

1. Open the workspace's + menu and choose the agent. The desktop offers Claude.
2. Choose Session in Checkout to run in the workspace directory, or Session in Worktree to follow the [isolated Git workflow](git.md#start-a-task).
3. The app selects the new session. It starts in Starting and becomes Idle when the agent is ready, unless the agent reports ongoing work.
4. A failure to start leaves the app's session Exited and reports the failure in its conversation.

The command line also allows choosing a different compatible agent and additional worktree branch options; see [command-line access](cli.md).

### Resume a session

1. Select a non-archived Exited session that previously started successfully, or load it from the command line.
2. The session goes through Starting and resumes with the same identity, workspace, directory, and conversation. Resuming does not duplicate its existing conversation.
3. A resumed agent may report ongoing work; the session remains Running until that work ends.
4. A failed resume leaves the session Exited and available for another attempt.

Resuming is refused while the session's agent is already active, while it is archived, if it never successfully started, or if its directory no longer exists. Resuming also depends on the agent still being able to continue that conversation.

### Leave a session running

- Closing the desktop app, disconnecting a client, or switching to another session does not cancel a running turn or an unanswered request.
- An agent may start another turn without a new user prompt, for example when background work reports back. Its output and requests appear in the same conversation.
- After 15 uninterrupted minutes idle and unviewed, with no pending requests and an explicit report of no background work, Ox parks the session's agent. A conversation subscription, including `oxctl attach`, counts as viewing; merely listing sessions does not.
- Becoming active, receiving a request, reporting background work, or being viewed restarts that idle period. An agent that does not report whether background work exists is not parked.
- Parking changes the session to Exited but keeps its conversation, worktree, and branch. Resume it as above.

### Archive and restore

1. Choose Archive Session on the row, or archive it from the command line.
2. Ox applies the [cleanup safety rules](git.md#protect-work-during-cleanup), stops the agent, removes the session's Ox-managed worktree if any, and keeps its branch and conversation. A session in the workspace directory leaves that directory untouched.
3. The session moves to the workspace's Archived group. Its conversation remains readable, but it cannot take prompts or resume while archived.
4. Choose Unarchive Session to restore its worktree on its retained branch and return it to the active list. Restoring does not start the agent; selecting the session resumes it.

If Git cannot restore the worktree, including when its branch is missing or checked out elsewhere, the operation fails and the session remains archived. Archiving an already-archived session or restoring a non-archived session fails.

### Delete a session

1. Choose Delete Session in the session row's context menu, in the post-merge dialog, or from the command line.
2. Ox applies the [cleanup safety rules](git.md#protect-work-during-cleanup). If unsaved work would be lost, the app lists the reason and offers Cancel or Discard and Delete. Archiving offers the corresponding Discard and Archive action.
3. Deletion stops the agent and its work, removes an Ox-managed worktree, and removes the conversation and session from all clients. It does not delete the workspace directory for a session running there.
4. A retained session branch is named in a Branch kept dialog in the app. Branch retention follows the [branch cleanup rule](git.md#retain-or-delete-the-branch).

The app treats a session another client already deleted as successfully deleted. An operation that fails to remove a worktree leaves the session available, although its agent may already have stopped. Archive and delete wait for worktree startup or a resume already in progress before proceeding.

## Rules

### States

| State | What it means to the user |
| --- | --- |
| Starting | The agent is being opened or resumed. |
| Idle | The agent is available for a prompt; it may still have background work. |
| Running | A turn is in progress. |
| Awaiting permission | At least one permission request is unanswered. |
| Awaiting input | At least one question form, but no permission request, is unanswered. |
| Exited | The agent is not active; the saved conversation can still be read. |

Archiving is separate from these states. The command line displays archived in place of the state for an archived session.

### Status dots

Each session's dot in the sidebar shows what it needs from the user, the first that applies:

| Dot | When |
| --- | --- |
| Orange | A permission request or question awaits an answer. |
| Red | A turn failed, or the agent exited with an error, since the user last viewed the session. |
| Blue | The agent is starting or a turn is in progress. |
| Green | A turn ended since the user last viewed the session. |
| Gray | Nothing needs the user. |

- A turn that ends while the session is in front of the user, in a window in front of other apps, turns nothing red or green. A cancelled turn, a park and an archive turn nothing red or green either.
- Red and green clear when the user views the session in a window in front of other apps. Prompting or answering a request does not clear them.
- Red stays through later turns until it clears; green gives way to blue while the next turn runs.
- A turn that oxd shutting down cuts off turns red.
- Runs and their steps follow [workflow status dots](../docs/workflows.md#running-a-workflow).

### History and service restarts

- Conversations, including reported tool output and images, remain readable until the session is deleted. Reading history does not require a running agent.
- Workspaces, session identities, titles, archive status, unseen finishes and errors (red and green dots), queued messages, and worktree associations survive a session-service restart. Queued messages belong to the session, and the session service sends them as turns end. Agents do not: saved sessions return Exited and must be resumed to continue work.
- Graceful service shutdown stops active agents and retains their sessions. If a crash leaves a turn unfinished, the recovered conversation ends it with oxd stopped during the turn and cancels unanswered requests. The most recent output may be lost after an abrupt crash.
- A session that never successfully started is cleaned up after a service restart. If its worktree cannot safely be removed, it is retained for deletion rather than resumed.
- Agent-specific retention may prevent continuing a conversation that Ox can still display; see [Claude Code continuation](claude.md#continue-and-replay-conversations).

## Related areas

- [Conversations](conversations.md): prompts, output, and turn errors.
- [Permissions and questions](requests.md): pending requests and stopping work.
- [Git tasks](git.md): worktree creation, merge, and cleanup safeguards.
