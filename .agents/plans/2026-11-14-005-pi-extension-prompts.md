# Pi extension prompts as form elicitations

## Goal

Deliver [task 5, Extension prompts as form elicitations](../scratch/pi.md#5-extension-prompts-as-form-elicitations) under the scratch decision "Extension prompts become form elicitations now": Pi's `extension_ui_request` `select`, `confirm`, `input`, and `editor` dialogs reach the client as form elicitations, the client's answer returns to Pi as `extension_ui_response`, and a dialog Pi stops waiting for has its elicitation withdrawn. A turn never waits on a dialog nobody can answer.

## Related code

- `crates/pi-acp/src/pi.rs` — `Output::translate` answers every dialog `cancelled: true` (the `TODO(task 5)`). `State` is shared by `Pi` and `Output`; `Pi::abort` (from `session/cancel` and `Pi::stop`), `Output::settled`, and `Output::read`'s exit path are where a dialog can stop being answerable. `Output::send` writes to Pi through a weak sender.
- `crates/pi-acp/src/updates.rs` — `Updates` holds the connection and the session's `working` flag; `state_update` only knows running and idle.
- `crates/pi-acp/src/bridge.rs` — `Bridge::initialize` ignores client capabilities. `start_pi` and `resume_session` call `Updates::new`.
- `crates/claude-code-acp/src/bridge.rs` (`forms`, `initialize`), `updates.rs` (`ask_user`, `Asking`, `current`, `publish`), `runner.rs` (`control_request`, `permissions: HashMap<String, AbortHandle>`), and `questions.rs` — the pattern to follow: capability captured at `initialize`, one spawned task per request with its `AbortHandle` in a map, abort drops the in-flight request so the SDK sends `$/cancel_request`, and `requires_action` while a request is open.
- `crates/oxd/src/state.rs` — a pending elicitation makes the session `AwaitingInput` and keeps it from parking whatever the agent's state. `Change::Cancelled` leaves elicitations open until the agent withdraws them; `$/cancel_request` ends one as `cancel`.
- `ui/src/features/composer/Elicitation.tsx` — renders the message as the panel title, each field with its `title` as legend and `description` below, `oneOf` strings as radios, other strings as a single-line `input` prefilled from `default`; Decline and Submit buttons. Empty strings are omitted from accepted content.
- Pi RPC, read in installed Pi 1.4.0 (`/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent`, `dist/modes/rpc/rpc-mode.js`, `docs/rpc-extension-ui.md`) and checked against `rpc-types.ts` on Pi's `main`:
  - Requests: `select {title, options: string[], timeout?}`, `confirm {title, message, timeout?}`, `input {title, placeholder?, timeout?}`, `editor {title, prefill?}`. Answers: `{value}`, `{confirmed}`, or `{cancelled: true}`; cancelled resolves `undefined` (`false` for confirm).
  - `createDialogPromise` resolves the default and forgets the ID when `timeout` elapses or the extension's `AbortSignal` fires, with no output to the client. A response to a forgotten ID is ignored.
  - Extension commands run inside `prompt` before its `preflightResult`, so a dialog an extension command opens holds the prompt's response until it is answered; Pi's `abort` does not end it.

## Decisions

- **`src/dialogs.rs`, pure functions.**
  - `form(request: &Value) -> Option<(String, ElicitationSchema)>`: message `Pi has a question`; one property, `answer`, titled with Pi's `title`:
    - `select`: required string with `one_of` = Pi's options as `EnumOption::new(option, option)`, duplicates dropped. `None` when there are no string options.
    - `confirm`: required string with `one_of` `yes` "Yes" and `no` "No"; description is Pi's `message` when non-empty.
    - `input`: optional string; description is Pi's `placeholder` when non-empty.
    - `editor`: optional string; `default` is Pi's `prefill` when present.
    - Any other method: `None`.
  - `response(request: &Value, action: Option<ElicitationAction>) -> Value`: the `extension_ui_response` with Pi's `id`. Accept gives `select` `{value}` when `answer` is one of the options, `confirm` `{confirmed: answer == "yes"}` for `yes` or `no`, `input` and `editor` `{value}` with the `answer` string or `""` when absent. Decline, cancel, no action (request failed or withdrawn), and an accepted answer of the wrong shape give `{cancelled: true}`.
- **Capability.** `Bridge` gains `forms: AtomicBool`, set in `initialize` (now `&self`, called on the shared bridge in `main.rs`) from `ElicitationCapabilities::supports_form`, as in `claude-code-acp`. `Updates::new` takes `forms`; `start_pi` and the replay `Updates` pass the bridge's value.
- **`Updates::ask(&self, message, schema) -> impl Future<Output = Option<ElicitationAction>> + Send + 'static`.** Sends `CreateElicitationRequest::new(ElicitationFormMode::new(ElicitationSessionScope::new(session_id), schema), message)` and awaits it with `block_task`, holding an owned `Asking` guard for the request's life. `Updates::forms()` exposes the flag.
- **`requires_action`.** `updates::State` gains `asking: usize` and `sent: Sent` (`Idle`, `Running`, `RequiresAction`). The current state is idle when not working, `requires_action` when working with a dialog open, otherwise running. `working`, `idle`, and `Asking`'s creation and drop publish the current state when it differs from `sent`; `idle` carries its stop reason. `snapshot` reports the current state. A dialog opened while Pi is not working (an extension command before its prompt is answered, or a `session_start` handler) sends no state; `oxd` still shows the session awaiting input from the pending elicitation.
- **One task per dialog (`pi.rs`).** `State` gains `dialogs: HashMap<String, AbortHandle>` keyed by Pi's request ID. For a dialog method, `Output::translate` calls `Output::dialog(record)`:
  - When `updates.forms()` is false or `dialogs::form` is `None`, it answers `cancelled: true` at once.
  - Otherwise, holding the `State` lock, it spawns a task and inserts its `AbortHandle`. The task awaits `updates.ask(..)`, raced against `tokio::time::sleep(timeout)` when the request carries a numeric `timeout` (the timeout branch gives `None`). It then takes the `State` lock and, only if it removes its own entry, sends `dialogs::response` to Pi. Holding the lock across spawn and insert keeps a fast task from missing its entry.
  - Each dialog gets exactly one answer: from its task, or from `State::withdraw_dialogs`.
- **Withdrawal.** `State::withdraw_dialogs(&mut self, input: Option<&UnboundedSender<Value>>)` drains `dialogs`, aborts each task (dropping its request, so the SDK sends `$/cancel_request`), and sends `{"type":"extension_ui_response","id":…,"cancelled":true}` for each. It runs:
  - in `Pi::abort`, before sending `abort`, so `session/cancel` and `session/close` end open dialogs, including an extension command's dialog that Pi's `abort` would not end;
  - in `Output::settled` when `aborted` is true, since an aborted run fires the extensions' abort signals;
  - in `Output::read` once Pi has exited (Pi is gone, so the answers go nowhere).
- **Editor text must survive the form.** `editor` remains an ordinary ACP string property with its prefill as the default. Render plain-text string fields without a format or choices in `ui/src/features/composer/Elicitation.tsx` as textareas using the existing `text-field` styling; preserve formatted string inputs and choice controls. This lets users edit and submit line breaks even when the editor starts empty, without a Pi-specific schema extension or daemon protocol change. Apply the same existing validation to textareas as to text inputs. Keep the control aligned with peer text fields and add only layout CSS needed by the textarea.
- **Notification methods** (`notify`, `setStatus`, `setWidget`, `setTitle`, `set_editor_text`) stay ignored.
- **Dependencies.** `pi-acp`'s `tokio` gains the `time` feature.

## Check your assumptions

1. **How the bridge learns that Pi withdrew a dialog.**
   - **Decided:** Pi sends no withdrawal record (1.4.0 and `main` resolve timed-out or aborted dialogs silently), so the bridge withdraws an elicitation when the request's own `timeout` elapses, on `session/cancel` or close, when a run settles aborted, and when Pi exits.
   - **Consequences:** The bridge runs a timer only for dialogs whose extension set `timeout`, starting when it reads the request, so the form closes just after Pi has given up. A dialog an extension abandons through its own `AbortSignal` for any other reason stays on screen until the user answers or cancels, and that answer is ignored by Pi.
   - **Alternative:** Get Pi to emit a withdrawal record upstream, and drop the timer.
   - **Trade-offs:** Inference works with today's Pi and covers the timeout and cancel cases extensions use. An upstream record would be exact but depends on a Pi release.
2. **Cancelling a turn also cancels dialogs outside the run.**
   - **Decided:** `session/cancel` answers every open dialog `cancelled: true`, because oxd leaves elicitations open on cancel and Pi's `abort` does not end a dialog an extension command opened.
   - **Consequences:** Pressing stop while an extension command asks something makes that command continue with `undefined` (or `false` for confirm), and its prompt then completes.
   - **Alternative:** Withdraw only dialogs opened during the running turn.
   - **Trade-offs:** Withdrawing all keeps "stop" from leaving a form behind that blocks the prompt. Withdrawing only run dialogs needs per-dialog run tracking and leaves extension-command forms open after stop.
3. **The form's layout.**
   - **Decided:** Every dialog is one field, `answer`, titled with Pi's title under the panel title `Pi has a question`, following the AskUserQuestion form. `confirm` is a required Yes/No choice with Pi's message as the field's description; `input`'s placeholder becomes the description.
   - **Consequences:** The panel title is generic for all four dialog kinds, and a confirm needs a choice before Submit.
   - **Alternative:** Use Pi's title as the panel title and give the field no title, with `confirm` as a field-less form where Submit means yes and Decline means no.
   - **Trade-offs:** The chosen form matches the existing Claude question panel and makes yes and no explicit. The alternative avoids the generic title, but the app labels an untitled field with its name, and a Submit button reads poorly as "yes".
4. **Declining a dialog.**
   - **Decided:** Decline, like cancel, answers `cancelled: true`.
   - **Consequences:** The extension receives `undefined`, or `false` for `confirm`, the same as dismissing it in Pi's terminal UI.
   - **Alternative:** Decline a `confirm` as `confirmed: false` and others as `cancelled`.
   - **Trade-offs:** The extension sees the same value either way for `confirm`; one rule is simpler.
5. **`editor` dialogs preserve multiline text.**
   - **Choice:** Use the existing plain-string form schema and let the shared form renderer accept multiline plain text.
   - **Consequences:** Plain-text form fields, including existing Claude free-text answers, become textareas; their content and submit/decline semantics remain the same. Formatted inputs and choices keep their controls.
   - **Reason:** Mapping an editor to a single-line input would silently discard line breaks from normal editor content. A shared plain-text control avoids introducing a Pi-only protocol hint.
6. **Forms are not linked to a tool call.**
   - **Decided:** The elicitation is scoped to the session only, because Pi's request carries no tool call ID.
   - **Consequences:** A form an extension's `tool_call` handler raises (such as a future permission gate) is not attached to the tool call it concerns in the transcript.
   - **Alternative:** Attach the elicitation to the single tool call that has started and not ended, when there is exactly one.
   - **Trade-offs:** Session scope is always correct. Guessing the tool call links the common case but can mislabel a form raised by a non-tool handler while a tool runs.

## Test plan

- `dialogs.rs` unit tests:
  - Representative translation cases for the four methods, covering choices, confirmation, optional empty text, and a multiline editor default. Check the schema details here rather than repeating them in protocol tests; include an unrenderable select.
  - `response`: `select` accepted with an option, and with a value outside the options (cancelled); `confirm` `yes`, `no`, and other (cancelled); `input` with text and with `answer` absent (`""`); `editor` with text; decline, cancel, and `None` give `cancelled: true`; each keeps Pi's `id`.
- `tests/protocol.rs`. The `initialize` helper takes the client capabilities; `session` advertises `{"elicitation":{"form":{}}}`, and a variant opens a session with `{}`. A helper waits for a client-bound request by method and answers it.
  - `extension_dialogs_become_forms`: during a run the fake Pi emits `notify` (no elicitation), then `select`, `confirm`, `input`, and `editor`, each followed by `next_command` recording the answer. The client receives session-scoped `elicitation/create` forms, and Pi receives the matching response IDs and values. Assert the `requires_action`/`running` transition once during this flow; schema translation is covered by unit tests. The client accepts `{"answer":"Block"}`, `{"answer":"no"}`, `{}`, and `{"answer":"edited"}`; Pi records `{"value":"Block"}`, `{"confirmed":false}`, `{"value":""}`, and `{"value":"edited"}` with the request IDs. The turn ends `end_turn`.
  - `declined_and_cancelled_dialogs_answer_cancelled`: decline then cancel two `input` dialogs; Pi records `cancelled: true` for both.
  - `cancel_withdraws_open_dialogs`: with a `confirm` open, the client sends `session/cancel`; it receives `$/cancel_request` naming the elicitation's request ID, and the fake Pi reads `extension_ui_response` `cancelled: true` before `abort`. The turn ends `cancelled`.
  - `a_dialog_with_a_timeout_is_withdrawn`: a `select` with `"timeout":50` that the client never answers is withdrawn with `$/cancel_request`, and the fake Pi reads `cancelled: true`.
  - `an_aborted_run_and_pi_exit_withdraw_dialogs`: an open dialog followed by `settled true` from the fake Pi, and in a second case by the fake Pi exiting, each yields `$/cancel_request`.
  - `extension_dialogs_are_dismissed` becomes `clients_without_forms_get_no_dialogs`: with capabilities `{}`, no `elicitation/create` is sent and Pi reads `cancelled: true`.

- UI: extend the existing elicitation E2E coverage to edit and submit multiline plain text, including preservation of a multiline default. Inspect the affected controls beside their peers on screen.
- Required checks: `make check-rust`, `make check-ui`, and `make e2e` for the bridge and form-renderer changes, following [Testing](../../docs/dev/testing.md#running-checks). Do not repeat broad checks unless a subsequent change or failure invalidates them.

## Implementation plan

1. Add `src/dialogs.rs` with `form` and `response` and their unit tests; declare it in `main.rs`.
2. `updates.rs`: `forms` in `Updates::new` and `Updates::forms`; `asking`, `Sent`, `current`, and `publish` replacing the direct sends in `working` and `idle`; three-state `state_update` and `snapshot`; the owned `Asking` guard and `Updates::ask`.
3. `bridge.rs` and `main.rs`: `Bridge.forms`, `initialize(&self, ..)` capturing it, and passing it to every `Updates::new`.
4. `Cargo.toml`: add tokio's `time` feature.
5. `pi.rs`: `State.dialogs` and `State::withdraw_dialogs`; `Output::dialog` replacing the `TODO(task 5)` branch; withdrawal in `Pi::abort`, aborted `Output::settled`, and `Output::read` after exit.
6. `tests/protocol.rs`: capability parameter, request helper, and the tests above.
7. `ui/src/features/composer/Elicitation.tsx` and existing elicitation E2E coverage: use a shared textarea for plain text, preserve validation and newlines, and verify the rendered controls.
