# Ox

Ox is a macOS desktop app for running Claude Code and Pi agents in parallel
sessions. A per-user daemon runs the agents, so they keep working when the app
closes.

## Features

- Claude Code and Pi sessions with model, effort, and mode controls, tool
  permissions, skills, and extension commands.
- Sessions grouped by workspace, each in the workspace checkout or its own Git
  worktree and branch.
- Streaming responses, thinking, tool calls, and background-task updates. A
  completed turn's work folds under Worked, leaving its prompt and reply.
- File attachments on prompts.
- Prompts queued while an agent works, sent one per turn by the daemon, and
  prompts that steer the running turn.
- Agent tools to message other sessions, read any session's transcript, and
  ask the user to start a new session.
- Conversations and tool output saved by the daemon, so sessions survive app
  and daemon restarts.
- Archiving, which removes a session's worktree and keeps its conversation and
  branch. Archiving and deleting ask before discarding unsaved work.
- Claude Code sessions run outside Ox imported into their workspace, archived,
  with their history.

## Installation

On macOS, with Rust, Node.js, and the Xcode Command Line Tools installed:

```sh
(cd ui && npm install)
make install
```

This builds a release `Ox.app` with the daemon, MCP server, and the Claude and
Pi bridges, signs it with an ad hoc signature, and copies it to
`~/Applications/Ox.app`, replacing any previous installation. It stops the
running `oxd` if no session is busy and relaunches Ox; saved workspaces and
sessions carry over. `make install-ui` reinstalls only the app and leaves the
running `oxd` and its sessions alone. Claude Code must
be installed and authenticated to use the Claude bridge. Pi must be installed
with a model provider authenticated to use the Pi bridge. The app reads `PATH`
from your interactive login shell so it can find these tools when opened from
Finder or the Dock.

`make install` also copies `config/config.json` to `~/.config/ox/config.json`
when no config is there yet. Its `harnesses` object lists the harnesses,
models, effort levels, and modes Ox offers; the first of each listed is a new
session's default.

## Development

See the [development guide](docs/dev/development.md).
