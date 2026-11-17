# Development

## Repository layout

| Path                      | Description                                   |
| ------------------------- | --------------------------------------------- |
| `crates/ox/`              | Ox desktop app (Tauri 2)                      |
| `crates/oxd/`             | Session daemon between the app and ACP agents |
| `crates/ox-protocol/`     | oxd wire types and client                     |
| `crates/ox-mcp/`          | MCP server oxd gives its agents               |
| `crates/acp-bridge/`      | Shared parts of the ACP bridges               |
| `crates/claude-code-acp/` | ACP bridge for the Claude Code CLI            |
| `crates/pi-acp/`          | ACP bridge for the Pi CLI                     |
| `crates/oxctl/`           | Daemon client agents use to test oxd          |
| `ui/`                     | Ox frontend (TypeScript, Vite)                |

## Running the app

With the UI dependencies installed, run the app from the repository root:

```sh
make dev
```

`make dev` builds `oxd`, its `ox-mcp` server, and the Claude and Pi bridges. It
runs the app against its own `oxd` on `target/dev/oxd.sock`, which it stops
before building and after the app exits.

## UI mock mode

```sh
cd ui && npm run mock
```

`npm run mock` serves the app in a browser at `localhost:1430/mock/` with
Vite's hot reload, in place of the desktop core and `oxd`. The app shows a
scenario's fixed state: `?scenario=<name>` picks one of the scenarios in
`ui/mock/scenarios.ts`, and `sidebar` is the default. Commands that change
state do nothing. Context menus, native dialogs, and file contents come from
the desktop core and are absent.

## UI tests

```sh
cd ui && npx vitest
```

`npx vitest` runs the UI tests in jsdom and runs the affected tests again on
each save; `npm test` runs them once. `npx vitest run <path>` runs one file.

## UI changes

- Develop and show UI changes in mock mode, on a scenario that has the states
  the change affects. Add the scenario when none does.
- A change that needs new backend data gets that data in a scenario before the
  backend changes.
- Show context menus, native dialogs, and file views in `make dev`.
