# Ox MCP server with a set_title tool

## Goal

`oxd` gives every ACP agent it starts an MCP server named `ox`, which later
changes will extend with tools for messaging and transcript search. Its first
tool, `set_title`, sets the title of the agent's own Ox session. The new title
shows in the desktop's session list, is saved with the session, and survives
an `oxd` restart.

## Related code

- `crates/oxd/src/session.rs` — `Session::run` sends `NewSessionRequest` and
  `ResumeSessionRequest` with no MCP servers; `update` saves a changed title
  through `save` and signals the lists.
- `crates/oxd/src/state.rs` — `State::title` is set only by `append` from a
  `session_info_update`; `restored` and `unstarted` take the saved title, and
  `fill` leaves it alone.
- `crates/oxd/src/main.rs` — daemon startup; knows the socket path.
- `crates/ox-protocol/src/client.rs` — `daemon_binary` finds `oxd` beside
  the client's executable; `ox-mcp` is found beside `oxd` the same way.
- `crates/oxd/src/daemon.rs` — `Daemon::new`, `Session::spawn` and
  `Session::restore` calls, and `handle`, which dispatches protocol methods.
- `crates/oxd/src/conn.rs` — quick methods fall through to `daemon.handle`.
- `crates/ox-protocol/src/messages.rs`, `crates/ox-protocol/src/client.rs` —
  `Method` and `Client`, where `session.set_title` and `Client::set_title` go.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — scripted agent; advertises no MCP
  capability and ignores `mcpServers`.
- `crates/claude-code-acp/src/runner.rs` — `Claude::start` passes client MCP
  servers with `--mcp-config`; Claude asks permission for their tools.
- `crates/claude-code-acp/tests/protocol.rs` —
  `passes_stdio_mcp_servers_to_claude` captures Claude's `--mcp-config`.
- `scripts/smoke-acp.py` — live Claude checks; `check_load_after_restart`
  checks the saved title.

## Decisions

- **The server is `ox-mcp`,** a binary in its own crate. The agent
  launches it as `ox-mcp --socket <absolute socket> --session <session id>`.
  `oxd` finds it beside its own executable
  (`std::env::current_exe()?.with_file_name("ox-mcp")`, resolved once at
  startup). The session ID is `oxd`'s session ID, not the agent's.
- **`ox-mcp` is a daemon client.** Each tool call opens a `Client::connect`
  connection, calls the daemon method, and drops it. Future tools map onto
  daemon methods the same way. Holding no connection between calls means the
  process never counts as a client for idle exit.
- **`rmcp` implements MCP in `ox-mcp`.** `ox-mcp` depends on `ox-protocol`,
  the official Rust SDK `rmcp` 3.10 (`server`, `macros`, `transport-io`),
  `schemars` for the tool's parameter schema, and `tokio`. Read the crate
  docs for the `#[tool_router]`/`#[tool_handler]` pattern and
  `serve(stdio())`. `oxd` takes no new dependency: the fake agent writes the
  MCP client messages it needs (`initialize`, `notifications/initialized`,
  `tools/call`) as JSON lines.
- **Builds include `ox-mcp`.** `make dev`, `make e2e` and the smoke script
  build it beside `oxd`. `cargo test --workspace` builds it before the
  daemon tests run; `cargo test -p oxd` alone needs `cargo build -p ox-mcp`
  first.
- **`session.set_title { session_id, title }`** is a daemon method any client
  can call. It collapses runs of whitespace to one space, refuses an empty
  result with `invalid_request`, and answers `null`. `State::set_title`
  replaces the title; `Session::set_title` applies it through `update`, which
  saves it to the session row and publishes it in the lists. No event is
  recorded: the history holds what the agent reported, and the title is
  session metadata the lists carry.
- **`OxServer`** in `crates/oxd/src/mcp.rs` holds the `ox-mcp` and socket
  paths. `main` builds it, `Daemon` keeps an `Arc<OxServer>` and gives it to
  every `Session`, and `OxServer::for_session(id)` returns the
  `McpServer::Stdio` named `ox`. `Session::run` sends it on `session/new` and
  `session/resume` when the agent advertises `session.mcp.stdio`, and sends no
  servers otherwise.
- **The bridge trusts the client's MCP servers.** `Claude::start` adds
  `--allowedTools mcp__<name>` for each server the client supplies, before
  `--permission-mode` since the tool list is variadic, so Claude calls their
  tools without a permission request.
- **The tool:** `set_title`, parameter `title` (string), description "Set the
  title of this session in Ox, shown in the session list. Use a short phrase
  that names the task." Success returns the text `Title set.`; a daemon error
  returns a tool error with the daemon's message.

## Check your assumptions

1. **Whether Claude asks before calling `set_title`.** Kept.
   - **Decided:** The bridge allows every tool of every MCP server the ACP
     client supplies (`--allowedTools mcp__<name>`); today the bridge passes
     them with Claude's default permission handling, which asks.
   - **Consequences:** Agents retitle sessions without a permission prompt.
     Any MCP server a future client passes is trusted the same way.
   - **Alternative:** Leave permissions as they are, so each call raises a
     permission request the user answers or allows always.
   - **Trade-offs:** Trusting client servers keeps titles from interrupting
     work and suits a client that chose the servers; asking keeps the user in
     control of every call at the cost of a prompt per title change.

2. **When agents set a title.** Kept.
   - **Decided:** The tool is only exposed, with a description; the server
     sends no instructions telling agents to retitle, as the request asked
     for a tool only.
   - **Consequences:** Sessions keep the bridge's first-prompt title until
     the agent decides to call the tool or is asked to.
   - **Alternative:** MCP server `instructions` asking agents to set a short
     title once they understand the task.
   - **Trade-offs:** No instructions keeps the agent's context unchanged and
     titles predictable; instructions would give most sessions a better
     title at the cost of extra tool calls and less predictable titles.

3. **Where the MCP server runs.** Changed from an `oxd mcp` mode of the
   daemon binary to a separate `ox-mcp` binary.
   - **Decided:** A stdio process, `ox-mcp`, in its own crate, that the agent
     spawns and that calls the daemon over its socket; stdio is the only MCP
     transport the bridge accepts.
   - **Consequences:** Each Claude process starts one extra `ox-mcp`
     process. Tools are limited to what daemon methods expose. Builds and
     the daemon tests need `ox-mcp` beside `oxd`.
   - **Alternative:** Serve MCP inside `oxd` over HTTP or MCP-over-ACP.
   - **Trade-offs:** Stdio works with the bridge now and with any v2 agent
     that advertises it; in-daemon serving avoids the extra process and the
     protocol hop but needs bridge and Claude transport work first.

4. **How the title is set in `oxd`.** Kept.
   - **Decided:** A new `session.set_title` daemon method that changes the
     saved title without recording an event.
   - **Consequences:** The desktop could later offer rename through the same
     method. `oxctl attach` and the transcript show nothing when the title
     changes; the session list does.
   - **Alternative:** Record a `session_info_update` event, as if the agent
     had reported the title over ACP.
   - **Trade-offs:** No event keeps the history to what the agent sent; an
     event would show the change in the transcript stream but attribute it to
     the agent's ACP updates.

5. **Title validation.** Kept.
   - **Decided:** Collapse whitespace, refuse an empty title, no length cap.
   - **Consequences:** A long title is stored whole and the sidebar truncates
     it.
   - **Alternative:** Cap at 120 characters, as the bridge's first-prompt
     title does.
   - **Trade-offs:** No cap stores what the agent sent; a cap bounds what a
     misbehaving agent can store.

6. **New dependency.** Changed: the dependency is in `ox-mcp`, not `oxd`.
   - **Decided:** `rmcp` 3.10 and `schemars` in `ox-mcp`.
   - **Consequences:** More crates to build; MCP protocol details follow the
     official SDK. `oxd` and its fake agent take no new dependency.
   - **Alternative:** Hand-written JSON-RPC for `initialize`, `tools/list`
     and `tools/call`.
   - **Trade-offs:** The SDK covers version negotiation, schemas and errors
     as tools grow; hand-written code has no dependency but must track the
     MCP spec itself.

## Test plan

- `crates/oxd/tests/daemon.rs`, `agent_sets_title_through_ox_mcp_server`:
  start a session on the fake agent, wait for it to be idle, prompt
  `mcp-title Renamed by agent`, wait for the turn to end with the chunk
  `Title set.`, then check that `client.list()` gives the session the title
  `Renamed by agent`. Fails if `oxd` sends no server, `ox-mcp` cannot reach
  the daemon, or `session.set_title` does not change the title.
- `crates/claude-code-acp/tests/protocol.rs`: extend
  `passes_stdio_mcp_servers_to_claude` so the fake `claude` records its
  arguments, and check that `--allowedTools mcp__filesystem` precedes
  `--permission-mode`.
- `scripts/smoke-acp.py`, `check_title_tool`, run before
  `check_load_after_restart`: ask Haiku to call the `ox` server's `set_title`
  tool with `Smoke <token>` and nothing else, finish the turn, and check the
  session row's title. `check_load_after_restart` then confirms it survives
  the restart. A permission request would stall the turn, so this also
  covers the bridge's allowed tools.

## Implementation plan

1. `crates/ox-mcp/Cargo.toml` (new): binary crate `ox-mcp` with
   `ox-protocol`, `rmcp` 3.10 (`default-features = false`, features
   `server`, `macros`, `transport-io`), `schemars` at the version `rmcp`
   uses, `serde`, and `tokio`; `[lints] workspace = true`. The workspace
   picks it up through `members = ["crates/*"]`.
2. `crates/ox-protocol/src/messages.rs`: add `Method::SessionSetTitle
   { session_id, title }`, renamed `session.set_title`, with a doc comment.
   `crates/ox-protocol/src/client.rs`: add `Client::set_title`, through
   `ack`.
3. `crates/oxd/src/state.rs`: add `State::set_title`. Update the `title`
   field's doc comment to name both sources.
4. `crates/oxd/src/session.rs`: add `Session::set_title`, which normalizes
   the title and applies it through `update`. Keep an `Arc<OxServer>` on
   `Session`, taken by `spawn` and `restore`. In `run`, read
   `capabilities.mcp` and pass `ox_server.for_session(&id)` with
   `.mcp_servers(..)` on both requests when `stdio` is advertised.
5. `crates/oxd/src/daemon.rs`: hold `Arc<OxServer>`, taken by `Daemon::new`,
   and pass it to `Session::spawn` and `Session::restore`. Handle
   `Method::SessionSetTitle` in `handle`.
6. `crates/oxd/src/mcp.rs` (new): `OxServer { command, socket }` with `new`,
   which resolves `ox-mcp` beside the current executable, and
   `for_session`. `crates/oxd/src/main.rs`: build `OxServer` from the
   absolute socket path and pass it to `Daemon::new`.
7. `crates/ox-mcp/src/main.rs` (new): parse `--socket` and `--session`, and
   serve the `rmcp` server with the `set_title` tool on stdio until stdin
   closes. A module doc gives the usage.
8. `crates/oxd/src/bin/oxd-fake-agent.rs`: advertise `session.mcp.stdio`,
   keep each session's `mcpServers` from `session/new` and `session/resume`,
   and add the directive `mcp-title <text>`: start the server named `ox`,
   write `initialize`, `notifications/initialized` and a `tools/call` of
   `set_title` with the rest of the line as JSON lines on its stdin, read
   the answers from its stdout, and send the tool result's text, or the
   error, as one chunk. Close its stdin and wait for it to exit. Document
   the directive and the capability in the header.
9. `crates/claude-code-acp/src/runner.rs`: add `--allowedTools mcp__<name>`
   for each MCP server, before `--permission-mode`.
10. `Makefile`: add `-p ox-mcp` to the `dev` and `e2e` builds.
    `scripts/smoke-acp.py`: add `ox-mcp` to the packages it builds.
11. Tests from the test plan.

## Documentation updates

- `docs/dev/architecture.md`: add `ox-mcp` to the process diagram under
  `claude`, and a paragraph, **ox-mcp**, describing the server, how `oxd`
  finds and passes it, `set_title`, and `session.set_title`.
- `docs/dev/acp-v2.md`: `session/new` and `session/resume` rows say `oxd`
  sends the `ox` server to agents that advertise `session.mcp.stdio`; the
  `session.mcp.stdio` row marks `oxd` supported, and the bridge's note says
  Claude may use every tool of a server the client supplies without asking.
