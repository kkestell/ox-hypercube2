# Pi resume and replay

## Plan

[Pi resume and replay](../plans/2026-11-14-003-pi-resume-replay.md)

## Summary

Pi sessions retain their ACP/native identity, cwd, and title across bridge replacement and close. Explicit native replay recovers recorded messages and tool results before state and the resume response; normal Ox history remains unchanged.

## Departures from the plan

- The pinned ACP schema has no resume capability field. Registered `session/resume` while retaining the existing session capability, matching the available schema rather than changing dependencies or inventing metadata.

## Decisions

- Registry read/parse/write failures propagate directly; no registry repair subsystem was added.
- Save the first title before submitting its prompt so a registry failure can fail the request before Pi accepts work.
- Historical writes omit a diff because native arguments/results do not establish whether they created a file. Recorded arguments, locations, status, and output remain available.

## Checks run

- `cargo test -p pi-acp` — passed (one store test and 15 protocol tests).
- `make check-rust` — passed: formatting, workspace clippy with warnings denied, `ox-mcp` build, and workspace tests.
- `git diff --check` — passed.

## Manual verification

- `python3 /tmp/pi-resume-live.py` — passed against installed Pi 1.4.0. The local verification script creates an isolated `PI_ACP_STORE_DIR` and `PI_CODING_AGENT_DIR`, opens/closes an ACP session, writes a user/assistant transcript with the installed native `SessionManager.create`/`appendMessage` API using the registered native ID and canonical cwd, then starts a replacement bridge and resumes with `replayFrom: {"type":"start"}`. Observed user message, assistant message, idle state, then successful response. Closing waits for Pi to exit and the temporary directories are removed. No model call was made; next-prompt continuity is verified by the fake-Pi protocol test rather than a live model.
- Initial manual fixture used macOS's `/var` alias rather than canonical `/private/var`; Pi consequently discovered a different project session directory. Correcting the fixture cwd to match the bridge's existing canonicalization made the native check pass; production behavior needed no change.
