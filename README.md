# Ox

A Cargo workspace containing the Ox desktop app, its session daemon, and the Claude
Code ACP bridge.

| Path                      | Description                                                                       |
| ------------------------- | --------------------------------------------------------------------------------- |
| `crates/ox/`              | Ox desktop app (Tauri 2)                                                          |
| `crates/claude-code-acp/` | ACP bridge for the Claude Code CLI                                                |
| `crates/oxd/`             | Session daemon that owns ACP agent processes and serves Ox UIs over a Unix socket |
| `crates/ox-protocol/`     | oxd wire types and client                                                         |
| `crates/oxctl/`           | Daemon client for the desktop end-to-end and live Claude smoke tests               |
| `ui/`                     | Ox frontend (TypeScript, Vite)                                                    |

See [architecture](docs/dev/architecture.md), [where sessions run](docs/worktrees.md),
and [ACP v2 support](docs/dev/acp-v2.md). Historical harness research covers
[Claude Code](docs/research/harnesses/claude-code.md) and
[Pi](docs/research/harnesses/pi.md).

## Features

- **Claude Code bridge included.** A custom ACP wrapper brings Claude Code into
  Ox, including model and effort controls, tool permissions, and background tasks.
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

## Development

See [agent guidance](AGENTS.md) and the [testing guide](docs/dev/testing.md)
for development conventions and required verification.

Run the app from the repository root:

```sh
(cd ui && npm install)
make dev
```

`make dev` builds `oxd` and the Claude bridge and runs the app against its own
`oxd` on `target/dev/oxd.sock`, which it stops before building and after the
app exits.
