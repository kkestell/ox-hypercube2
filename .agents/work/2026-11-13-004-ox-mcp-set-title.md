# Ox MCP server with a set_title tool

## Plan

`.agents/plans/2026-11-13-004-ox-mcp-set-title.md`

## Summary

`oxd` passes the `ox` stdio MCP server (`ox-mcp`) to agents that advertise `session.mcp.stdio`, and its `set_title` tool retitles the session through the new `session.set_title` daemon method. The title shows in the lists, is saved, and survives an `oxd` restart. The plan's goal is met; the live Haiku smoke run confirms it end to end.

## Departures from the plan

- `README.md`: `make dev` now builds `ox-mcp`, so the sentence describing what it builds names it. The plan's documentation list did not include the README.
- `docs/dev/architecture.md`: the diagram also has a dotted `ox-mcp → oxd` client arrow, and the legend now describes dotted arrows as client connections, since `ox-mcp` connects to `oxd` without starting it.
- `agent_sets_title_through_ox_mcp_server` prompts `mcp-title Renamed  by\tagent`, so the test also covers whitespace collapsing.

## Decisions

- `ox-mcp`'s tool returns `Result<String, String>`; rmcp turns `Err` into a tool result with `isError: true` and the daemon's message as text.
- The socket is passed as `display().to_string()` because ACP's stdio `args` are strings.
- The fake agent keeps MCP servers per session ID, replaced on each `session/new` or `session/resume`, and `mcp-title` reports `no ox MCP server` when the session has none.
- rmcp's default `serverInfo` (`rmcp`) is left as is.

## Checks run

- `make check-rust` — passed (fmt, clippy `-D warnings`, workspace tests).
- `make check-scripts` (ruff check and format) — passed.
- `make smoke-claude` — passed, including `agent_set_title_through_ox_mcp` and `saved_session_listed_after_daemon_restart` with title `Smoke ox-acp-tqhau9rd`.
- `make e2e` — not run; no change to `crates/ox` or the UI, only the Makefile build line.

## Manual verification

1. `ox-mcp` speaks MCP on stdio and reports a daemon error as a tool error.

   ```sh
   cargo build -p ox-mcp
   printf '%s\n' \
     '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
     '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
     '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
     '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"set_title","arguments":{"title":"x"}}}' \
     | (cat; sleep 1) | target/debug/ox-mcp --socket /tmp/nonexistent.sock --session s1
   ```

   Observed: `tools/list` lists `set_title` with the planned description and a required string `title`; the call answers `isError: true` with `No such file or directory (os error 2)`.

2. The daemon test fails without `ox-mcp` beside `oxd`.

   ```sh
   mv target/debug/ox-mcp /tmp/ox-mcp.bak
   cargo test -p oxd --test daemon -- agent_sets_title_through_ox_mcp_server
   mv /tmp/ox-mcp.bak target/debug/ox-mcp
   ```

   Observed: the chunk is `No such file or directory (os error 2)` instead of `Title set.`, and the test fails.
