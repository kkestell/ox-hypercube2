# Claude Code session import

## Goal

Import the interactive Claude Code sessions started in a workspace's directory as archived Ox sessions with their full, timed history, when the workspace is added and on request.

**Brief:** `agents/briefs/2026-11-17-001-claude-code-session-import.md`

## Related code

- `crates/claude-code-acp/src/history.rs` — `History` finds a session's transcript under `<claude dir>/projects/*/<id>.jsonl` by matching the first `cwd` entry exactly (`transcript_cwd`), and reads its `user`/`assistant` entries and subagent transcripts.
- `crates/claude-code-acp/src/updates.rs` — `Updates::replay` turns transcript entries into ACP updates (`user_message`, `event`, `subagent_event`, `fail_open_tools`). Every update goes out through `send`, which notifies the ACP connection; `permission` and `ask_user` also send requests on it.
- `crates/claude-code-acp/src/main.rs` — handles `--describe <dir>`, then works out the Claude config dir (`CLAUDE_CONFIG_DIR`, else `~/.claude`) and serves ACP.
- `crates/claude-code-acp/src/runner.rs` — `message_events` (skips `isMeta` entries).
- `crates/ox-protocol/src/client.rs` — `describe` runs a bridge mode with the client's `env` and parses its stdout; the pattern for the new modes.
- `crates/ox-protocol/src/messages.rs` — `Method` (`WorkspaceAdd`, `SessionLoad`'s doc), `EventKind::PromptStarted`, `TurnEnded`, `Harness::bridge`.
- `crates/oxd/src/daemon.rs` — `handle`, `add_session` (saving and registering a session), `Daemon::new` (restored sessions as `State::restored`), `request_session` (finds the bridge beside `current_exe`), `forget` (deletion).
- `crates/oxd/src/conn.rs` — the list of methods answered in their own task (`answer_later`).
- `crates/oxd/src/store.rs` — schema (`CREATE TABLE IF NOT EXISTS`), `add_session`, `remove_session`, `write_events`' insert.
- `crates/oxd/src/state.rs` — `State::load` requests replay when the history has no `AgentStarted`.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — stands in for `claude-code-acp` in `Options { bridges: true }` daemon tests; has a `--describe` mode.
- `crates/ox/src/lib.rs` — Tauri commands (`add_workspace`, `utf8_env`).
- `ui/src/features/sessions/SessionList.tsx` — `addWorkspace`, the workspace heading's context menu (Show Archived, Remove Workspace). `ui/src/features/sessions/SessionList.test.tsx`, `ui/src/test/core.tsx` (`core.on`, `core.invoked`, `core.choose`).
- Transcript facts checked on this machine: each entry has an ISO `timestamp`; the first entry with `entrypoint` says `cli` (interactive terminal), `claude-desktop`, or `sdk-cli` (`claude -p`, the Agent SDK, and Ox's own bridge sessions); titles are `{"type":"custom-title","customTitle"}` (from `/rename`) and `{"type":"ai-title","aiTitle"}` entries, repeated through the file.

## Decisions

- **The bridge reads transcripts; `oxd` records them.** `claude-code-acp` gains two modes beside `--describe`, both run with the client's environment so `CLAUDE_CONFIG_DIR` applies, and neither starts Claude:
  - `--sessions <dir>` prints a JSON array `[{"sessionId", "startedAt"}]` of the interactive sessions started in `<dir>`, newest first (ties by ID). `startedAt` is the first entry's `timestamp` in epoch milliseconds.
  - `--transcript <dir> <id>` prints `{"title", "updates": [{"at", "update"}]}`: the updates `Updates::replay` produces, each with the `timestamp` of the entry that produced it. Updates from `fail_open_tools` take the last entry's time. An entry without a timestamp takes the previous one's. A missing transcript exits non-zero with `session <id> not found` on stderr.
- **Interactive** means the first `entrypoint` is `cli` or `claude-desktop` (`INTERACTIVE` in `history.rs`). This also leaves out Ox's own sessions, which are `sdk-cli`.
- **Title.** The last `custom-title`, else the last `ai-title`, else `acp_bridge::title` of the first user text. `null` when none.
- **`Updates` output.** `Updates` holds `output: Output`, `enum Output { Client(ConnectionTo<Client>), Listed(Mutex<Vec<SessionUpdate>>) }`. `Updates::new` builds `Client`; `Updates::listed(session_id)` builds `Listed`, and `take_listed()` drains it. `send` pushes onto `Listed`; `permission` and `ask_user` take the connection with `expect("listed updates ask nothing")`. `replay` is split into `replay_entry(entry, &mut subagents)` and the closing `fail_open_tools()`, so the transcript mode can stamp each entry's updates.
- **Daemon method.** `workspace.import_claude_sessions { workspace_id, env }`, answered in its own task (added to the `conn.rs` list) with `ImportedSessions { imported }`, the number of sessions it saved, when done. It runs the Claude bridge beside the daemon's executable (`daemon::bridge(Harness::Claude)`, extracted from `request_session`) with `--sessions`, then, newest first, for each session Ox does not know: `--transcript`, then saves and registers it before going to the next, so each appears as soon as it is read. A listing failure fails the method; a transcript failure is logged to `oxd.log` and skipped.
- **Known sessions.** A Claude session is known when a registered session has its ID as `agent_session_id`, or the new `deleted_agent_sessions (agent_session_id TEXT PRIMARY KEY)` table holds it. `Store::remove_session` inserts the session's `agent_session_id` there, when set, in the same transaction, so every deletion is remembered. The check runs once before reading the transcript and again under the registry lock when saving, so concurrent imports of one workspace save each session once. The new table is created by `CREATE TABLE IF NOT EXISTS`, so existing databases need no migration.
- **Imported session.** Saved by `Store::import_session(&SavedSession, &[Event]) -> number` in one transaction: the `sessions` row with title, `agent_session_id` and `created_at` = `startedAt`, the `archived` row, and the events. `command` is the bridge path, `cwd` the workspace path, no worktree. It is registered as `State::restored(id, title, agent_session_id, last_seq, Queue::default())` with `archived: true`, exactly as a session restored at startup.
- **Imported history** (`oxd::import::events`). Each `UserMessage` update with content becomes `PromptStarted { prompt, from: None }`, preceded by `TurnEnded { stop_reason: Some(EndTurn), error: None }` when a prompt came before it; every other update is an `Update` event; a final `TurnEnded` follows the last event when there was a prompt. Each event's `recorded_at` is its update's `at`; a `TurnEnded` takes the time of the event before it. Seqs run from 1.
- **Load without duplicate replay.** `State::load` requests replay only when the history holds neither `AgentStarted` nor `PromptStarted`. Every existing session that has a prompt also has an agent start, so only imported sessions change: they resume without replay. `SessionLoad`'s doc says "a session whose history holds no agent start and no prompt records the replay".
- **Desktop.** Tauri command `import_claude_sessions(workspace_id)` with `utf8_env()`, returning the count. `addWorkspace` invokes it with the added workspace's ID without awaiting it and shows nothing on success; the heading menu gets **Import Claude Code Sessions** between Show Archived and Remove Workspace, always enabled, which shows an info message box (`message` from `@tauri-apps/plugin-dialog`, as `showError` uses) when it finishes: `Imported 1 session`, `Imported N sessions`, or `No new Claude Code sessions` for zero. Errors go through `showError`, as other actions do.

## Check your assumptions

1. **Which Claude sessions count as interactive.** Kept.
   - **Decided:** Entrypoint `cli` or `claude-desktop`. On this machine, first entrypoints are 364 `cli`, 14 `claude-desktop` and 669 `sdk-cli`, and `sdk-cli` transcripts carry `promptSource: "sdk"` where `cli` ones carry `typed`.
   - **Consequences:** Sessions from the Claude desktop app's Code tab in the workspace directory are imported too. Agent SDK programs and Ox's own bridge sessions are skipped along with `claude -p`.
   - **Alternative:** `cli` only.
   - **Trade-offs:** Including `claude-desktop` brings in every conversation a person typed; `cli` only matches "sessions run in a terminal" exactly but leaves desktop-app conversations out.

2. **How imported conversations are divided into turns.** Kept.
   - **Decided:** Each user message becomes a prompt that starts a turn, and the turn before it ends as completed.
   - **Consequences:** Imported conversations look like native ones: each turn's work folds under Worked, and `read_transcript` shows the user's prompts. An interrupted turn shows as completed, and transcript lines like `[Request interrupted by user]` or slash-command tags show as prompts.
   - **Alternative:** Record the replay as `session.load` records it today: user messages as updates, no turns.
   - **Trade-offs:** Turns make imported sessions read and fold like Ox's own; the plain replay adds nothing but shows every tool call unfolded and leaves user messages out of `read_transcript`.

3. **The model an imported session continues with.** Kept.
   - **Decided:** Claude's defaults (Opus 5.5, High, Manual), as for any session whose history reports no options.
   - **Consequences:** Unarchiving and continuing a terminal session that used another model or mode switches it to the defaults until the user changes them.
   - **Alternative:** Carry the model of the transcript's last assistant message into the imported history, so resuming sets it.
   - **Trade-offs:** Defaults need nothing more; carrying the model keeps continuity but adds a recorded option update the import has to synthesize.

4. **Who starts the import when a workspace is added.** Kept.
   - **Decided:** The desktop app, which requests `workspace.import_claude_sessions` with its environment after `workspace.add` succeeds.
   - **Consequences:** Claude's config dir comes from the app's environment, as for every agent it starts. A workspace added by another client is not imported until someone runs the menu action.
   - **Alternative:** `workspace.add` starts the import itself, with the daemon's environment.
   - **Trade-offs:** The client-driven call follows how clients pass their environment and choose operations; the daemon-driven one imports for every client but ignores the app's environment.

5. **What the user sees while importing.** Changed: the menu action reports how many sessions it imported when it finishes.
   - **Decided:** The sessions land in the workspace's Archived group, which starts collapsed and shows a growing count. The menu action ends with an `Imported N sessions` message box; the import on add shows none. A failure to list sessions shows an error dialog.
   - **Consequences:** A manual import always gives a visible result, including when nothing was new. An unreadable transcript is skipped with a line in `oxd.log`.
   - **Alternative:** A progress indicator on the workspace heading while the import runs.
   - **Trade-offs:** No indicator adds no UI; an indicator confirms the action ran but needs import state carried to the client.

6. **Imported sessions' titles.** Kept.
   - **Decided:** Claude's own title (a `/rename` title over its generated title), else the first prompt's text.
   - **Consequences:** Imported sessions show the titles Claude Code showed for them. 17 of the 378 interactive transcripts here have no Claude title and get the first prompt.
   - **Alternative:** Always the first prompt's text, as Ox titles its own Claude sessions.
   - **Trade-offs:** Claude's title is shorter and matches what the user saw in Claude Code; the first prompt is uniform with Ox sessions but often long and less descriptive.

## Test plan

- `crates/claude-code-acp/tests/protocol.rs`, with transcripts written under a temporary `CLAUDE_CONFIG_DIR`:
  - `--sessions <project>` lists `cli` and `claude-desktop` sessions started in the project, newest first, and leaves out an `sdk-cli` session, a session started in another directory, and a subdirectory's session.
  - `--transcript` prints the custom title over the AI title; the first prompt's text without either; the replay's user message, agent message, and tool call with each entry's `timestamp` as `at`; a tool call without a result ends `failed` at the last entry's time.
  - `--transcript` of an unknown ID exits non-zero with `not found`.
- `crates/oxd/src/state.rs`: a restored session whose history holds a `prompt_started` loads without replay; the existing replay tests stand.
- `crates/oxd/src/store.rs`: `import_session` round-trips the session as archived with its title, agent session ID, `created_at` and events with their times; `remove_session` remembers the agent session ID and a session without one adds nothing.
- `crates/oxd/src/import.rs`: `events` turns updates into prompts, updates and turn ends with the given times, and adds no turn end without a prompt.
- `crates/oxd/tests/import.rs`, a daemon with `bridges: true` and the fake agent's `--sessions`/`--transcript` reading `fake-transcripts.json` in the workspace directory (`[{"sessionId","startedAt","title","updates"}]`; an entry with `"fail": true` makes `--transcript` exit non-zero):
  - importing answers the number imported and makes each session archived, `exited`, titled, with `created_at` = `startedAt`, the bridge as its command, and its history with the fixture times;
  - importing again adds nothing and answers 0; a session added to the fixture is then the only one imported;
  - a deleted imported session is not imported again;
  - a failing transcript is skipped and the rest are imported;
  - an unarchived imported session loads under `--no-resume-replay` (which rejects a replay request) and keeps its history unchanged.
- `ui/src/features/sessions/SessionList.test.tsx`: Add Workspace (dialog answered through `core.on("plugin:dialog|open", …)`, `add_workspace` answering a workspace) invokes `import_claude_sessions` with its ID; the heading's Import Claude Code Sessions invokes it and shows `Imported 2 sessions` when it answers 2, and `No new Claude Code sessions` when it answers 0.

## Implementation plan

1. `history.rs`: replace `transcript_cwd` with `head(path)` returning the first `cwd`, `entrypoint` and `timestamp` (same 1 MiB cap); `transcript` uses its `cwd`. Add `INTERACTIVE`, `History::sessions(cwd)` and `History::title(id, cwd)`.
2. `updates.rs`: `Output`, `Updates::listed`, `take_listed`, the `replay_entry` split.
3. New `crates/claude-code-acp/src/sessions.rs`: `sessions(history, dir) -> Value` and `transcript(history, dir, id) -> Result<Value, String>` (timestamps parsed with `chrono`). `main.rs`: work out the Claude dir first, then dispatch `--describe`, `--sessions`, `--transcript`. Protocol tests.
4. `messages.rs`: `Method::WorkspaceImportClaudeSessions` (`workspace.import_claude_sessions`) with its doc, and `ImportedSessions { imported: u64 }`; `SessionLoad`'s doc. `client.rs`: `Client::import_claude_sessions(workspace_id, env) -> ImportedSessions`.
5. `store.rs`: `deleted_agent_sessions`, `import_session`, `was_deleted`, `remove_session` remembering; tests.
6. `state.rs`: the load rule; test.
7. New `crates/oxd/src/import.rs`: `ClaudeSession`, `Transcript`, `TimedUpdate`, `sessions` and `transcript` (running the bridge as `describe` does), `events`; tests. `daemon.rs`: `bridge(harness)` extracted, `import_claude_sessions`, `add_imported`, the `handle` arm. `conn.rs`: the method runs in its own task.
8. `oxd-fake-agent.rs`: `--sessions <dir>` and `--transcript <dir> <id>` over `fake-transcripts.json`, documented in its header. `crates/oxd/tests/import.rs`.
9. `crates/ox/src/lib.rs`: `import_claude_sessions` command returning the count, registered. `SessionList.tsx`: `addWorkspace` and the menu item with its message; tests.

## Documentation updates

- `spec/workspaces.md`: adding a workspace imports its Claude Code sessions in the background, newest first, as archived sessions; Import Claude Code Sessions in the heading's menu imports those not yet in Ox and reports how many it imported. Rules: only interactive sessions started in the workspace directory itself; deleted sessions are never imported again; imported messages show their original times.
- `spec/sessions.md`, Archive and restore: an imported session is archived and continues like any archived Claude session once unarchived.
- `spec/claude.md`: a section for `--sessions <dir>` and `--transcript <dir> <id>`, beside Describe a new session.
- `docs/dev/architecture.md`: persistence gains the remembered agent session IDs of deleted sessions, and imported events carry the transcript's times; session lifecycle gains a paragraph on importing through the bridge's two modes.
- `README.md` features: Claude Code sessions run outside Ox imported, archived, with their history.
