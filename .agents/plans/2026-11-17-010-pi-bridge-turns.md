# The Pi bridge holds prompts behind config changes and cancels

## Goal

Close the Structure item "The Pi bridge infers which prompt a dialog or model change belongs to" and its two Pi bridge bugs in `.agents/todo.md`: "Cancelling before an extension dialog arrives leaves the prompt pending" and "A prompt sent during a model change can run on the previous model".

Pi tags none of its run events, dialogs or user messages with a prompt, and it handles RPC lines concurrently. The bridge makes the attribution exact by deciding when commands go to Pi: prompts and config changes wait while a config change is unanswered, and prompts also wait while a cancel is still stopping Pi's work. During a cancel, every dialog belongs to the cancelled work, and the bridge answers it as cancelled without showing a form. One pure type, `Turns`, owns these rules and the existing prompt bookkeeping, and it is unit-tested over interleavings.

## Related code

- `crates/pi-acp/src/pi.rs` — `State` (lines 56–109) holds pending commands, the steering queue, `abort_on_start` and open dialogs. `Output` holds `running` and `starting` (285–289) and spreads the prompt rules across `response`, `prompt_answered`, `user_message_started`, `settled` and `dialog` (383–534). `Pi::abort` (243–259) withdraws only the dialogs already open.
- `crates/pi-acp/src/bridge.rs` — `set_config` sends the change straight to Pi (273, with the TODO). `cancel` (121) and `description` (394) are the other callers of `Pi`.
- `crates/oxd/src/session.rs:713` and `:1086–1118` — oxd sends each setting without waiting for its answer, whether the user changed it on a live session or a resumed agent is being restored. A prompt can follow on the next line, and a model pick sends `model` and then `effort`.
- `crates/oxd/src/state.rs:209–213` — a steered prompt that is still pending when its turn ends starts a turn of the agent's own. This covers a prompt the bridge holds until a cancelled run settles.
- Pi 1.5.2, `dist/modes/rpc/rpc-mode.js` — each stdin line runs as `void handleInputLine(line)`. `set_model` awaits `checkAuth` before it switches the model. `set_thinking_level`, `get_state` and `get_commands` run synchronously. `prompt` answers `started` from `_startRun`'s `onStarted` (`core/agent-session.js:1505`), which can come before the run's `agent_start`. A dialog promise ends only on the extension's own signal, a timeout, or an answer, so Pi's `abort` waits for any dialog that is open.

## Decisions

- **`Turns<P>` is a new pure module, `crates/pi-acp/src/turns.rs`.** It owns the command IDs, pending commands, the commands waiting to be sent, the steering queue, `running`, `starting`, `cancelling`, `changing` and `exited`. Its methods take the client's actions and Pi's records and return `Vec<Action<P>>`. `pi.rs` keeps the process, the dialog tasks and the translation of message and tool events. `P` is the prompt's payload because an ACP `Responder` cannot be built outside a connection. `pi.rs` uses `Box<Prompt>`, and the tests use plain values. Requests resolve their `oneshot` replies directly.
- **`Action<P>`** has these variants: `Send(Value)` writes a command to Pi. `Accept(P)` answers the prompt, sends its user message and reports working. `Fail(P, Error)` fails the prompt. `WithdrawDialogs` answers every open dialog as cancelled. `EndTurn` reports idle with `end_turn` when an extension command handled a prompt and no run is going. `pi.rs` carries out the actions in order while it holds the lock, so commands reach Pi in the order `Turns` chose them.
- **A config change blocks the commands sent after it.** Pi applies `set_model` only after an awaited auth check and applies `set_thinking_level` before it answers. Prompts and changes wait in one FIFO. The head of the FIFO goes to Pi when no change is unanswered and, if it is a prompt, no cancel is in progress. Queries (`get_state`, `get_commands`), stops and dialog answers go straight to Pi. When a model pick sends `model` and then `effort`, Pi now receives `set_thinking_level` after it has switched models. Before, Pi could apply the effort to the previous model and then replace it with the new model's default level.
- **A cancel lasts until the work it stopped has ended.** `cancelling` is set when a cancel finds a run going or a prompt that Pi has not answered. It clears when no run is going and every prompt sent before the cancel has its answer. While a cancel lasts, `Turns::takes_dialogs` is false, and `Output::dialog` answers new dialogs as cancelled without a form. Prompts sent during a cancel wait until it ends, along with any change sent after them. So a dialog during a cancel always belongs to cancelled work, and the extra stop for a cancelled prompt that Pi starts cannot stop a later prompt. The prompt that waited starts its own turn after the cancelled one ends, which oxd already handles. `cancelling` replaces `abort_on_start`, with the same extra stop when Pi answers a cancelled prompt `started` or `queued`.
- **A `started` answer marks the run as going.** Pi sends that answer when the run starts, and the answer can arrive before the run's `agent_start`.
- **A cancel fails prompts that are still waiting** with `request_cancelled`, as it already fails steering prompts. Changes that are still waiting are sent once nothing blocks them.
- **Pi exiting** fails pending, waiting and steering prompts with Pi's exit error and answers pending and waiting requests with it. `Turns::prompt` returns the payload when Pi has exited, so `Bridge::prompt` keeps its current error path.

## Check your assumptions

None.

## Test plan

- `turns.rs` unit tests. No test can reach this module today, and integration tests cannot show a command that was never sent while the bridge waits on Pi.
  - `changes_hold_later_prompts_and_changes_until_pi_answers`: covers a `set_model` followed by `set_thinking_level` and then a prompt, in the order oxd sends a model pick and its effort. The second change is sent only after Pi answers the first, and the prompt only after Pi answers the second. Also check that a prompt queued before a later change retains its FIFO position.
  - `dialogs_are_refused_from_a_cancel_until_the_cancelled_work_ends`: a table of interleavings with the expected `takes_dialogs` after each step. The cases are a cancel during a run, up to `settled(true)`; a cancel before Pi answers a prompt, which Pi then starts (an extra stop is expected), up to its settle; a cancel before Pi answers a prompt that an extension command then handles; and a cancel while idle.
  - `a_prompt_sent_during_a_cancel_waits_for_the_cancelled_run_and_is_not_stopped`: the prompt is sent only after `settled(true)`. Its `started` answer yields `Accept` and no further stop.
- `crates/pi-acp/tests/protocol.rs`:
  - Extend `cancel_withdraws_open_dialogs` and rename it `cancel_withdraws_open_dialogs_and_refuses_later_ones`. After `aborted`, the fake emits a second dialog and records the answer before it runs `settled true`. Assert that exactly one `elicitation/create` reaches the client, that both recorded answers are `cancelled`, and that the turn ends `cancelled`. On the code before the change, a second form opens.
  - New `a_cancel_fails_a_prompt_waiting_for_a_model_change`: guards the bridge sending a prompt to Pi before Pi answers `set_model`. No existing test sends a prompt behind an unanswered change. The fake reads `set_model`, runs `aborted` and then answers `set_model` and `describe`. The test sends the model change, a prompt and a cancel. Assert that the prompt fails with `-32800`, that the change succeeds, and that the fake wrote no `prompts` file. On the code before the change, the fake reads the prompt where it expects `clear_queue` and exits.
- The other protocol tests pass unchanged, including `cancel_sent_right_after_prompt_cancels_it`, the steering tests, `an_aborted_run_and_pi_exit_withdraw_dialogs` and the config tests.

## Implementation plan

1. Add `crates/pi-acp/src/turns.rs` with `Turns<P>` and `Action<P>`. Its public methods are `prompt(command, P) -> Result<Vec<Action<P>>, P>`, `change(command, reply) -> Vec<Action<P>>`, `query(command, reply) -> Vec<Action<P>>`, `cancel()`, `response(&Value)`, `run_started()`, `user_message_started()`, `settled(aborted)`, `takes_dialogs()`, `exited(error)` and `has_exited()`. Its private helpers are `send`, `stop` and `release`. `release` sends waiting commands in order until a change is unanswered, or until the head is a prompt and a cancel is in progress. `response`, `settled` and `cancel` end with `release`. Move the logic of `State::send`, `State::stop`, `prompt_answered` and `user_message_started` into it, and the refused-stop and unknown-response logging. Register the module in `main.rs`, and add the unit tests.
2. In `pi.rs`, replace `State` with a struct of `turns: Turns<Box<Prompt>>` and `dialogs: HashMap<String, AbortHandle>` behind the existing `Arc<Mutex<…>>`. Add one function that carries out a `Vec<Action<Box<Prompt>>>` with the input sender, `Updates` and the dialogs. `Pi` exposes `prompt`, `change`, `query`, `cancel` (renamed from `abort`), `has_exited` and `snapshot`. `Output` drops `running` and `starting` and calls `Turns` on `response`, `agent_start`, a user `message_start` and `agent_settled`. `settled` carries out `Turns::settled`'s actions and then fails open tools and reports idle as now. `dialog` answers as cancelled when `takes_dialogs` is false. `read` uses `Turns::exited` when Pi exits.
3. In `bridge.rs`, `cancel` calls `pi.cancel()`, `set_config` calls `pi.change(command)` with the TODO removed, and `description` calls `pi.query`.
4. Update the protocol tests as the Test plan describes.
5. Check the Structure item and the two Pi bridge bugs in `.agents/todo.md`.

## Documentation updates

- `docs/dev/acp-v2.md`:
  - In the `session/prompt` caveat, add that Pi's bridge holds a prompt sent while a config change is unanswered or a cancel is stopping Pi until the change is answered or the cancel has finished, and that a prompt held behind a cancel starts a turn of its own.
  - In the `elicitation/create` caveat, add that Pi's bridge answers a dialog that arrives while a cancel is stopping Pi as cancelled, without a form.
