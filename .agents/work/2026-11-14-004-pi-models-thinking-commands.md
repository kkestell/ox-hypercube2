# Pi models, thinking levels, and commands

## Plan

[Pi models, thinking levels, and commands](../plans/2026-11-14-004-pi-models-thinking-commands.md)

## Summary

The plan's goal is met. Pi supplies model and thinking options and command
catalogs on startup, restart, and resume. Config changes use Pi RPC and return
Pi's resulting options; skill names remain `skill:name` through prompt submission.
The bridge adds no persisted preferences or UI changes.

## Checks run

- `cargo test -p pi-acp --bin pi-acp` — passed: five unit tests.
- `cargo test -p pi-acp` — passed: five unit tests and 18 protocol tests.
  The fake Pi answers the four description queries out of order. Coverage includes
  startup/restart publication, resume catalogs after replay and state with one
  `get_state`, model IDs containing `/`, thinking changes after a model switch,
  config errors, and native skill prompt submission.
- `make check-rust` — passed: workspace formatting, clippy with warnings denied,
  `ox-mcp` build, and all workspace tests. Its first run identified unnecessary
  raw-string hashes and a missing semicolon; these were fixed before the final run.
  Final output: `/tmp/ox-pi-models-check-rust.log`.
- `git diff --check` — passed.
- No UI, daemon, protocol, CLI, Claude bridge, or script changes: the documented
  UI end-to-end and Claude smoke requirements do not apply.

## Manual verification

An isolated ACP exchange against installed Pi 1.4.0 passed without submitting a
prompt. Startup returned 382 models, six thinking levels, and 17 commands. Setting
its current model (`openai-codex/gpt-6.1-sol`) and thinking level (`medium`) succeeded;
each response matched its `config_option_update`. Resume included both catalogs
after the state snapshot, and close stopped Pi. This proves the live RPC schema
and configuration path; fake tests establish model-switch and skill-prompt behavior.

To reproduce interactively:

```sh
cargo build -p pi-acp
PI_ACP_STORE_DIR="$(mktemp -d)" target/debug/pi-acp
```

Send these ACP lines on stdin:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":2,"capabilities":{},"info":{"name":"pi-config-check","version":"1"}}}
{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"/tmp","mcpServers":[]}}
```

After the idle snapshot and both catalog updates, replace `SESSION_ID` with the
new session ID, and `CURRENT_MODEL` and `CURRENT_LEVEL` with the corresponding
`currentValue` values from `configOptions`. Send:

```json
{"jsonrpc":"2.0","id":3,"method":"session/set_config_option","params":{"sessionId":"SESSION_ID","configId":"model","type":"id","value":"CURRENT_MODEL"}}
{"jsonrpc":"2.0","id":4,"method":"session/set_config_option","params":{"sessionId":"SESSION_ID","configId":"thinking","type":"id","value":"CURRENT_LEVEL"}}
{"jsonrpc":"2.0","id":5,"method":"session/resume","params":{"sessionId":"SESSION_ID","cwd":"/tmp","mcpServers":[]}}
{"jsonrpc":"2.0","id":6,"method":"session/close","params":{"sessionId":"SESSION_ID"}}
```

Wait for each response before sending the following request. Config responses
should match their preceding config update; resume should carry both catalogs
following a state snapshot. End stdin after close.
