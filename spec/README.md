# Ox

Ox is a desktop app for people who run coding agents on local projects. It groups conversations by workspace, supports parallel sessions in a shared checkout or isolated Git worktrees, and lets users follow work, answer requests, and merge completed tasks. Conversations remain available after the app closes and after the session service restarts.

## Areas

- [Workspaces and navigation](workspaces.md): project folders, session browsing, desktop layout, and connection feedback.
- [Sessions](sessions.md): creation, states, resuming, parking, archiving, deletion, and conversation retention.
- [Conversations](conversations.md): prompts, attachments, slash commands, streaming messages, and tool activity.
- [Permissions and questions](requests.md): tool decisions, question forms, validation, and cancellation.
- [Git tasks](git.md): isolated checkouts, branch selection, comparison, local merge, and cleanup safety.
- [Agents and settings](agents.md): compatible agents, supported prompt content, and session controls.
- [Claude Code bridge](claude.md): subscription authentication, standalone ACP use, session discovery, replay, and background work.

## Terms

- **Workspace:** A local directory added to Ox to group sessions for a project.
- **Checkout:** A Git working tree and the files in it. A workspace may also be a directory outside Git.
- **Worktree:** A separate Git checkout created and managed by Ox for a session.
- **Session:** A conversation with one selected agent, belonging to one workspace and running in one directory.
- **Task:** A session started with Session in Worktree, in its own worktree and compared with the workspace checkout's branch.
- **Agent:** The coding assistant selected for a session. Its capabilities determine the content and controls available.
- **Prompt:** One submission of text, attachments, or other supported content to an agent.
- **Turn:** A period of agent work, started by a prompt or by the agent itself, ending when the agent reports it has finished or stopped.
- **Tool call:** An action the agent reports, with a title, status, and any reported arguments and output.
- **Permission request:** An agent's request for a decision about an action, with a set of offered answers.
- **Elicitation:** A question form requested by an agent.
- **Comparison ref:** A Git reference or commit against which a worktree session's branch is compared.
- **Parked session:** A session whose idle agent Ox has stopped automatically, without removing its conversation or worktree.
- **Archived session:** A session set aside with its agent stopped and its Ox-managed worktree removed, retaining its conversation and branch.
- **Client:** The desktop app or another application connected to the session service.
- **Session service:** The per-user service, `oxd`, through which clients share workspaces and sessions.
- **ACP:** Agent Client Protocol, the interface through which Ox and other clients communicate with compatible agents.
- **Claude Code bridge:** The `claude-code-acp` agent, which makes an installed Claude Code available through ACP.
- **MCP server:** An external tool provider supplied to an agent by a compatible ACP client.
