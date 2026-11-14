# Ox MCP server with a set_title tool

## Scope and coverage

Reviewed the staged implementation against
`.agents/plans/2026-11-13-004-ox-mcp-set-title.md`. The plan links no brief.
Coverage includes the `ox-mcp` stdio server and tool error conversion, daemon
protocol dispatch, title normalization, saving and list publication, MCP
capability negotiation on new and resumed sessions, client connection
lifetimes, Claude's allowed tools, the fake agent, build commands, integration
tests, live smoke checks, and associated documentation. Traced the desktop's
existing watch and session-title rendering paths.

Gaps:

- The desktop was not inspected interactively. Its code is unchanged;
  title rendering was checked by tracing the existing listing consumer.
- Default-concurrency Rust checks encountered two timeouts in existing tests.
  Each test passed alone, and the complete required Rust check passed with
  `RUST_TEST_THREADS=4`. The cause of the intermittent timeouts was not
  established.

## Summary

| ID | Severity | Category | Fix | Title |
| -- | -------- | -------- | --- | ----- |
| F1 | Medium | Testing | Fixed | Rust checks depend on a previously built MCP executable |

## F1. Rust checks depend on a previously built MCP executable

Medium · Testing · Fixed · `Makefile:26`

The daemon's new integration test launches `ox-mcp` beside `oxd`, but
`cargo test --workspace` builds only the MCP test harness under `debug/deps`,
not the standalone `debug/ox-mcp` executable. Neither formatting nor clippy
provides that executable. Consequently, the required `make check-rust`
command fails on a fresh checkout. CI runs `make check` before `make e2e`,
so the later e2e build cannot supply the missing prerequisite. A binary left
by development or smoke checks hides the failure and can also be stale.

### Evidence

Temporarily moved `target/debug/ox-mcp` aside and ran
`cargo test --workspace agent_sets_title_through_ox_mcp_server -- --exact`.
The test failed with the chunk `No such file or directory (os error 2)`
instead of `Title set.`. Cargo did not recreate the standalone executable.
The saved executable was restored after the check.

### Fix

Added `cargo build -p ox-mcp` before the workspace tests in `make check-rust`.
Documented the executable prerequisite for direct Cargo test runs in
`docs/dev/testing.md`. This was a Trivial fix: the approved plan already
requires builds and daemon tests to include the MCP executable.

Repeated `make check-rust` with the executable temporarily moved aside and
confirmed the build recreated it. That run later encountered the separate
protocol-test timeout described below. The final full Rust check passed
with four test threads.

## Checks run

- `make check-rust`, before the fix: formatting and clippy passed; the
  workspace tests timed out in
  `a_delete_during_startup_waits_for_the_worktree`.
- `cargo test -p oxd --test worktrees a_delete_during_startup_waits_for_the_worktree -- --exact`:
  passed.
- `cargo test --workspace agent_sets_title_through_ox_mcp_server -- --exact`,
  with the standalone MCP executable temporarily absent: reproduced F1.
- `make check-rust`, after the fix with the executable temporarily absent:
  recreated the executable; later timed out in
  `clients_without_forms_disallow_ask_user_question`.
- `cargo test -p claude-code-acp --test protocol clients_without_forms_disallow_ask_user_question -- --exact`:
  passed.
- `RUST_TEST_THREADS=4 make check-rust`: passed formatting, clippy with
  warnings as errors, and all workspace tests.
- `make check-scripts`: passed Ruff lint and formatting checks.
- `make smoke-claude`: passed with Haiku, including the MCP title call,
  saved title after daemon restart, resume, permissions, parking, archive,
  and process cleanup. Artifacts: `/private/tmp/ox-acp-jbmj_7fr`.
- `git diff --check`: passed.
- `make e2e` was not run: neither `crates/ox` nor UI behavior changed.

## Verdict

The implementation delivers the planned MCP title tool, preserves the saved
title across daemon restarts, and permits client-supplied MCP tools as
approved. F1 is fixed; no unresolved findings remain. Default-concurrency
test timeouts remain a verification limitation, with the complete suite
passing at four test threads.
