# Ox

A Cargo workspace containing the Ox desktop app, its session daemon, and the ACP
bridges for Claude Code and Pi.

| Path                      | Description                                                                       |
| ------------------------- | --------------------------------------------------------------------------------- |
| `crates/ox/`              | Ox desktop app (Tauri 2)                                                          |
| `crates/claude-code-acp/` | ACP bridge for the Claude Code CLI                                                |
| `crates/pi-acp/`          | ACP bridge for the Pi CLI                                                         |
| `crates/oxd/`             | Session daemon that owns ACP agent processes and serves Ox UIs over a Unix socket |
| `crates/ox-protocol/`     | oxd wire types and client                                                         |
| `crates/oxctl/`           | Daemon client for the end-to-end and live smoke tests, and for workflow runs      |
| `ui/`                     | Ox frontend (TypeScript, Vite)                                                    |

See [architecture](docs/dev/architecture.md), [where sessions run](docs/worktrees.md),
[workflows](docs/workflows.md), and [ACP v2 support](docs/dev/acp-v2.md). Historical harness research covers
[Claude Code](docs/research/harnesses/claude-code.md) and
[Pi](docs/research/harnesses/pi.md).

## Features

- **Claude Code bridge included.** A custom ACP wrapper brings Claude Code into
  Ox, including model and effort controls, tool permissions, and background tasks.
- **Pi bridge included.** Use model and thinking controls, skills and extension
  commands, and extension dialogs as forms.
- **Multiple projects, parallel sessions.** Organize sessions by workspace and
  keep independent tasks running.
- **Isolated Git worktrees.** New Task gives each task its own checkout
  and branch.
- **Sessions that survive the app.** Close and reopen Ox without interrupting
  agents; resume saved conversations after daemon restarts.
- **Live conversation and tool activity.** Follow streaming responses, thinking,
  tool calls, results, and background-task updates.
- **File attachments.** Attach local files to a prompt as links.
- **Permission controls.** Approve or reject tool requests and interrupt running
  work.
- **Persistent history.** Retain conversations, tool output, and images
  independently of the agent process.
- **Archive and restore.** Release a session's worktree while keeping its
  conversation and branch.
- **Safe cleanup.** Detect unsaved work before removing worktrees and preserve
  branches containing unique commits.

## Installation

On macOS, with Rust, Node.js, and the Xcode Command Line Tools installed:

```sh
(cd ui && npm install)
make install
```

This builds a release `Ox.app` with the daemon, MCP server, and the Claude and
Pi bridges, signs it with an ad hoc signature (no certificate required), and
copies it to `~/Applications/Ox.app`, replacing any previous installation.
Claude Code must be installed and authenticated to use the Claude bridge. Pi
must be installed with a model provider authenticated to use the Pi bridge. The
macOS app reads `PATH` from your interactive login shell so it can find tools
when opened from Finder or the Dock.

## Development

See [agent guidance](AGENTS.md) and the [testing guide](docs/dev/testing.md)
for development conventions and required verification.

Run the app from the repository root:

```sh
(cd ui && npm install)
make dev
```

`make dev` builds `oxd`, its `ox-mcp` server, and the Claude and Pi bridges. It
runs the app against its own `oxd` on `target/dev/oxd.sock`, which it stops
before building and after the app exits.

### Daemon configuration and data

`oxd` listens on `$OX_SOCKET` when set. Otherwise it uses `oxd.sock` in
`$XDG_DATA_HOME/ox` when `XDG_DATA_HOME` is an absolute path, or
`~/.local/share/ox`. The database is `<socket>.db`; daemon stderr is appended
to `oxd.log` beside the socket. Clients start `oxd` from beside their own
executable when no daemon answers.

The daemon's `--park-after-ms` option sets the idle agent parking delay;
the default is 15 minutes.
