# Ox records each session's config

## Goal

A session's model, effort and mode are what the user picked. Ox records them when the session is created and whenever the user changes them, shows that record in the composer, and sends it to the agent. Today the composer shows the options the agent reports. A new Claude session therefore flips to the bridge's defaults (`manual`, `claude-opus-5-5`, `high`, `crates/claude-code-acp/src/bridge.rs:71`) between `session/new` and the daemon's `set_config_option` requests, then flips back. After this change, nothing the agent reports changes what the composer shows.

## Related code

- `crates/ox-protocol/src/messages.rs` — `Method::SessionNew` (`config`), `Method::SessionSetConfigOption`, `ConfigSetting`, `ConfigValue`, `SessionInfo.config_options`, `EventKind::AgentStarted` (stored schema, keeps `config_options`), the request parsing tests near line 1098.
- `crates/ox-protocol/src/client.rs` — `Client::set_config_option`.
- `crates/oxd/src/session.rs` — `Session` fields (`archived` is the pattern for a mutex-held, saved field), `info_of`, `update`/`save` (write changed `SessionInfo` fields to the row), `set_config_option`, `Opening`, `Command::SetConfigOption`, and `run`: the option loop after `open.await` compares against `reported` and pre-checks "The agent has no X option".
- `crates/oxd/src/state.rs` — `Agent.config_options`, `Change::ConfigOptionsSet`, the `Live` + `ConfigOptionUpdate` arm, `Resume.options`, `option_values`, `State::load` (derives options from events).
- `crates/oxd/src/daemon.rs` — `new_session`/`add_session` (builds `Opening`), `set_config_option`, `request_session` (`settings` in model, effort, mode order), `add_imported`, restoring sessions from `store.sessions()`.
- `crates/oxd/src/conn.rs:199` — `SessionSetConfigOption` answered in its own task, sent in connection order.
- `crates/oxd/src/store.rs` — `SavedSession`, `sessions`, `add_session`, `import_session`, `set_title` (setter pattern), `MIGRATIONS`, the fixture tests.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — `mode` starts at `ask`; `--silent-config` and `--late-config-update` exist only to test the composer following agent reports.
- `crates/oxd/tests/daemon.rs` — `current`, `config_options_are_listed_and_set`, `a_prompt_sent_after_a_config_change_runs_with_the_new_value`, the resume test around line 1500, the `config` cases near line 2286.
- `crates/pi-acp/src/bridge.rs` — `set_config` describes Pi after each change; `crates/pi-acp/src/options.rs` — `build`, `command`.
- `crates/ox-mcp/src/main.rs:310-388` — `list_sessions` reads model and effort through `current_value`.
- `crates/ox/src/lib.rs:531` — the `set_config_option` Tauri command; `crates/ox/src/conversation.rs:835`.
- `ui/src/lib/protocol.ts` — `SessionInfo.config_options`.
- `ui/src/features/composer/controls.ts` — `useLastUsed`, `draftOptions`, `sessionOptions`, `withReported`.
- `ui/src/App.tsx:100-113` — `startingConfig`; `ui/src/features/composer/Composer.tsx` — `SessionComposer` (`starting`, `optionsDisabled`, `onSetOption`).
- `ui/src/test/core.tsx`, `ui/mock/scenarios.ts`, `ui/src/features/composer/Composer.test.tsx`, `ui/src/features/sessions/drafts.test.tsx`.

## Decisions

- **The session's config.** `SessionInfo.config_options` becomes `config: Vec<ConfigSetting>`, the session's config: the values Ox set the agent to, in the order they are sent (`model`, then `effort` and `mode` where the session has them). It is the same type `session.new` takes as `config`. `Session` holds it in a `config: Mutex<Vec<ConfigSetting>>`, `info_of` reports it, and `save` writes it with `Store::set_session_config` when it changes. `SavedSession` gains `config`. `session.new` records its `config` when it creates the session, so the composer shows it from the first listing.
- **Changing the config replaces it.** `session.set_config_option { config_id, value }` becomes `session.set_config { session_id, config: Vec<ConfigSetting> }`, with a null result. `Client::set_config` and the Tauri `set_config` command replace the old ones. `Command::SetConfig` sends one `session/set_config_option` per setting, in order, all before any later command, so connection ordering holds as it does now. Once every answer is in, the session's config becomes `config` if the agent accepted every setting. Otherwise each accepted setting replaces the one with its ID, and the error names each refused setting (`The agent refused effort xhigh: <its error>`). Each change sends the whole config, so Pi gets its effort again after a model change, which in Pi resets the thinking level.
- **The agent is told; its answers are not read.** At a new session's start and at a resume, `run` sends every setting in the session's config, without comparing against what the agent reports and without the "has no option" pre-check, because the agent's refusal reports that. A new session still awaits the answers and rejects its first prompt on a refusal. A resume still sends without waiting and logs a refusal. `Opening.options` and `Resume.options` go, and so do `option_values`, `Change::ConfigOptionsSet` and the `Live` handling of `ConfigOptionUpdate` into `agent.config_options`. The agent's `config_option_update`s are still recorded as events, and `AgentStarted` still records what the agent reported, because that is stored schema.
- **A session with no config adopts the agent's.** An imported session, and an older one whose history reports no options, has an empty config. When its agent starts, the session's config becomes what the agent reported in `session/new` or `session/resume`, read by category (`model` → `model`, `thought_level` → `effort`, `mode` → `mode`) by `state::reported_config(&[SessionConfigOption]) -> Vec<ConfigSetting>`. Until then the composer shows no pickers for it.
- **Schema step 2.** `MIGRATIONS` gains `add_session_config`: `ALTER TABLE sessions ADD COLUMN config TEXT NOT NULL DEFAULT '[]'` (JSON array of `ConfigSetting`). It fills each session from its latest `agent_started` or `config_option_update` event through `reported_config`, so existing sessions keep the values they ran with, including a Pi session's older `thinking` option, which has category `thought_level`. A `schema-2.sql` fixture goes with it.
- **Pi refuses an effort its model does not support.** After `set_thinking_level`, `set_config` compares Pi's reported `thinkingLevel` with the requested one and refuses when they differ: `Pi runs <provider/id> at effort <reported>, not <requested>`. Pi clamps a level the model lacks and reports success (`setThinkingLevel`, `agent-session.js:2210`, Pi 1.5.2). The comparison is `options::check_effort(state, requested) -> Result<(), String>`.
- **The composer.** `sessionOptions(harness, config)` builds the selects from the session's config: Model where it has `model`, Effort where it has `effort`, with the recorded model's levels, and Mode where it has `mode`. A recorded value the config does not list shows as its ID, through `withReported` and `ModelPicker`'s fallback. A change computes the next config as `draftOptions(harness, { [configId]: value }, current)`, so a model change brings the effort the draft rule picks, and sends it with `set_config`. When the config file does not list the harness, it replaces only the changed setting. `startingConfig` and `SessionComposer`'s `starting` prop go, and pickers are disabled while the session is `starting`. `useLastUsed` reads `session.config`.
- **Listing for agents.** `ox-mcp`'s `list_sessions` reads `model` and `effort` from `session.config` by ID, and `current_value` goes.
- **Fake agent.** `--silent-config` and `--late-config-update` go, along with the test cases that use them.

## Check your assumptions

1. **A sessions column rather than a side table.** *Kept; the migrations change it builds on is committed (89866ec).*
   - **Decided:** Add schema step 2 to `MIGRATIONS`, because the config is a field of the session row.
   - **Consequences:** Opening an existing database rewrites its sessions table once.
   - **Alternative:** Keep the config in a new `session_config` table created with `CREATE TABLE IF NOT EXISTS`.
   - **Trade-offs:** A side table needs no migration step but adds a second way to evolve the schema; the column follows the mechanism the store documents.

2. **The composer stops following the agent's own reports.** *Kept.*
   - **Decided:** Show only Ox's record. Neither bridge changes a value unless asked (`claude-code-acp/src/bridge.rs:400-420`, `pi-acp/src/bridge.rs:270-285`).
   - **Consequences:** If a harness ever changes its model or mode by itself, Ox keeps showing the old value until the user changes it. Pi's unprompted option update after it starts is ignored.
   - **Alternative:** Accept an agent's `config_option_update` into the record when it differs from what Ox sent.
   - **Trade-offs:** Following updates would catch harness-initiated changes but brings back the flicker and the ordering problem this change removes.

3. **Changing one setting resends all of them, and the old single-option method goes.** *Kept.*
   - **Decided:** `session.set_config` takes the whole config, because Pi resets its effort on a model change and a model without effort levels has to drop the effort.
   - **Consequences:** Changing the mode also resends the model and effort, which costs a few local round trips. `session.set_config_option` stops existing for daemon clients, which are Ox's own UI and tests.
   - **Alternative:** Keep `session.set_config_option` and have the composer send a second request for the effort after a model change.
   - **Trade-offs:** The single method changes less, but it can leave the record and Pi out of step, and the daemon couldn't drop a stale effort.

4. **A config entry naming an effort that the model doesn't support now fails.** *Kept.*
   - **Decided:** The Pi bridge refuses an effort that Pi clamps.
   - **Consequences:** A new Pi session with such an entry gets no first prompt and ends its turn with the error. Changing to it in the composer shows the error. Pi has already switched to the clamped level by then, so it runs that level while Ox's record keeps the previous one.
   - **Alternative:** Accept the clamp and record the effort Pi reports.
   - **Trade-offs:** Accepting it never fails, but it shows a value the user did not pick and hides the config mistake. Refusing follows the rule that a wrong config is an error.

5. **Sessions without a recorded config show no pickers until their agent starts.** *Kept.*
   - **Decided:** Imported sessions, and older sessions whose history reports no options, take what the agent reports when it starts. Import doesn't know what model the transcript used.
   - **Consequences:** An imported Claude session shows no model, effort or mode until it is resumed. It then shows the bridge's defaults (`manual`, Opus 5.5, `high`).
   - **Alternative:** Record the config file's first Claude model, effort and mode at import, and send them on resume.
   - **Trade-offs:** Recording at import shows pickers immediately but makes import depend on the config file, and it may not match what the transcript ran with.

6. **A change shows once the agent accepts it.** *Kept.*
   - **Decided:** The record changes when the agent's answers arrive, as the composer's value does today.
   - **Consequences:** The picker keeps the old value for the round trip and stays there on a refusal.
   - **Alternative:** Record the change at once and roll it back on a refusal.
   - **Trade-offs:** Recording first feels instant but can show a value the agent never accepted.

## Test plan

- `crates/oxd/tests/daemon.rs`:
  - A new session with `config` `mode: code` against the fake agent, which starts at `ask`: every `SessionInfo` the session publishes, from the `session.new` answer through its first turn's end, has `config` equal to the request, and the turn answers `config mode` with `code`.
  - `session.set_config` replaces the config and the next prompt runs with it, replacing `config_options_are_listed_and_set` and `a_prompt_sent_after_a_config_change_runs_with_the_new_value`.
  - A refused setting (`mode: fly`) returns an error naming it, and the config keeps the accepted settings.
  - A loaded session sends its recorded config again: the resume test asserts the agent runs with the recorded mode.
  - A session restored with an empty config adopts the agent's reported values when it loads.
  - Existing new-session `config` refusal cases keep their errors, apart from "has no option", which becomes the agent's refusal.
- `crates/oxd/src/store.rs`: opening `schema-1.sql` fills `config` from the latest `agent_started` or `config_option_update` event, and leaves `[]` for a session without one. `schema-2.sql` opens unchanged.
- `crates/oxd/src/state.rs`: `reported_config` maps categories to `model`/`effort`/`mode`, including an option with ID `thinking`.
- `crates/pi-acp/src/options.rs`: `check_effort` accepts the matching level and refuses a different one with the message above.
- `crates/ox-protocol/src/messages.rs`: `session.set_config` parses, replacing the `set_config_option` cases.
- `ui/src/features/composer/Composer.test.tsx`: a session's pickers show `session.config`, a starting session's pickers are disabled at its config, and changing the model sends `set_config` with the new model and the effort the draft rule picks. `sessionOptions` shows an unlisted recorded effort as its ID.

## Implementation plan

1. `ox-protocol`: replace `SessionInfo.config_options` with `config`, and `Method::SessionSetConfigOption` with `SessionSetConfig { session_id, config }`. Update `Client` and the parsing tests.
2. `oxd/src/store.rs`: add `add_session_config` to `MIGRATIONS` with its backfill. Add `config` to `SavedSession`, `sessions`, `add_session` and `import_session`, and add `set_session_config`. Write `tests/fixtures/schema-2.sql`.
3. `oxd/src/state.rs`: add `reported_config`. Remove `Change::ConfigOptionsSet`, `option_values`, `Resume.options` and the `Live` option tracking. `Agent.config_options` stays for `AgentStarted`.
4. `oxd/src/session.rs`: add the `config` field, report and save it, add `set_config` and `Command::SetConfig`, send the config at start and resume as described, and adopt `reported_config` when the config is empty. Drop `Opening.options`.
5. `oxd/src/daemon.rs` and `conn.rs`: pass `config` into `add_session`, route `SessionSetConfig`, and leave `add_imported` with an empty config.
6. `oxd-fake-agent`: remove `--silent-config` and `--late-config-update`. Update `tests/daemon.rs` and `tests/agents.rs:583`.
7. `pi-acp`: add `options::check_effort`, called from `set_config` for `effort`.
8. `ox-mcp`: read `session.config` in `list_sessions`. `ox`: replace the Tauri command and fix `conversation.rs:835`.
9. UI: update `protocol.ts`, rewrite `sessionOptions`, update `useLastUsed`, remove `startingConfig` and `starting`, send `set_config` from `SessionComposer`, and update the mock, `test/core.tsx` and the tests.

## Documentation updates

- `docs/dev/architecture.md:207-220` and `:266-269`: the daemon records a session's config, the composer shows it, and the daemon sends every setting at start and on load, with a session lacking one adopting the agent's.
- `docs/dev/acp-v2.md:35`: Pi's bridge refuses an effort Pi clamps to another level.
