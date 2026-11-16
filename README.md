# Ox

Ox is a macOS desktop app for running Claude Code and Pi agents in parallel
sessions and multi-step workflows. A per-user daemon runs the agents, so they
keep working when the app closes.

## Features

- Claude Code and Pi sessions with model, effort, and thinking controls, tool
  permissions, skills, and extension commands.
- Sessions grouped by workspace, each in the workspace checkout or its own Git
  worktree and branch.
- Workflows that run a task through a sequence of agent and shell steps in one
  worktree.
- Streaming responses, thinking, tool calls, and background-task updates.
- File attachments on prompts.
- Conversations and tool output saved by the daemon, so sessions survive app
  and daemon restarts.
- Archiving, which removes a session's worktree and keeps its conversation and
  branch. Archiving and deleting ask before discarding unsaved work.

## Installation

On macOS, with Rust, Node.js, and the Xcode Command Line Tools installed:

```sh
(cd ui && npm install)
make install
```

This builds a release `Ox.app` with the daemon, MCP server, and the Claude and
Pi bridges, signs it with an ad hoc signature, and copies it to
`~/Applications/Ox.app`, replacing any previous installation. Claude Code must
be installed and authenticated to use the Claude bridge. Pi must be installed
with a model provider authenticated to use the Pi bridge. The app reads `PATH`
from your interactive login shell so it can find these tools when opened from
Finder or the Dock.

## Development

See the [development guide](docs/dev/development.md).
