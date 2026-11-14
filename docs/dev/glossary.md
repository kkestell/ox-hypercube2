# Glossary

Use these terms consistently. See [Architecture](architecture.md) for their
ownership and lifecycle.

- **Workspace**: A directory the user adds to Ox, under which sessions are grouped.
- **Session**: A saved conversation and its execution context, belonging to a
  workspace. A session can outlive its agent process and the daemon.
- **Turn**: One period of agent work within a session, started by a prompt or by
  the agent itself.
- **Agent**: The ACP-speaking process that `oxd` runs for a session.
- **Bridge**: An ACP agent that adapts another tool's interface. `claude-code-acp`
  bridges the Claude Code CLI to ACP.
- **Daemon**: `oxd`, the per-user process that owns sessions and agent processes.
- **Client**: The desktop app or `oxctl` when discussing the daemon protocol;
  `oxd` when discussing ACP. Qualify as daemon client or ACP client when ambiguous.
- **Checkout**: A Git working tree, either the workspace's existing checkout or
  a worktree created for a session.
- **Worktree**: In session checkout choices, an additional Git working tree
  created by Ox for that session.
- **Comparison ref**: The Git ref against which a worktree session's branch is
  compared for commits ahead and behind.
- **Parking**: Stopping an idle session's agent while retaining its conversation
  and checkout.
- **Archiving**: Retaining a session's conversation and branch while removing its
  worktree and preventing it from loading until unarchived.
