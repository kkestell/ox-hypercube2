# Harness detection and agent choice

## Scope and coverage

The staged changes for `.agents/plans/2026-11-14-001-harness-detection.md`: the `agents` command, `Agent::Pi` and bridge selection in `crates/ox/src/lib.rs`, `useAgents` and the New Task menu in `ui/src/features/sessions/SessionList.tsx`, the e2e launch environment and `harness.e2e.ts`, and the architecture, glossary, and testing documents. They were checked against the plan, its five accepted assumptions, and section 1 of `.agents/scratch/pi.md`. The review traced detection through `utf8_env()` and `fix_path_env::fix()` (the crate runs `$SHELL -ilc '<command>'`, which the e2e stub shell executes with `/bin/sh -c "$2"`), the bridge's `claude` spawn in `crates/claude-code-acp/src/runner.rs`, and the agent fallback and popover rendering in the session list.

Gaps:

- Re-detection on a real window activation was not exercised. The e2e tests dispatch a synthetic `focus` event on the webview's `window`, and the app was not opened by hand.
- Choosing Pi was not exercised, since `pi-acp` arrives in task 2.

## Summary

No findings.

## Checks run

- `make check-rust`: passed, including `finds_executable_harnesses_on_path`.
- `make check-ui`: passed.
- `make e2e`: 11 of 12 spec files passed. `stress.e2e.ts` "streams a long reply into a long transcript without stalling" measured a 406 ms stall against its 100 ms limit. The spec passed alone (22 ms), and a second full run (`npm run e2e`) passed 12 of 12 with a 22 ms stall. The spec sends no `focus` event and does not touch agent choice.

## Verdict

The change delivers section 1 of the notes as planned: detection reads the `PATH` sessions get, the New Task button and Harness select follow the installed agents, focus re-detects, and the e2e tests cover none, one, and both harnesses through a controlled `PATH`. No changes were needed.
