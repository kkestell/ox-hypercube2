# Pi models, thinking levels, and commands

## Goal

Deliver [task 4, Models, thinking levels, and commands](../scratch/pi.md#4-models-thinking-levels-and-commands): a Pi session offers Pi's models and thinking levels as ACP session config options that `session/set_config_option` changes, and Pi's `get_commands` catalog reaches the client as `available_commands`, with skills listed and invoked as `/skill:name`.

## Related code

- `crates/pi-acp/src/pi.rs` — RPC command correlation. `Pending::SessionFile` is the only command that returns data; `Output::response` and `Output::read` settle it on response and on exit.
- `crates/pi-acp/src/bridge.rs` — `start_pi`, `start_new`, `prompt`, and `resume_session`. Resume already sends `get_state` for the session file before replay, state, and response.
- `crates/pi-acp/src/main.rs` — request handlers; no `SetSessionConfigOptionRequest` handler yet.
- `crates/claude-code-acp/src/bridge.rs` (`set_config`, `options`, `resume_session`) and `main.rs` — the config-option and resume-response pattern to follow: answer with the full option list and also send `config_option_update`.
- `crates/claude-code-acp/src/commands.rs` — `catalog`, the native-descriptor-to-`AvailableCommand` translation to mirror.
- `crates/oxd/src/session.rs` and `state.rs` — `oxd` takes `config_options` and `available_commands` from the new/resume response into `Change::Started`, and applies `config_option_update` and `available_commands_update` only once the session is live. Updates sent while a resume is pending are not applied, so the resume response must carry both lists.
- `ui/src/features/composer/Composer.tsx` and `ModelPicker.tsx` — pick the model by category `model` and the level by category `thought_level`; the picker searches at six or more models. `CommandInput.tsx` inserts `/{name}` into the prompt text, which reaches Pi unchanged through `content::pi_prompt`.
- Installed Pi 1.4.0 (`/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent`; no 1.5.2 install is present):
  - `dist/modes/rpc/rpc-mode.js` — `get_state` returns `model` (full object or absent) and `thinkingLevel`; `get_available_models` returns `{models}` of models with configured auth; `set_model {provider, modelId}` fails with `Model not found: …`; `set_thinking_level {level}` always succeeds; `get_available_thinking_levels` returns `{levels}`; `get_commands` returns `{commands:[{name, description?, source, sourceInfo}]}` with skills as `skill:<name>`. Input lines are handled concurrently (`void handleInputLine`), so responses can arrive out of order.
  - `dist/core/agent-session.js` — `setModel` throws without auth, records a `model_change` entry, and re-applies the thinking level for the new model (per-model default, then clamped). `setThinkingLevel` clamps to the model's levels and records a `thinking_level_change` entry.
  - `dist/core/sdk.js` — on start, a session with a conversation restores its model and thinking level from those entries. `dist/core/session-manager.js` writes the session file only once a user or assistant message exists.

## Decisions

- **One generic correlated request.** Replace `Pending::SessionFile` with `Pending::Request(oneshot::Sender<Result<Value, String>>)` and `Pi::session_file` with `Pi::request(&self, command: Value) -> oneshot::Receiver<Result<Value, String>>`, resolving to the response's `data` (`Value::Null` when absent), Pi's `error` on `success: false`, or the exit reason. Every query in this task and resume's `get_state` use it.
- **`Bridge::describe`.** Sends `get_state`, `get_available_models`, `get_available_thinking_levels`, and `get_commands` to the session's Pi in one step (before returning), and returns a `'static` future resolving to a `Description { state: Value, config_options: Vec<SessionConfigOption>, available_commands: Vec<AvailableCommand> }`. Resume uses `state["sessionFile"]` from it for replay, so resume sends one `get_state`.
- **When the client learns them.**
  - `start_pi` returns whether it started a new Pi. When `start_new` or `prompt` started one, the bridge spawns a task that awaits `describe` and sends `config_option_update` then `available_commands_update`; a Pi that exits first is logged.
  - `resume_session` awaits `describe` (whether or not Pi was already running), replays when asked, sends state, then answers with `config_options` and `available_commands`.
  - `set_config` answers with the options read after the change and sends `config_option_update`, as `claude-code-acp` does.
- **Options (`src/options.rs`).** `options::build(state, models, levels) -> Vec<SessionConfigOption>`:
  - With no `state.model`, no options.
  - `model` ("Model", category `Model`): one choice per available model in Pi's order, value `provider/id`, name Pi's `name`, followed by ` (provider)` only when another available model has the same name. The current value is `state.model`'s `provider/id`.
  - `thinking` ("Thinking", category `ThoughtLevel`): only when `levels` has more than one entry. Values are Pi's levels; names are `Off`, `Minimal`, `Low`, `Medium`, `High`, `XHigh`, `Max`. The current value is `state.thinkingLevel`.
- **`set_config`.** Requires an open session and an `Id` value; starts Pi if needed. `model` splits the value at its first `/` into `provider` and `modelId` (provider names carry no `/`; model IDs may) and sends `set_model`; Pi rejects unknown or unauthenticated models. `thinking` accepts only Pi's seven level names and sends `set_thinking_level`; Pi clamps it to the model. Other IDs or values are invalid params. A Pi error becomes an internal error with Pi's message. After Pi answers, the bridge calls `describe` and uses its options, which reflect any level Pi changed with the model.
- **The bridge saves no model or thinking level.** Pi's session records them and restores them when Pi restarts on a session with a conversation (see assumption 1).
- **Commands (`src/commands.rs`).** `commands::catalog(&Value) -> Vec<AvailableCommand>` keeps every `get_commands` entry (extension commands, prompt templates, skills) in Pi's order, drops empty, whitespace-containing, or duplicate names, uses an empty description when Pi gives none, and sets no input hint, since Pi's RPC gives none. Skill entries keep Pi's `skill:<name>` name, so the composer inserts `/skill:<name>` and Pi expands it. The bridge does no prompt translation.
- **Ordering of a change and a following prompt.** Pi handles RPC lines concurrently and `set_model` awaits an auth check, so a prompt sent right after a model change may start on the previous model. Add a `TODO` at `set_config` naming this case.

## Check your assumptions

1. **Where a session's model and thinking level persist.**
   - **Decided:** Pi's own session file keeps them. `setModel` and `setThinkingLevel` record entries that `sdk.js` restores on restart, so the registry stays unchanged.
   - **Consequences:** A choice made before the session's first prompt is lost if Pi restarts before that prompt (parking after 15 idle minutes, an Ox restart, or a crash), and the session then uses Pi's default model. Once a conversation exists, choices survive restarts.
   - **Alternative:** Save `model` and `thinking` in `acp-sessions.json` and reapply them with `set_model` and `set_thinking_level` after each Pi start.
   - **Trade-offs:** Saving covers the pre-prompt case. It adds a reapply step that a prompt sent right after a restart can overtake, needs handling for a saved model that is no longer available, and appends a `model_change` entry to Pi's session on every start.
2. **The default model and the list offered.**
   - **Decided:** A new session starts on whatever Pi selects from the user's Pi settings, and the list is Pi's available models (those with configured auth).
   - **Consequences:** Ox imposes no default, such as the smoke model `openai/gpt-6-luna`. A Pi install with no authenticated provider shows no model picker, and its prompts fail with Pi's error.
   - **Alternative:** A bridge default model applied at session creation.
   - **Trade-offs:** Pi's default matches the user's Pi setup with no Ox setting. A bridge default gives every Pi session the same start but overrides the user's Pi configuration.
3. **The composer's model picker for Pi sessions.**
   - **Decided:** No UI change. The picker shows Pi's models and levels, and it still draws the Claude logo beside the button and each model.
   - **Consequences:** Pi sessions show a Claude logo next to non-Claude models.
   - **Alternative:** Tell the UI each session's agent and draw no logo, or Pi's logo, for Pi sessions.
   - **Trade-offs:** Leaving the UI keeps this task in `pi-acp`. Fixing the logo needs the agent in the daemon protocol and the UI, and Pi's logo asset.
4. **When options and commands refresh.**
   - **Decided:** At each Pi start, at resume, and after an Ox config change.
   - **Consequences:** A model or level changed inside Pi by an extension command, or a skill added on disk, shows in Ox only after Pi next starts. Pi's own catalog also ignores new skills until restart.
   - **Alternative:** Also refresh on Pi's `thinking_level_changed` events and on each turn's end.
   - **Trade-offs:** Refreshing on events tracks changes made outside Ox at the cost of more queries and updates during turns; the decided rule covers every change made through Ox.

## Test plan

- Give `FAKE_PI` a `describe` helper that reads the four startup queries in any order and answers each by type: `get_state` with a model, `thinkingLevel`, and the `sessionFile` a test sets; two models; levels; and commands including a prompt template, an extension command without a description, and `skill:review`. Call it at each fake Pi start in the existing scripts, replacing the resume tests' bare `get_state` loops.
- `options::build` and `commands::catalog` unit tests: values `provider/id`, provider suffix only on a shared name, thinking omitted with a single level, no options without a model, level names; catalog order, empty description, `skill:` names kept, invalid and duplicate names dropped.
- Protocol: a new session sends `config_option_update` and `available_commands_update` after its idle state, with the `model` and `thinking` options and `/skill:review` as `skill:review`.
- Protocol: `session/set_config_option` for `model` sends `set_model` with the provider and a model ID containing `/` intact, then answers with the options Pi reports afterward (including a thinking level Pi changed) and sends a matching `config_option_update`. `thinking` sends `set_thinking_level`. Pi's `Model not found` error, an unknown option ID, an unknown level, and a non-`Id` value fail.
- Protocol: resume answers with `config_options` and `available_commands`, after replay and state, using one `get_state`. A Pi that exits before answering `describe` fails resume as `get_state` does now.
- Protocol: a prompt after Pi exited restarts Pi, and the new Pi's options and commands are sent again.

## Implementation plan

1. `pi.rs`: replace `Pending::SessionFile` and `Pi::session_file` with `Pending::Request` and `Pi::request`, settled in `Output::response` and on exit. Update resume to read `sessionFile` from the `get_state` data.
2. Add `src/options.rs` (`build`, level names, value parsing for `set_config`) and `src/commands.rs` (`catalog`), with unit tests; declare both in `main.rs`.
3. `bridge.rs`: add `Description` and `describe`; make `start_pi` return whether it started Pi and publish the description from `start_new` and `prompt` when it did; have `resume_session` use `describe` for the session file and return `config_options` and `available_commands` in its response.
4. `bridge.rs` and `main.rs`: add `Bridge::set_config` (send the Pi command synchronously, then a future that awaits Pi's answer, calls `describe`, sends `config_option_update`, and answers with the options), with the ordering `TODO`, and register the `SetSessionConfigOptionRequest` handler as `claude-code-acp` does.
5. `tests/protocol.rs`: add the `describe` helper, update existing scripts, and add the protocol tests above.
