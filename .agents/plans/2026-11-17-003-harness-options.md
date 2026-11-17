# Harnesses, models, and effort levels from a config file

## Goal

Ox reads the harnesses, models, effort levels, and modes it offers from a
config file instead of asking the harnesses, and agents request sessions by
naming a listed harness, model, and effort level.

**Brief:** `.agents/briefs/2026-11-17-002-harness-options.md`

## Related code

- `crates/ox-protocol/src/client.rs` — `data_dir`/`data_dir_from` (the XDG rule to mirror), `describe` and `bridge_mode` (`bridge_mode` stays for import), `Client::request_session`.
- `crates/ox-protocol/src/messages.rs` — `Method::SessionRequest`, `Harness`, `Controls`, `EventKind::PermissionRequested`.
- `crates/oxd/src/daemon.rs` — `request_session` builds the text description and calls `settings`, which runs the bridge's `--describe`; the `SessionRequest` params struct.
- `crates/oxd/src/session.rs` — `Session::ask` (only caller is `request_session`); `run` applies `Opening.options` only where the agent reports that config ID with a different value, and logs a refused value.
- `crates/oxd/src/state.rs` — `Change::PermissionRequested` becomes `EventKind::PermissionRequested`.
- `crates/ox-mcp/src/main.rs` — `start_session` and the tool list in the module doc.
- `crates/ox/src/lib.rs` — `agents` (installed harnesses), `agent_controls`.
- `crates/ox/src/conversation.rs` — `Item::Permission`.
- `crates/claude-code-acp/src/bridge.rs` — `describe`, `MODELS`, `EFFORTS`, `mode_options`, `is_*` checks in `send_config`; `crates/claude-code-acp/src/runner.rs` — `commands` and `Conversation::Unsaved`, used only by `describe`; `crates/claude-code-acp/src/main.rs` — the `--describe` arm.
- `crates/pi-acp/src/bridge.rs` — `describe`, `description` (asks Pi for state, models, thinking levels, commands); `crates/pi-acp/src/options.rs` — `build`, `command`, `LEVELS`, provider labels; `crates/pi-acp/src/main.rs` — the `--describe` arm.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — `--describe` mode; `CONFIG` (`model` fast/smart, `effort` low/medium/high, `mode` ask/code).
- `ui/src/features/composer/controls.ts` — `useControls` (probes and caches `agent_controls` per agent and workspace), `useLastUsed`, `withValues`.
- `ui/src/App.tsx` — `draftControls`, `startingControls`, the probe on viewing a draft.
- `ui/src/features/composer/DraftComposer.tsx`, `Composer.tsx` (`SessionComposer`, how `model`/`effort`/`others` are split), `ModelPicker.tsx` (reads `_meta["ox/provider"]`).
- `ui/src/features/sessions/useAgents.ts` — `useAgents`, refreshed on window focus.
- `ui/src/features/composer/Permission.tsx`, `ui/src/lib/protocol.ts` — the uncommitted `SessionRequestView` and `SessionRequest` type this plan feeds.
- `ui/src/test/core.tsx`, `ui/mock/main.ts` — `agents` and `agent_controls` fakes.
- `ui/e2e/wdio.conf.ts` — fixture `claude-code-acp` answers `--describe`; `XDG_CONFIG_HOME` is the run's `config` directory.
- `crates/oxd/tests/agents.rs` — session request tests; `crates/oxd/tests/common/mod.rs` — the daemon's `XDG_CONFIG_HOME` is its temp dir.
- `scripts/smoke-acp.py` — `check_describe`, Pi's `check_thinking`.

## Decisions

- **Ox's config file.** `config.json` in Ox's config directory: `$XDG_CONFIG_HOME/ox` when `XDG_CONFIG_HOME` is absolute, else `~/.config/ox`, else `<temp dir>/ox` (the `config_dir` the workflows used, restored beside `data_dir`). It is Ox's one config file; the harness list is its `harnesses` key, and later settings become other keys. `harnesses` has one object per harness, keyed by its executable name; each lists `models` (array of `{ id, name, provider?, efforts }`) and `modes`. Effort levels and modes are `{ id, name }` objects. Array order is the picker order.

  ```json
  {
    "harnesses": {
      "claude": {
        "modes": [{ "id": "manual", "name": "Manual" }, { "id": "auto", "name": "Auto" }],
        "models": [
          {
            "id": "claude-opus-5-5",
            "name": "Opus 5.5",
            "efforts": [{ "id": "medium", "name": "Medium" }, { "id": "high", "name": "High" }]
          }
        ]
      },
      "pi": {
        "modes": [],
        "models": [
          {
            "id": "openrouter/deepseek/deepseek-v4.1-flash-1017",
            "name": "DeepSeek V4.1 Flash",
            "provider": "OpenRouter",
            "efforts": [{ "id": "low", "name": "Low" }]
          }
        ]
      }
    }
  }
  ```

- **One loader, three readers.** `ox_protocol::config` holds `Config { harnesses: Vec<HarnessConfig> }`, `HarnessConfig { harness: Harness, models: Vec<ModelConfig>, modes: Vec<Choice> }`, `ModelConfig { id, name, provider: Option<String>, efforts: Vec<Choice> }`, `Choice { id, name }`, and `load() -> Result<Config, String>`, with harnesses in `Harness::ALL` order. A missing or unparsable file, or an unknown harness name, is an error naming the file. The desktop core, the daemon, and `ox-mcp` each call `load()` when they need it, so an edit applies at the next window focus, request, or listing. It parses with `serde_json`, already a dependency.
- **The `agents` command returns the config.** `agents` returns `Result<Vec<HarnessConfig>, String>`: the listed harnesses whose executables are on the session `PATH`. `agent_controls`, `ox_protocol::describe`, and `Controls` go.
- **Uniform option IDs.** Every bridge names its options `model`, `effort`, and `mode`. Pi's `thinking` becomes `effort`. The UI builds a draft's options with these IDs; for a running session it uses the ID of the option the session reports in the same category, so a Pi session still running an older bridge keeps working.
- **Bridges stop listing.** Each bridge reports each option with its current value as the only choice, named by its value, and passes any value through to its harness. Claude: `MODELS`, `EFFORTS`, the mode list, the `is_*` checks, `describe`, `runner::commands`, and `Conversation::Unsaved` go; `send_config` accepts any value of `model`, `effort`, `mode`. Pi: `description` asks only `get_state` and `get_commands`; `options::build(state)` reports `model` (`provider/id`) and `effort` (`thinkingLevel`, when the state has one); `options::command` maps `effort` to `set_thinking_level` with any non-empty level; `LEVELS` and the provider labels go. Both `--describe` modes go, and the fake agent's.
- **Building pickers in the UI.** `controls.ts` keeps `useLastUsed` and replaces `useControls`, `withValues`, and `offers` with:
  - `draftOptions(harness, ...sources)`: a `model` select of the config's models, an `effort` select of the chosen model's efforts (absent when it has none), and a `mode` select of the config's modes (absent when none), each at the first source value it offers, else its first choice. Effort is chosen after model, so switching a draft's model to one without its effort level takes the new model's first level. A model's `provider` goes in its choice's `_meta["ox/provider"]`, so `ModelPicker` is unchanged.
  - `sessionOptions(harness, reported)`: the same selects, each only where the session reports an option of its category, at the reported value and with the reported option's ID. The effort choices are those of the config model matching the reported model. A reported value the config does not list shows as its ID (`ModelPicker` already falls back to the value).
  Names are fixed: Model, Effort, Mode.
- **Drafts carry no slash commands.** `DraftComposer` passes `commands={[]}`; a starting session shows none until its agent reports them.
- **Session requests.** `session.request` takes `harness: String`, `model: String`, `effort: Option<String>`, `mode: Option<String>`; harness and model are required. `Daemon::request_session` loads the config and refuses, before asking, a harness the config does not list (`codex is not a harness Ox offers; it offers claude, pi`), a model the harness does not list (`huge is not a claude model Ox offers; it offers fast, smart`), a missing effort for a model with effort levels, an effort the model does not list, an effort for a model without levels, a missing mode for a harness with modes, a mode the harness does not list, or a mode for a harness without modes, each naming what is listed. The settings are `ConfigSetting`s for `model`, `effort`, and `mode`.
- **Refused settings fail the first prompt.** A new session awaits the agent's answer to each of its `config` settings before it sends its first prompt. When the agent refuses one, or reports no option with that ID, the first prompt is not sent: `Session::run` records `Change::PromptRejected` for it with an error naming each failed setting (`The agent refused model claude-x: <its error>`, `The agent has no effort option`), which ends its turn with that error in the conversation, as any rejected prompt does. This applies to drafts and requested sessions alike. A resumed session's settings, values the agent accepted before, are still applied without waiting, and a refusal is still only logged. The `session.new` doc's sentence on refused `config` values changes to match.
- **What the user approves.** `ox_protocol::SessionRequest { harness: Harness, model: String, effort: Option<String>, mode: Option<String>, checkout: RequestedCheckout, prompt: String }`, where `model`, `effort`, and `mode` are the config's names and `RequestedCheckout` is `Workspace { path }` or `Worktree { branch: Option<String>, base: String }` (`base` is the workspace's branch, else `HEAD`), serialized as the UI's `SessionRequest` type. `Session::ask(title, request, options)` records it: `Change::PermissionRequested` and `EventKind::PermissionRequested` gain `session_request: Option<SessionRequest>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, so stored events read), the agent path passes `None`, and the request has no description. `Item::Permission` carries it to the UI. The daemon's params struct becomes `RequestParams`. The uncommitted UI gains `mode: string | null` on `SessionRequest`; `SessionRequestView` shows the mode's name after the effort in the harness line, as a `mode-name` span added to the `.request-panel .effort-name` rule in `composer.css`; the mock scenario's request adds `mode: "Manual"`.
- **Listing for agents.** New `ox-mcp` tool `list_harnesses {}`: "List the harnesses, models and effort levels a session can be started with, as start_session takes them." One block per harness, a `modes: <id>, <id>` line when it has modes, and one line per model: `- <id> (<name>[, <provider>]), effort: <id>, <id>` or `, no effort levels`; its description says it lists modes too. `start_session` takes `harness` and `model` as required strings and `effort` and `mode` as optional; its description names `list_harnesses` and says effort is required when the model has effort levels and mode when the harness has modes.
- **Install.** The example is `config/config.json`. `make install` copies it to the config path, by the same XDG rule, only when no file exists there, and prints the path it installed.

## Check your assumptions

1. **The example config's contents.** Changed: Pi lists only the three OpenAI models and DeepSeek V4.1 Flash; the OpenCode Go models and OpenRouter Kimi K3 are dropped. Claude's list stands.
   - **Decided:** Claude: Opus 5.5 (`claude-opus-5-5`, Medium, High), Sonnet 5.5 (`claude-sonnet-5-5`, High, XHigh), Haiku 5.5 (`claude-haiku-5-5`, Low, Medium); modes Manual, Auto, Accept edits, Plan. Pi: GPT-6.1 Sol (`openai/gpt-6.1-sol`), GPT-6.1 Terra (`openai/gpt-6.1-terra`), GPT-6 Luna (`openai/gpt-6-luna`), provider OpenAI, each Low, Medium, High, XHigh; OpenRouter DeepSeek V4.1 Flash (`openrouter/deepseek/deepseek-v4.1-flash-1017`, the smoke model), Low, Medium, High; no Pi modes. The IDs come from Pi's model list on this computer, which has Pi's `openai` (API key) and `openrouter` providers but not `openai-codex` or `opencode-go`; the effort levels are a guess at "best value".
   - **Consequences:** Every computer installs exactly this list, and a model or effort level the user's Pi or Claude does not accept fails when a session starts on it.
   - **Alternative:** The user gives the model IDs, providers, and effort levels for each model.
   - **Trade-offs:** The guessed list ships without another round trip but may name the wrong OpenAI provider, the wrong OpenCode Go and OpenRouter models, and effort levels the user would not pick; the user's list is right the first time.

2. **What a new draft starts on before anything is used.** Kept.
   - **Decided:** The first model, effort level, and mode listed, which is why the example lists Manual first; after that, the values last used with the harness in the workspace, as today.
   - **Consequences:** The order of the config sets the defaults, and Claude's Mode picker lists Manual before Auto.
   - **Alternative:** Start on the harness's own default by sending no setting until the user picks one.
   - **Trade-offs:** First-listed is visible in the picker and needs no knowledge of the harness's defaults; the harness's default keeps today's picker order but the draft cannot show what the session will start on.

3. **Which harnesses the app offers, and what happens without a config.** Kept.
   - **Decided:** The harnesses that are both installed and listed. A missing or unreadable config shows its error each time the window gains focus and offers no harness; session requests are refused with the same error.
   - **Consequences:** An installed harness left out of the config disappears from New Session. Running sessions of an unlisted harness show no model picker.
   - **Alternative:** Treat a missing config as an empty one, silently, or offer every installed harness and show pickers only for listed ones.
   - **Trade-offs:** The error tells the user what to fix on a computer `make install` has not set up; silence is quieter but leaves an app that cannot start sessions with no explanation.

4. **A value the harness refuses when a session starts.** Changed: the refusal shows as an error in the session's conversation (see Refused settings fail the first prompt).
   - **Decided:** The first prompt is not sent, and its turn ends with an error naming each refused setting, so a session never silently runs on a model, effort, or mode other than the one chosen.
   - **Consequences:** A typo in the config shows as a turn error on the session's first prompt; the user picks another value and sends again. The first prompt waits for the agent to answer the settings.
   - **Alternative:** Record the refusal in the session's conversation as an error.
   - **Trade-offs:** Keeping the contract adds nothing; showing the refusal makes a bad config obvious but adds an event and its rendering.

5. **Bridges stop listing models and levels.** Kept.
   - **Decided:** Both bridges report only each option's current value and pass any value to the harness; Pi's `thinking` option is renamed `effort`.
   - **Consequences:** Pi's effort slider is labelled Effort instead of Thinking. Pi no longer reports which models it can reach, so a running Pi session on an unlisted model shows the model's ID. Standalone ACP clients of the bridges see no choices; `spec/claude.md` drops its model table.
   - **Alternative:** Keep the bridges' lists and validation, and filter them in the UI.
   - **Trade-offs:** Dropping the lists deletes code, stops Pi's ~420 models from being stored in every session's events, and lets the config name any model; keeping them leaves Claude refusing any model missing from its built-in list.

6. **The mode of a requested session.** Changed: a request carries a mode, required when the config lists modes for the harness, checked against the config, and shown in the request panel.
   - **Decided:** `start_session` and `session.request` take `mode`, checked like effort; the panel shows its name after the effort.
   - **Consequences:** An agent requesting a Claude session names its mode, and the user approves it.
   - **Alternative:** Start requested sessions in the first mode the config lists, or add a `mode` to the request.
   - **Trade-offs:** No mode keeps the request as the brief describes it; the first listed mode makes it follow the config at the cost of one more setting the user does not see in the request.

## Test plan

- `ox-protocol`: `load` reads the example shape in order (`Harness::ALL`), with provider, efforts, and modes; refuses an unknown harness key and an unparsable file, naming the file; a missing file is an error naming its path. The `config_dir_from` rule (absolute `XDG_CONFIG_HOME`, else `~/.config`, else temp) as the removed test had it. `config/config.json` parses (test reads it by `CARGO_MANIFEST_DIR`).
- `oxd` (`tests/agents.rs`): a `Daemon` helper writes `ox/config.json` under the daemon's config home, listing `claude` with `fast` (Low, Medium, High) and `smart` (no levels), modes Ask, Code, and `pi` with `fast` (no levels) and no modes.
  - An approved request's `PermissionRequested` has `session_request` `{ harness: claude, model: "Fast", effort: "Low", mode: "Code", checkout: worktree on topic from main, prompt }` and no description; the started session reports `model` fast, `effort` low, and `mode` code.
  - A workspace-checkout request carries `checkout: { type: "workspace", path }`.
  - Refused without asking: an unlisted harness, an unlisted model, a missing effort for `fast`, an unlisted effort, an effort for `smart`, a missing mode for `claude`, an unlisted mode, a mode for `pi`, each naming what the config lists; the existing branch refusals stay.
  - `list_harnesses` through the fake agent's `mcp` directive prints the config.
- `oxd` (`tests/daemon.rs`): a new session whose `config` the fake agent refuses (an unknown model value) sends the agent no prompt, and its first prompt's turn ends with an error naming the model. A setting for an option the agent does not report fails the first prompt the same way. An accepted `config` is applied before the agent receives the first prompt.
- `ox-mcp`: `format_harnesses` output for a harness with modes, a model with provider and efforts, and one without levels.
- `claude-code-acp`: setting `model`, `effort`, and `mode` to values outside the old lists sends them to Claude and reports them as current; the `--describe` test goes.
- `pi-acp`: the fake Pi answers only `get_state` and `get_commands`; the options test checks `model` and `effort` built from state; `effort` maps to `set_thinking_level`; config tests use `effort`; the `--describe` tests go.
- `ox` core (`conversation.rs`): a `PermissionRequested` with a session request folds into an `Item::Permission` carrying it.
- UI (`drafts.test.tsx`, replacing "A draft's controls"):
  - A draft offers the config's models, the chosen model's effort levels, and its modes, starting on the first listed; choosing a model with other levels shows those levels and sends the first; choosing a model with none hides the slider.
  - A draft starts on the values last used in the workspace, including the mode, and sends them.
  - A Pi draft shows no Mode picker.
  - A starting session shows the sent values, disabled.
  - Slash commands do not complete in a draft.
  - Unsent work survives a restart without probing anything.
- UI (`Composer.test.tsx`): a running session that reports a model the config lists, among many it does not, offers only the config's models at the reported value; a reported model the config lacks shows its ID. The old "shows its description under its title" test becomes a permission test (a description still renders for agent permission requests); the uncommitted session request test expects the mode after the effort (`"Claude CodeOpusHighManual"`).
- e2e: the fixture writes `ox/config.json` under the run's `XDG_CONFIG_HOME`, listing `claude`; the existing session specs pass.

## Implementation plan

1. `crates/ox-protocol`: add `src/config.rs` (`config_dir`, path, types, `load`, tests) and export it; add `SessionRequest`, `RequestedCheckout` to `messages.rs`; add `session_request` to `EventKind::PermissionRequested`; change `Method::SessionRequest` (`harness: String`, `model: String`, `effort` and `mode` as `Option<String>`) and its doc, and `SessionNew`'s `config` doc; update `Client::request_session`; delete `describe` and `Controls`.
2. `crates/claude-code-acp`: as in Decisions (bridge, runner, main, `tests/protocol.rs`).
3. `crates/pi-acp`: as in Decisions (bridge, options, main, `tests/protocol.rs`).
4. `crates/oxd/src/bin/oxd-fake-agent.rs`: delete `--describe`.
5. `crates/oxd`: `state.rs` and `session.rs` carry `session_request`; `Session::ask` takes a `SessionRequest`; `Session::run` awaits a new session's settings and rejects its first prompt on a refusal; `daemon.rs` `request_session` checks against `harnesses::load()`, builds the `SessionRequest` and settings, and drops `settings` and the description; rename the params struct `RequestParams`; tests as above.
6. `crates/ox-mcp/src/main.rs`: `list_harnesses` and `format_harnesses`; `start_session` fields and description; module doc.
7. `crates/ox`: `agents` returns the installed listed `HarnessConfig`s; delete `agent_controls`; `conversation.rs` `Item::Permission.session_request`.
8. UI: `protocol.ts` (`HarnessConfig`, `ModelConfig`, `Choice`; drop `Controls`); `useAgents` returns `HarnessConfig[]` (App passes their IDs to `SessionList`); `controls.ts` as in Decisions; `App.tsx` builds draft and starting options with `draftOptions` and passes the session's `HarnessConfig` to `SessionComposer`, which uses `sessionOptions`; `DraftComposer` takes options instead of `Controls`; `test/core.tsx` and `ui/mock/main.ts` answer `agents` with config fixtures and drop `agent_controls`; `SessionRequest.mode` in `protocol.ts`, `Permission.tsx`, `composer.css`, and `ui/mock/scenarios.ts`; tests as above.
9. `config/config.json`, the example; `Makefile` `install` copies it when absent.
10. `ui/e2e/wdio.conf.ts`: write the config; drop the fixture bridge's `--describe` branch.
11. `scripts/smoke-acp.py`: drop `check_describe`; Pi sets `effort`.

## Documentation updates

- `docs/dev/architecture.md`: replace the `--describe` paragraph with Ox's config file (location, the `harnesses` key's format, that the first listed model, effort, and mode are a draft's defaults, who reads it); add `list_harnesses` to the `ox-mcp` table; the session request paragraph checks harness, model, effort, and mode against the config and records the requested session for the panel; local presentation state no longer holds agent controls.
- `README.md`: the install section says `make install` sets up `~/.config/ox/config.json` from `config/config.json` when absent, and that its `harnesses` key lists what Ox offers; the features line says model, effort, and mode controls.
- `spec/sessions.md` (Create a session steps 4 and 6), `spec/agents.md` (Change session settings): the pickers offer what the harness config lists; drafts have no slash commands; a setting the agent refuses at start fails the first prompt with an error instead of running it (sessions step 7).
- `spec/claude.md`: drop the `--describe` section; the models table becomes the defaults (Opus 5.5, High, Manual) and that any value is passed to Claude.
- `docs/dev/acp-v2.md` (`session/set_config_option` row): Claude and Pi both `model`, `effort`, `mode` (Pi has no mode), any value passed through, no provider metadata.
