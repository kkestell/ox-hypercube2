# Glossary

Use these terms consistently. See [Architecture](architecture.md) for their
ownership and lifecycle.

- **Workspace**: A directory the user adds to Ox, under which sessions are grouped.
- **Session**: A saved conversation and its execution context, belonging to a
  workspace. A session can outlive its agent process and the daemon.
- **Draft**: A new session the desktop app holds before its first message is
  sent: its workspace, agent, checkout, and chosen option values. Sending the
  first message creates the session.
- **Turn**: One period of agent work within a session, started by a prompt or by
  the agent itself.
- **Queued prompt**: A prompt waiting in a session's queue, which `oxd` keeps.
  The first starts a turn each time the session's agent becomes idle, unless a
  cancelled or failed turn holds the queue.
- **Steered prompt**: A prompt sent during a turn, which the agent takes into
  that turn at its next opportunity. Distinct from a queued prompt, which
  waits for the turn to end.
- **Peer message**: A message one session's agent sends another
  through the `ox` tools, recorded with its sender. It starts a turn or steers
  like a prompt sent now, without resuming a held queue.
- **Agent**: The ACP-speaking process that `oxd` runs for a session.
- **Bridge**: An ACP agent that adapts another tool's interface. `claude-code-acp`
  bridges the Claude Code CLI to ACP.
- **Harness**: The coding-agent CLI a bridge drives, `claude` or `pi`.
- **Daemon**: `oxd`, the per-user process that owns sessions and agent processes.
- **Client**: The desktop app when discussing the daemon protocol;
  `oxd` when discussing ACP. Qualify as daemon client or ACP client when ambiguous.
- **Checkout**: A Git working tree, either the workspace's existing checkout or
  a worktree created for a session.
- **Worktree**: An additional Git working tree created by Ox for a session.
- **Comparison ref**: The Git ref against which a worktree's branch is
  compared for commits ahead and behind.
- **Parking**: Stopping an idle session's agent while retaining its conversation
  and checkout.
- **Archiving**: Retaining a session's conversation and branch while removing its
  worktree and preventing it from loading until unarchived.
- **Focus**: The session a client shows in front of the user, which
  the client's connection tells `oxd`. The desktop sets it only while its
  window is in front of other apps. Focusing an exited session that has
  started loads it. Distinct from viewing, the event subscriptions that keep
  a session from being parked.
- **Unseen**: How a session's turn ended, `finished` or `error`,
  while no client had it in focus. It turns the sidebar dot green or red
  until a client focuses it.
