# Clients state intent; oxd sequences session loads and config

## Goal

Close the Structure item "Clients sequence session lifecycle themselves" in `.agents/todo.md` and the two Desktop bugs it underlies, "A slow session switch can undo a later one" and "Concurrent config edits can drop a pending setting".

When the work is done, a client never waits on one session operation to decide the next, and never sends settings it did not change:

- Focusing a session loads it when it is exited and can load, so selecting a session switches to it at once. `session.load` leaves the protocol.
- `session.set_config` sets the settings it names and leaves the others as they are. In a running session the model picker works as a draft's does: a pick shows at once, from `config.json`, and sends only what it changed.

The clients are the desktop and the MCP tools (`oxctl` no longer exists). The MCP `send_message` path already loads through the daemon and sends no config.

## Related code

- `ui/src/features/sessions/SessionList.tsx:184-194` — `select` awaits `load_session` for an exited session before `onSelect`. This causes the slow-switch bug.
- `ui/src/features/sessions/useFocus.ts` — sends `focus` with the selected session while the window is in front, and `null` otherwise.
- `ui/src/features/composer/Composer.tsx:246-285` — `SessionComposer`, its `sent` ref (`base`/`config`), and `setOption`, which sends the whole config that `changedConfig` builds. This causes the dropped-setting bug.
- `ui/src/features/composer/controls.ts` — `valuesOf`, `draftOptions` (the draft rule for a model's effort: the current effort if the model lists it, else its first level), `sessionOptions`, and `changedConfig` (`:215-236`).
- `ui/src/App.tsx:95-166` — a draft keeps its picks as `Values` and shows `draftOptions` of them. This is the pattern the session composer follows.
- `ui/src/features/composer/ModelPicker.tsx:189-211` — `useChosen`, which shows a chosen value until the option has it or the change fails.
- `crates/ox/src/lib.rs` — `watch` forwards each focus change with `client.focus` (`:135-147`). It also holds the `load_session` command (`:407-415`) and its registration (`:684`), and `utf8_env` (`:655`).
- `crates/ox-protocol/src/messages.rs` — `Method::SessionLoad` (`:112-125`), `Method::SessionSetConfig` (`:271-281`), `Method::Focus` (`:355-362`), and the parse tests at `:1093-1130`.
- `crates/ox-protocol/src/client.rs` — `load_session` (`:382`) and `focus` (`:684`).
- `crates/oxd/src/conn.rs:183-197` — the `Focus` handler.
- `crates/oxd/src/daemon.rs` — the `Daemon::set_config` doc (`:276`), and `handle`'s `SessionLoad` arm (`:317-321`).
- `crates/oxd/src/session.rs` — `Session::load` (`:294-303`), `load_if_exited` (`:304-319`), `resume`, `focus` (`:521-526`), and the `set_config` doc (`:694`).
- `crates/oxd/src/state.rs` — `settle_config` (`:1235-1267`) and its test `a_settled_config_takes_what_the_agent_accepted` (`:3368`).
- `crates/oxd/tests/` — 25 `load_session` calls across `daemon.rs`, `worktrees.rs`, `agents.rs` and `import.rs`. The config tests are `a_loaded_session_sends_its_recorded_config_again` (`daemon.rs:1521`) and `set_config_replaces_the_config_and_the_next_prompt_runs_with_it` (`:1560`). Also `only_an_exited_session_that_started_can_be_loaded` (`:2210`) and the focus tests (`:3161-3270`). `tests/common/mod.rs` holds the shared helpers.

## Decisions

- **Focus loads.** `Method::Focus { session_id, env }` gains `env` (`#[serde(default)]`): the client's environment for an agent the focus starts.
  - `Session::focus(self: &Arc<Self>, env)` sets the focus as it does now. When the session is `exited`, not archived, and has an agent session ID, it also spawns a task that runs `load_if_exited(env)`. The check runs under the state lock before the focus answers, so focusing a session that cannot load spawns nothing.
  - The focus answers `null` at once. A refused load, such as a missing directory, a branch move under way or a storage error, is logged as `oxd: session <id>: <message>`.
  - A later focus does not cancel an earlier one's load. The earlier session finishes loading, and parks later like any unviewed session.
  - The Tauri `watch` task sends `client.focus(target, utf8_env())`.
- **`session.load` goes.** `Method::SessionLoad`, `Client::load_session`, the Tauri `load_session` command, `handle`'s arm and `Session::load` are deleted. `load_if_exited` is the one way to load, used by focus and by `send_message`.
- **`session.set_config` sets only what it names.** Its type stays `{ session_id, config: Vec<ConfigSetting> }`, and so do `Client::set_config`, the Tauri command and the daemon path.
  - `State::settle_config` loses its branch that replaces the whole config when every setting is accepted. Each accepted setting replaces the one with its ID, or is added, and the other settings keep their values. A refused setting changes nothing.
  - An accepted `model` sent without an `effort` removes the session's effort, because a model without effort levels takes none.
  - The docs on `Method::SessionSetConfig`, `Daemon::set_config` and `Session::set_config` say "sets" rather than "replaces". `session.new`'s `config` still gives the whole initial config.
- **A pick sends only what it changed.** `changedConfig` becomes `pickedSettings(harness, shown, configId, value): ConfigSetting[]`, the settings one pick sends.
  - A model pick sends the model, plus the effort the draft rule picks with `draftOptions(harness, { model: value }, valuesOf(shown))`, or the model alone when the model lists no levels. The model and effort always go together because Pi resets its effort on a model change.
  - Any other pick sends its one setting.
  - `shown` is the config the picker shows, so a model pick keeps an effort the user has just picked and not yet seen listed.
- **The session composer shows picks at once.** `SessionComposer` replaces its `sent` ref with `picked`: the settings the user has picked that `session.config` does not show yet, by ID, with `null` for an effort a model pick removed.
  - The pickers show `sessionOptions(harness, shown)`, where `shown` is `session.config` with `picked` applied. A model pick therefore shows the model, its effort levels and its effort immediately, as in a draft.
  - An entry leaves `picked` when `session.config` shows its value. When the agent refuses a pick, the error shows and the pick's entries leave `picked` if no later pick has replaced them, so the pickers show what the agent runs.
  - `useChosen` and its use in `ModelPicker` are deleted, because the composer now passes options that already show the pick. A draft's picks already update its options synchronously.
- **Selecting a session.** `SessionList` passes `onSelect` straight to `SessionRow`. The `select` callback and its comment are deleted.

## Check your assumptions

1. **How a refused load on focus reaches the user.** *Kept.*
   - **Decided:** oxd logs it and the session stays Exited. The focus itself succeeds, because the desktop forwards focus from its watch task, which ignores focus answers and must not wait on a load.
   - **Consequences:** Selecting a session whose directory no longer exists, or one in a checkout during a branch move, no longer shows an error dialog. The session shows Exited with its composer disabled. A session focused during a branch move loads only when it is focused again.
   - **Alternative:** Record a refused load in the session's conversation as an agent-exit error.
   - **Trade-offs:** Logging adds no events but hides the reason. Recording shows the reason to every client, but repeats it on every focus while the cause lasts.
2. **When a session loads.** *Kept.*
   - **Decided:** A session loads whenever a client focuses it while it is exited, unarchived, and has started before.
   - **Consequences:** Bringing the window to the front, or the desktop reconnecting after an oxd restart, also loads the shown session if its agent exited or crashed. Today only a click on the row does. A session archived and unarchived while it is shown waits for the next focus.
   - **Alternative:** Load only when focus moves to a different session than the connection's last one.
   - **Trade-offs:** The decided rule is one condition and restarts a crashed agent on the next focus. The alternative avoids retrying a failing agent on every window switch, but adds per-connection state.
3. **How a picker change appears and what it sends.** *Changed: a pick shows at once in the desktop and sends only what it changed; oxd records a setting once the agent accepts it.*
   - **Decided:** The session composer shows each pick at once and sends only the changed settings. A model pick sends the model and the effort the draft rule picks.
   - **Consequences:** A model pick shows the new model's effort levels at once in a running session. Today they appear only after the agent answers. Another desktop window sees a change once the agent accepts it. A refused pick shows the error and the value the agent runs.
   - **Alternative:** oxd records a setting when it is requested and undoes it on refusal.
   - **Trade-offs:** The decided form keeps one meaning for the session's config, the values the agent accepted, and keeps the pick in the client that made it. The alternative shows a pick in every client at once, but needs pending values and undo logic in oxd.

## Test plan

- `state.rs`, replacing `a_settled_config_takes_what_the_agent_accepted`:
  - An accepted setting replaces the one with its ID or is added, the others keep their values, and `RowChange::Config` is pushed.
  - A refused setting leaves its value, and the error reads "The agent refused effort max: no".
  - An accepted `model` without an `effort` removes the effort. A model sent with an effort keeps both.
- `crates/oxd/tests/daemon.rs`:
  - `set_config_replaces_the_config_and_the_next_prompt_runs_with_it` becomes `set_config_sets_only_its_settings_and_the_next_prompt_runs_with_them`. A session with a model, effort and mode is set to a new mode. Its config keeps the model and effort, and the next prompt runs with the new mode.
  - New: two `set_config` requests, each naming a different setting, are sent on two connections without waiting, and both values end up in the config. This tests the dropped-setting bug.
  - `only_an_exited_session_that_started_can_be_loaded` becomes `focus_loads_only_an_exited_session_that_started`. An unknown ID is refused with `NOT_FOUND`. Focusing an idle, a never-started, or an archived session leaves its state unchanged once the focus answers.
  - New: focusing exited session A and then exited session B on one connection leaves the connection focused on B, and both sessions load.
  - Every other `load_session` caller loads through a new `common::load(client, session_id)` helper. It focuses the session and waits on a watch until the session leaves `exited`. Tests that expected a refused load (`worktrees.rs:1512` and `:2084`, and the missing-directory case in `worktrees.rs:677`) focus the session, then `wait_for_log` the refusal.
- `ui/src/features/sessions/SessionList.test.tsx`: "views a saved session by loading its agent" becomes a test that selecting an exited session shows it and focuses it, with no `load_session`. New: selecting exited session A and then session B leaves B shown. This tests the slow-switch bug.
- `ui/src/features/composer/Composer.test.tsx`, "The model picker":
  - Picking the mode sends only `mode code`.
  - Picking Deep shows Deep with its levels and its effort before any listing, and sends `model deep` with that effort. Picking Tiny, which lists no levels, sends `model tiny` alone and hides the effort.
  - Picking effort High and then model Fast before any listing sends `effort high`, then `model fast` with `effort high`. Both show at once. This tests the dropped-setting bug.
  - A refused pick shows the error, and the picker shows the session's config again. A listing that changes another setting while a pick is pending keeps the pick shown.
  - "keeps recorded settings the config does not list when another changes" is deleted, because a pick no longer sends them.
- `crates/ox-protocol/src/messages.rs`: the `session.load` parse case is deleted, and a `focus` case parses with and without `env`.

## Implementation plan

1. In `crates/ox-protocol/src/messages.rs`, delete `Method::SessionLoad`. Add `env` to `Focus`, and document that focusing an exited, unarchived session that has started loads it with `env`, and that a refused load is logged. Reword the `SessionSetConfig` doc as in Decisions. Update the parse tests.
2. In `crates/ox-protocol/src/client.rs`, delete `load_session`, and make `focus` take `(session_id, env)`.
3. In `crates/oxd/src/state.rs`, change `settle_config` and its doc as in Decisions, and replace its unit test.
4. In `crates/oxd/src/session.rs`, delete `Session::load`, make `focus` take `env` and spawn the load, and reword the `set_config` doc.
5. In `crates/oxd/src/daemon.rs`, delete the `SessionLoad` arm and reword the `set_config` doc. In `crates/oxd/src/conn.rs`, make `Focus` pass `env` to `session.focus`.
6. In `crates/oxd/tests/common/mod.rs`, add `load`, then move the integration tests to it and to `focus(.., env)` as the Test plan says.
7. In `crates/ox/src/lib.rs`, delete the `load_session` command and its registration, and make the watch task send `utf8_env()` with each focus.
8. In `ui/src/features/sessions/SessionList.tsx`, delete `select` and pass `onSelect` to `SessionRow`.
9. In `ui/src/features/composer/controls.ts`, replace `changedConfig` with `pickedSettings`. In `Composer.tsx`, replace the `sent` ref with `picked` and `shown`, make `setOption` send `pickedSettings` and clear the pick on refusal, and make the `SessionComposer` doc say that a pick shows at once and sends only what it changed. In `ModelPicker.tsx`, delete `useChosen` and make `onChange` return `Promise<unknown>`.
10. Update the UI tests as the Test plan says.
11. In `.agents/todo.md`, tick "Clients sequence session lifecycle themselves", "A slow session switch can undo a later one", and "Concurrent config edits can drop a pending setting".

## Documentation updates

- `docs/dev/architecture.md:221-229`: `session.set_config` sets the settings it names. Each accepted setting replaces the one with its ID, and a model sent without an effort removes the effort.
- `docs/dev/architecture.md`, "Loading and stopping agents" (`:274-290`): focusing an exited session that has started loads it.
- `docs/dev/glossary.md`, **Focus**: focusing an exited session that has started loads it.
- `spec/sessions.md`, "Resume a session": showing the session while the window is in front resumes it. A refused resume leaves the session Exited, without an error dialog.
- `spec/agents.md`, "Change session settings": a pick shows at once, including a new model's effort levels and effort. A pick sends only what it changed, and a model pick also sends its effort. Accepted changes show in every client. A refusal shows the agent's error, and the pickers show what the agent runs.
