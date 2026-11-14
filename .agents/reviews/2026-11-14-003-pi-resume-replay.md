# Pi resume and replay review

## Scope and coverage

Reviewed the staged task-3 implementation against [the corrected plan](../plans/2026-11-14-003-pi-resume-replay.md) and [task 3](../scratch/pi.md#3-resume-and-replay). Read the complete changed Rust files and protocol test file, registry configuration and dependencies, and traced resume through the ACP handler, Pi command correlation and exit handling, native history conversion, and Ox's existing load decision. Compared the installed Pi 1.4.0 session selection and `get_state` response with the bridge's assumptions.

The implementation preserves Ox's saved displayed history: native replay is requested only when the existing daemon load path lacks a recorded agent start. The change adds no native branching, replay gate for working sessions, durable tool metadata, or corruption recovery machinery. The registry and history conversion have separate responsibilities and the changed files remain straightforward to follow.

Coverage limits: the work log's real-Pi check verifies native transcript reopening and replay without a model call; next-prompt continuation is established by protocol fixtures and installed Pi's native-ID selection rather than a live model conversation. No new verification concern or code change required repeating the completed checks.

## Summary

No findings.

## Checks and evidence

- Inspected the staged diff, complete affected files, callers, project guidance, and installed Pi's session-ID lookup and `get_state` implementation.
- Confirmed `oxd::State::load` requests native replay only without a saved `AgentStarted` event; ordinary reload retains recorded Ox events.
- Confirmed resume sends recorded updates before its state snapshot and response, and pending `get_state` requests resolve on Pi response or exit.
- Inspected protocol coverage for replacement bridge and close/reopen identity, replay order and recorded content, omitted replay, empty native files, parse errors, process exit, invalid cursors, unknown IDs, and mismatched cwd.
- Existing completed verification recorded in [the work log](../work/2026-11-14-003-pi-resume-replay.md): `cargo test -p pi-acp`, `make check-rust`, and `git diff --check` passed; the installed Pi 1.4.0 transcript reopening check passed. These were not rerun during this review because no implementation changed.

## Verdict

The staged implementation meets the corrected task-3 scope. No changes required.
