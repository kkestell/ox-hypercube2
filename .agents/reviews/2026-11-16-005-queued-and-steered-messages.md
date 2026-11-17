# Queued and steered messages

## Scope and coverage

The staged changes for `.agents/plans/2026-11-16-005-queued-and-steered-messages.md`: the daemon's queue, steering, drain and pausing (`state.rs`, `session.rs`, `daemon.rs`, `store.rs`), the protocol and client, both bridges (`claude-code-acp` runner, `pi-acp`), the conversation fold, the Tauri commands, `oxctl`, the fake agent, the composer and transcript UI, and the docs and specs. Checked against the brief `.agents/briefs/2026-11-16-005-queued-and-steered-messages.md` and the plan's changed Check your assumptions entries (Stop discards untaken steers, Edit replaces in place, one Send/Stop button with Escape to stop).

The change delivers the brief's outcome and keeps its decisions: Enter queues and Cmd+Enter steers, the daemon keeps and persists the queue, prompts go one per turn, Stop and abnormal ends hold the queue, and steered prompts show dimmed until taken in and cannot be edited or deleted.

Gaps:

- The Pi bridge's message ordering was checked against the installed Pi's `agent-loop.js` and `agent-session.js` (a started prompt always emits its user `message_start`; extension-injected messages have role `custom`), not against every Pi version.
- The Claude preemption path was reviewed by reading only; it depends on Claude CLI behavior the smoke run covers.
- `scripts/smoke-acp.py` and the e2e spec were read but not rerun.

## Summary

| ID | Severity | Category    | Fix   | Title                                              |
| -- | -------- | ----------- | ----- | -------------------------------------------------- |
| F1 | Medium   | Correctness | Fixed | Escape that closes a dialog also stops the turn    |

## F1. Escape that closes a dialog also stops the turn

Medium · Correctness · Fixed · `ui/src/features/composer/Composer.tsx:94`

While the viewed session is running, the composer's window `keydown` listener stops the turn on Escape unless the event was default-prevented or a popover is open. The app's modal dialogs (`components/Dialog.tsx`: Discard unsaved work, Branch kept, the workflow run dialog, the merge dialog) close on Escape through the dialog's `cancel` event, which the browser fires only after `keydown` has been dispatched and left uncancelled. So pressing Escape to dismiss one of those dialogs also cancels the running turn and holds the queue.

### Evidence

Per the HTML close-request steps, a modal dialog's `cancel` fires after an uncancelled `keydown`, so the listener sees `defaultPrevented === false`, and `:popover-open` does not match a `<dialog>`. A new test that opens a modal dialog and presses Escape failed with `cancel` invoked before the fix and passes after it.

### Fix

The listener also skips Escape while `dialog[open]` matches. Added the Composer test "leaves the turn running when Escape closes a dialog", and `spec/requests.md` now says Escape that dismisses command suggestions, a menu, or a dialog does not stop the turn.

## Checks run

- `npx vitest run src/features/composer/Composer.test.tsx -t dialog` with the fix reverted: failed, as expected.
- `make check-ui`: passed (67 tests).

## Verdict

The feature is implemented as briefed and planned, with sound state handling in the daemon and both bridges. The one finding, F1, is fixed.
