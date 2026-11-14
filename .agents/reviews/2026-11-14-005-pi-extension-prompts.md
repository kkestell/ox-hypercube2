# Pi extension prompts as form elicitations

## Scope and coverage

Reviewed the staged implementation against [the plan](../plans/2026-11-14-005-pi-extension-prompts.md) and [scratch task 5](../scratch/pi.md#5-extension-prompts-as-form-elicitations), including the parent's correction that editor text must preserve line breaks through shared plain-text form controls. No separate brief applies.

Read the touched bridge, dialog translation, process and request lifetimes, session updates, form renderer, shared styling, fake-agent fixture, protocol tests, and session E2E coverage. Traced client capabilities, accepted/declined/cancelled responses, concurrent request ownership, cancellation, close, timeout, aborted runs, process exit, and session snapshots. Compared the installed Pi 1.4.0 RPC dialog implementation and inspected the recorded live answers and final desktop screenshot.

Coverage limits:

- Pi's private extension AbortSignal emits no RPC withdrawal record. Its abandoned form remains until answered or the session is cancelled, as the plan and harness research explicitly document. No invented signal or polling was added.
- The implementation's isolated real Pi extension probe covered ordinary dialogs, timeout, cancellation before prompt acceptance, and close without invoking a model. Aborted-run and process-exit withdrawal are established by scripted protocol tests, rather than a real model run.
- UI and live Claude checks were completed during implementation. This review inspected their recorded evidence and changed only Pi timeout handling and its protocol regression; those checks were not repeated.

## Summary

| ID | Severity | Category | Fix | Title |
| -- | -------- | -------- | --- | ----- |
| F1 | Medium | Correctness | Fixed | A zero timeout dismisses a dialog Pi leaves open |

## F1. A zero timeout dismisses a dialog Pi leaves open

Medium · Correctness · Fixed · `crates/pi-acp/src/pi.rs:396`

An extension can supply `timeout: 0` to leave a dialog open. Pi's `createDialogPromise` starts its timer only when `opts.timeout` is truthy, so zero means no timeout. The bridge instead accepted zero as a duration and withdrew the user's form immediately, returning `cancelled: true` to Pi.

### Evidence

Installed Pi 1.4.0's `dist/modes/rpc/rpc-mode.js:64` uses `if (opts?.timeout)` before scheduling the timer. The new `a_zero_timeout_keeps_the_dialog_open` protocol regression failed against the reviewed implementation with `a zero timeout withdrew the dialog`. It passes with the fix, verifies the form remains open beyond an immediate timer, and then verifies Pi receives the user's accepted `Ada` answer.

### Fix

Start the bridge timer only for positive timeout values. Added the focused protocol regression without changing other dialog or cancellation behavior.

## Checks run

- `cargo test -p pi-acp --test protocol a_zero_timeout_keeps_the_dialog_open -- --exact` — failed before the fix and passed afterward.
- `make check-rust` — passed for the final source, including formatting, clippy, workspace tests, eight Pi unit tests, and 25 Pi protocol tests. The first review invocation caught formatting in the new test; `cargo fmt --all` corrected it before the final passing run. Output: `/tmp/ox-pi-forms-review-check-rust.log`.
- `git diff --check` — passed.
- Inspected implementation verification evidence: `make check-rust` output at `/tmp/ox-pi-forms-check-rust-final.log`, full `make e2e` output at `/tmp/ox-pi-forms-e2e-final.log` (12 suites passed), and `make smoke-claude` output at `/tmp/ox-pi-forms-smoke.log` (passed). The work log records passing `make check-ui`.
- Inspected `elicitation-text-fields.png` in `/var/folders/x3/s0dy9w311sncwf87160crpw80000gn/T/ox-e2e-wJpXgb`: the multiline Note field matches neighboring Email and Count controls in edges, background, border, radius, and horizontal padding.
- Inspected recorded real Pi extension answers in `/var/folders/x3/s0dy9w311sncwf87160crpw80000gn/T/ox-pi-forms-live-ob9wub33/project`: `Block`, `false`, empty input, and `edited\nsecond`; timeout and session cancellation recorded cancellation.

## Verdict

F1 is fixed and covered by a regression that failed before the fix. No High or Low findings, unresolved decisions, or additional blocking defects were found. The implementation delivers task 5 within Pi's documented observable RPC lifecycle.
