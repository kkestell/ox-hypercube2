# Pi extension prompts as form elicitations

## Plan

[Pi extension prompts as form elicitations](../plans/2026-11-14-005-pi-extension-prompts.md)

## Summary

The plan's goal is met within Pi's observable RPC lifecycle. Select, confirm,
input, and editor dialogs become session-scoped forms; answers return to Pi.
Working sessions require action while a form is open. Timeout, cancellation,
close, aborted runs, and process exit withdraw outstanding requests. Clients
without form support receive no dialogs.

Plain-string form controls use shared textareas and preserve line breaks and
validation. Choice and formatted controls retain their behavior. Pi's inability
to report an extension's independent private AbortSignal remains documented in
[the harness observations](../../docs/research/harnesses/pi.md#9-tools-permissions-and-approvals).

## Departures from the plan

- Visual inspection found the existing `.composer textarea` rule overriding
  shared form-control styling. Narrowed that prompt-specific rule to
  `.composer .input textarea`, so form textareas use their shared `text-field`
  border, background, radius, padding, and alignment.
- The existing fake-agent Note field now has a multiline default, letting the
  existing form E2E tests verify default preservation, multiline submission,
  and textarea pattern validation. This fixture change also required the
  documented Claude smoke check.

## Checks run

- `cargo test -p pi-acp` / focused protocol test — eight unit tests and the
  expanded protocol coverage exercised during implementation. An initial scope
  assertion incorrectly expected a nested ACP scope; corrected it to the
  actual flattened `sessionId` before the passing checks.
- `make check-rust` — passed: formatting, clippy with warnings denied,
  `ox-mcp` build, and workspace tests, including eight Pi unit tests and
  24 Pi protocol tests. Initial local clippy violations were corrected.
  Final output: `/tmp/ox-pi-forms-check-rust-final.log`.
- `make check-ui` — passed: Biome and application/E2E TypeScript checks.
- `make e2e` — passed: 12 suites, including multiline submission and pattern
  validation. Repeated after correcting the styling conflict; final output:
  `/tmp/ox-pi-forms-e2e-final.log`.
- `make smoke-claude` — passed with `claude-haiku-5-5`, including a real
  AskUserQuestion form, cancellation, reconnect, replay, and cleanup.
  Output: `/tmp/ox-pi-forms-smoke.log`; artifacts:
  `/private/tmp/ox-acp-00m_xq6a`.
- `git diff --check` — passed.

## Manual verification

1. Compared the multiline Note textarea with the adjacent Email and Count
   controls in the desktop screenshot. Its edges, background, border, radius,
   and horizontal padding now match; both edited lines remain visible.

   ```sh
   cd ui
   npm run e2e -- --spec e2e/specs/session.e2e.ts
   ```

   The focused run passed all 17 cases. The final full E2E run captured the
   corrected rendering at
   `/var/folders/x3/s0dy9w311sncwf87160crpw80000gn/T/ox-e2e-wJpXgb/elicitation-text-fields.png`.

2. Ran the bridge against installed Pi 1.4.0 with a temporary extension and
   isolated `PI_CODING_AGENT_DIR`, `PI_ACP_STORE_DIR`, and project. A temporary
   PATH wrapper supplied `--offline --no-extensions --no-context-files
   --no-mcp --no-skills --no-prompt-templates -e <temporary-extension>` before
   the bridge's normal arguments. No user configuration changed and no model
   was invoked.

   ```sh
   python3 /tmp/ox-pi-forms-live.py
   ```

   An extension command opened all four dialogs before Pi accepted its prompt.
   The extension received `Block`, `false`, `""`, and `"edited\nsecond"`.
   A second command's 100 ms timeout withdrew its form. Cancelling a third
   command withdrew its form and released its pending prompt with an undefined
   extension answer. Closing the session completed successfully.
   Captured ACP events and extension answers:
   `/var/folders/x3/s0dy9w311sncwf87160crpw80000gn/T/ox-pi-forms-live-ob9wub33/`.

## Verification limits

Pi's independent private AbortSignal emits no withdrawal record in the inspected
RPC implementation. Such a form stays visible until answered or the session is
cancelled; Pi ignores an answer to a forgotten request ID. No polling or custom
Pi integration was added. Scripted protocol tests establish aborted-run and
process-exit withdrawal; the live extension probe establishes ordinary dialogs,
timeout, and cancellation before prompt acceptance.
