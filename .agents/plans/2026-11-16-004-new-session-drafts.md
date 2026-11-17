# New sessions start on first send

## Goal

New Session opens a draft that exists only in the desktop app. Sending its
first message creates the session, with that message as its first prompt; the
daemon records the prompt at once and starts the worktree and agent behind it.

**Brief:** `.agents/briefs/2026-11-16-003-new-session-drafts.md`

## Related code

- `crates/ox-protocol/src/messages.rs` — `Method::SessionNew`, `ConfigValue`, `EventKind::AgentStarted` ("the first event of a session that starts").
- `crates/ox-protocol/src/client.rs:342` — `Client::new_session`.
- `crates/oxd/src/daemon.rs:657` — `new_session`, `with_new_worktree`, `add_session`; `start_in_worktree` calls `Session::start` after the worktree is added. `daemon.rs:190` drops never-started checkout sessions at restart.
- `crates/oxd/src/session.rs` — `Launch`, `Session::start`, `launch` (creates the command channel), `run` (the resume path already sets saved option values where the agent's differ, before the command loop).
- `crates/oxd/src/state.rs` — `Phase::Starting`, `Phase::after`, `State::request` (prompts refused while starting), `check_prompt`.
- `crates/acp-bridge/src/lib.rs:165` — `title`: the 120-character title both bridges give a session from its first prompt.
- `crates/claude-code-acp/src/bridge.rs` — `Session::options` (the model, effort, and mode options and their defaults), `start_claude`; `runner.rs:145` builds claude's command line and `runner.rs:431` takes the commands from claude's `initialize` answer.
- `crates/pi-acp/src/bridge.rs:283` — `Bridge::describe`, which asks a running Pi for its state, models, thinking levels, and commands; `pi.rs:112` starts Pi.
- `crates/acp-bridge/src/lib.rs` — `serve`, the entry point both bridges' `main` call.
- `crates/ox/src/lib.rs:346` — Tauri `new_session` and `prompt` commands; `utf8_env` is the environment sessions get.
- `crates/oxctl/src/main.rs:196` — `oxctl new`.
- `ui/src/App.tsx` — the viewed session or run, the composer, `useDrafts`.
- `ui/src/features/composer/useDrafts.ts` — per-session unsent text, in memory only.
- `ui/src/features/composer/Composer.tsx` — the session composer; takes a `SessionInfo`.
- `ui/src/features/sessions/SessionList.tsx` — the New Session menu (`newSession`, `newWorktreeSession`), rows, `runTitle`.
- `ui/src/features/sessions/SessionHeader.tsx` — the subheader showing a session's checkout.
- `ui/src/features/files/useFileViewers.ts`, `treeState.ts` (`forgetDeletedTrees`), `RightPanel.tsx` — file panels keyed by view ID, fed from `views` in `App.tsx`.
- `ui/src/features/composer/ModelPicker.tsx`, `ui/src/components/SelectPicker.tsx` — the pickers, which have no disabled state.
- `ui/src/components/HarnessLogo.tsx` — `sessionAgent`.
- `ui/e2e/helpers.ts:204` — `newWorktreeSession`.

## Decisions

- **Names.** A *draft* is a new session not yet sent: its workspace, agent, checkout, and config option choices. *Unsent text* is the text and attachments in any composer, a session's or a draft's. The existing `Draft` type and `useDrafts` hook, which hold unsent text, become `Unsent` and `useUnsent`. An agent's config options and slash commands for a workspace, as a draft shows them, are its *controls*.
- **`session.new` carries the first prompt and option values.** It gains `prompt: Option<Vec<ContentBlock>>` and `config: Vec<ConfigSetting>` (`ConfigSetting { config_id, value: ConfigValue }`), both `#[serde(default)]` so `oxctl new` and the tests keep creating sessions without them. The daemon owns the sequencing, so the prompt shows to every client from the moment the session exists and the desktop core does not wait on the agent.
- **The first prompt is recorded before the agent starts.** `State::queue_prompt` records `PromptStarted` while the session is `starting` and keeps its seq in `Phase::Starting { prompt: Option<u64> }`. `Change::Started` makes the live turn `Turn::Prompted(seq)`. `Change::Exited` from `Starting` with a queued prompt records `TurnEnded { error }` with the exit's error (or "the agent exited before the prompt") before `AgentExited`, so a failed start or a failed worktree add ends the turn. `session.prompt` stays refused while starting.
- **A first prompt holds only text and resource links**, checked with `check_prompt` against empty capabilities before the session is saved, since the agent's capabilities are unknown until it starts. The desktop sends exactly these.
- **The agent task sets the options, then sends the prompt.** `Launch::New` carries an `Opening { step, options, prompt }`. `run` applies `options` with the loop the resume path uses (renamed from `saved_options`; set only where the agent's reported value differs; a refusal goes to oxd's log). `launch` queues the first prompt as `Command::Prompt` on the new command channel, so the command loop sends it after the option requests, with the instructions preamble when the agent takes no system prompt. A worktree session's opening travels through `add_session` and `start_in_worktree` to `Session::start`.
- **The daemon titles the session from the first prompt.** `add_session` sets the title with `acp_bridge::title` of the prompt's text through `Session::set_title`, the same title both bridges send once they get the prompt. The row then shows the prompt text from the first listing until the agent sets its own title, across reloads and clients. `oxd` gains `acp-bridge` as a dependency.
- **Bridges describe their controls.** `claude-code-acp --describe <dir>` and `pi-acp --describe <dir>` print one JSON object, `{"configOptions": [...], "availableCommands": [...]}` in ACP's schema, for a new session in `<dir>`, and exit. Neither sends a prompt or saves a session. The Claude bridge prints `Session::options()` at its defaults and the commands of claude's `initialize` answer, starting claude in `<dir>` as `start_claude` does, with `--no-session-persistence` and no `--session-id`, and stopping it once answered. The Pi bridge starts Pi with `--no-session` in place of `--session-id` and prints `Bridge::describe`'s options and commands. `main` checks for `--describe` before `acp_bridge::serve`.
- **The desktop core runs the probe.** Tauri command `agent_controls(agent, cwd)` runs the agent's bridge from beside the app with `--describe <cwd>` and `utf8_env()`, and returns `{ config_options, available_commands }`; the UI passes the workspace's path. A bridge that exits with an error fails the command with its standard error.
- **Controls are cached in local storage and refreshed.** `useControls(agents, workspaces)` keeps each probe result under `controls:${agent}:${workspaceId}`. At start, and whenever the installed agents or listed workspaces change, it probes each pair with no entry, one at a time. Viewing a draft probes its pair again in the background; the draft shows the cached entry until the new one replaces it. A pair with no entry yet uses the agent's entry from another workspace, so pickers show at once in a new workspace. A failed probe leaves the entry as it was and logs to the console; a probe that never ends gets a `TODO`.
- **Values default to the last used.** `useLastUsed(sessions)` saves, per agent and workspace, the option values of the newest session that reports options, skipping run children, whose options come from workflow steps; sending a draft also saves its values. A draft shows the probed options with each current value taken from the draft's `config`, else the last-used value when the option still offers it, else the probe's. Sending passes every shown option's value in the probed order, and the daemon sets those the new agent reports differently. Effort levels stay the probed list when the user picks another model in a draft; a level the agent then refuses is logged and the agent's default stands.
- **A new session's composer keeps its pickers while the agent starts.** A session that is `starting` with `agent_session_id === null` shows the cached controls of its agent and workspace at the last-used values, with the pickers disabled, until the agent reports its own. `ModelPicker` and `SelectPicker` gain `disabled`.
- **The desktop core builds the prompt for both commands.** A `prompt_blocks(text, attachments)` helper in `crates/ox/src/lib.rs` serves `prompt` and `new_session`.
- **Drafts and unsent text live in `localStorage`**, under `drafts` and `unsent`, written on each change. `useUnsent` keeps entries whose ID is a listed session or a draft, once connected. Drafts of a workspace that is no longer listed are dropped.
- **An empty draft is dropped when it is not viewed.** `useDrafts` drops every draft with no unsent text other than the viewed one, each time the viewed ID changes and on start. Drafts are newest first at the top of their workspace, above sessions and runs.
- **A draft view** shows the draft's title in the content header, a subheader with the Folder icon and workspace path for a checkout draft (as `SessionHeader` shows a checkout session) or the `GitBranch` icon and "New worktree" for a worktree draft, an empty conversation, and the composer. Its right panel shows the workspace directory's files, for a worktree draft too, so the user can find the files the prompt refers to; `App.tsx` adds each draft to `views` with the workspace path as `cwd`, so `useFileViewers` and `forgetDeletedTrees` treat it as a view. It has no focus.
- **Sending a draft.** For a worktree draft, `workspace_branches` gives `compare` at send time, as the branch the checkout is on when the session starts. Then `new_session` with the agent, workspace, checkout, text, attachments, and config. On success the app views the new session and drops the draft and its unsent text; on failure it shows the error and keeps both. Send is disabled while the request is in flight.
- **Composer split.** `Composer` becomes the shared entry, attachment, and settings layout, taking the unsent ID, agent, config options, commands, request, the `running`, `disabled` and `idle` flags, and `onSend`, `onStop`, `onSetOption`. `SessionComposer` supplies them from a session as today; `DraftComposer` from a draft and its remembered controls.

## Check your assumptions

1. **Where a draft's model, mode, and slash-command lists come from.** Changed: the pickers must show at once, always, with reasonably current options, and commands must work on the first message.
   - **Decided:** Each bridge's `--describe` mode, run by the desktop core, cached per agent and workspace in local storage and shown at once from the cache.
   - **Consequences:** Ox briefly starts claude or Pi in the background to read their controls. Only on the very first launch, before the first probe returns, can a draft open without pickers, for a few seconds.
   - **Alternative:** Remember the options and commands of the workspace's newest session.
   - **Trade-offs:** The probe always has lists and needs no earlier session, at the cost of short background agent starts; remembering starts nothing but leaves a draft without pickers or commands whenever no session has reported them.

2. **What model and mode a new session gets.** Kept.
   - **Decided:** A draft starts with the last-used values of its agent in the workspace, and sending applies every shown value, so a new session keeps the model and mode the user last used there.
   - **Consequences:** Switching a session to a different model makes later sessions in that workspace start on it. Today every new session starts on the agent's default.
   - **Alternative:** Send only the options the user changed in the draft, and show the pickers at the remembered values marked as defaults.
   - **Trade-offs:** Sticky values make the pickers truthful and match common chat apps; sending only changes keeps agent defaults authoritative but shows values the session may not get.

3. **Empty sessions already saved in existing databases.** Kept: cleanup at startup would be a migration, which the project does not do.
   - **Decided:** Leave them; the user deletes them as today. They have started agents and, for worktree sessions, branches and checkouts, which deletion must check for unsaved work.
   - **Consequences:** Existing empty sessions stay in the sidebar until deleted by hand.
   - **Alternative:** At startup, oxd deletes sessions with no prompt in their history, removing worktrees without unsaved work and their branches by the usual cleanup rule.
   - **Trade-offs:** Leaving them adds no code and risks nothing; cleanup clears the clutter once but needs a startup pass over every session's history and could remove something a user kept on purpose.

4. **What labels a sent session's row before the agent titles it.** Kept.
   - **Decided:** The daemon saves the first 120 characters of the prompt as the session's title at creation, the rule both bridges already apply when the prompt reaches them.
   - **Consequences:** The prompt start is the stored title, visible to `oxctl` and kept across restarts, until the agent sets one. A session started without a prompt, from `oxctl`, still shows "New session".
   - **Alternative:** The desktop keeps a local map from session to prompt text and shows it while the title is unset.
   - **Trade-offs:** The daemon title needs no client state and covers every client; the local map leaves `title` meaning only "set by the agent" but adds client bookkeeping and shows nothing in other clients.

5. **Discarding a draft.** Kept.
   - **Decided:** A draft row's context menu has Delete Draft, as session rows have Delete Session.
   - **Consequences:** One menu item beyond the brief.
   - **Alternative:** No menu; the user clears the text and moves away.
   - **Trade-offs:** The menu item matches the session rows and makes discarding direct; leaving it out keeps the change to the brief's behaviors.

6. **When Session in Worktree checks for a checked-out branch.** Kept.
   - **Decided:** At send, when the branch to compare with is read, so it is the branch at the session's start.
   - **Consequences:** In a workspace on a detached HEAD, the "is not on a branch" error appears on Send, and the draft keeps its text. Today it appears on New Session.
   - **Alternative:** Check at New Session as well, refusing to open the draft.
   - **Trade-offs:** Checking once at send is simpler and uses the branch the session actually starts from; checking at New Session as well reports the problem before the user types.

7. **The file panel for a draft.** Changed: the user needs to see files while writing a prompt, because prompts often refer to them.
   - **Decided:** A draft view's right panel shows the workspace directory's files, for checkout and worktree drafts alike.
   - **Consequences:** A worktree draft shows uncommitted files its worktree will not have.
   - **Alternative:** No right panel for drafts.
   - **Trade-offs:** The panel lets the user find and name files while writing; leaving it out avoids the worktree mismatch but blocks the main use.

8. **When probes run.** Kept.
   - **Decided:** At start and when a workspace is added, each installed agent and workspace pair with no cached controls is probed, one at a time; viewing a draft probes its pair again in the background.
   - **Consequences:** After the first launch, a launch starts no probes unless a workspace or agent is new; a draft's controls are as fresh as its last view.
   - **Alternative:** Probe every pair at every launch, as well as on viewing a draft.
   - **Trade-offs:** Probing what is missing keeps launches quiet; probing everything keeps every entry fresh but starts agents times workspaces processes at each launch.

9. **What a new session's composer shows while its agent starts.** Kept.
   - **Decided:** The cached pickers at the values just sent, disabled, until the agent reports its own options.
   - **Consequences:** The pickers never disappear after Send.
   - **Alternative:** Hide the pickers until the agent reports options, as new sessions do today.
   - **Trade-offs:** Disabled pickers keep the layout and the user's choice visible; hiding them needs no extra state but the pickers flash out and back.

## Test plan

Daemon integration tests (`crates/oxd/tests/daemon.rs`, `worktrees.rs`):

- `a_new_sessions_first_prompt_shows_before_its_agent_starts`: a session whose fake agent is held starting (`--hold`) has `PromptStarted` as its first event and its title from the prompt in the listing; after release, `AgentStarted` follows and the turn runs and ends.
- `a_new_session_sets_its_options_before_its_first_prompt`: `session.new` with `config` `mode=code` and the prompt `config mode` gives the chunk `code`.
- `a_first_prompt_ends_when_the_agent_does_not_start`: an agent that exits before opening (`--no-sessions`) records `TurnEnded` with an error, then `AgentExited`.
- `a_first_prompt_with_content_beyond_text_and_links_is_refused`: an image block is refused with `INVALID_REQUEST` and no session is saved.
- `worktrees.rs`: a worktree session with a first prompt runs it in the worktree once added; a worktree that cannot be added ends the first prompt's turn with the add's error.

Bridge protocol tests (`crates/claude-code-acp/tests/protocol.rs`, `crates/pi-acp/tests/protocol.rs`), with the existing fake claude and fake pi:

- `--describe <dir>` prints the options and the commands the fake reports, runs the fake in `<dir>`, passes `--no-session-persistence` (claude) or `--no-session` (pi) and no session ID, and exits 0.
- A fake that fails to start makes `--describe` exit non-zero with its error on standard error.

Live check: `scripts/smoke-acp.py` gains `check_describe`, which runs the bridge's `--describe` in the smoke project and checks for a model option and the smoke command marker; run `make smoke-claude` and `make smoke-pi` once each.

Unit tests (`state.rs`): `queue_prompt` while starting records `PromptStarted`; `Started` makes the session `running` on that turn; `Exited` from starting ends the turn before `AgentExited`.

UI tests (`ui/src/features/sessions/drafts.test.tsx`, replacing the New Session case in `SessionList.test.tsx`):

- Session in Checkout and Session in Worktree add a viewed draft row at the top of the workspace and invoke no `new_session` or `workspace_branches`.
- Typing labels the row with the text; a draft with text keeps it after viewing a session and coming back.
- Viewing something else drops an empty draft; Delete Draft drops a draft with text.
- Two New Session clicks make two drafts with their own agent and checkout.
- Send invokes `new_session` with agent, workspace, checkout, text, attachments, and config, views the session, and drops the draft; a worktree draft first reads `workspace_branches`; a failed send shows the error and keeps the draft and text.
- With no cache, the app invokes `agent_controls` for each installed agent and workspace, one at a time; with a cache, it invokes none at start, and viewing a draft invokes it for the draft's pair.
- A draft shows the cached model picker and slash commands at once, takes a pair's missing entry from another workspace, and shows a refreshed probe in place.
- A draft's values come from the newest session of its agent in the workspace; changing the model sends it in `config`.
- After Send, the starting session's composer shows the pickers disabled at the sent values, and the session's own options once it reports them.
- A draft view's right panel lists the workspace directory, and opening a file from it shows the file viewer.
- Drafts, a session's unsent text, and cached controls survive unmounting and rendering the app again.

`ui/src/performance.test.tsx`: typing in a draft renders no session row.

End-to-end: `newWorktreeSession` sends a prompt after choosing Session in Worktree, and the worktree spec passes unchanged otherwise.

## Implementation plan

1. `crates/ox-protocol/src/messages.rs`: add `ConfigSetting`; add `prompt` and `config` to `SessionNew` and document them; update the `AgentStarted` comment (a first prompt's `PromptStarted` precedes it). `client.rs`: `Client::new_session` takes `config` and `prompt`.
2. `crates/oxd/src/state.rs`: `Phase::Starting { prompt }`, `State::queue_prompt`, the `Started` and `Exited` transitions above; make `check_prompt` public. Unit tests.
3. `crates/oxd/src/session.rs`: `Opening`, `Launch::New(Opening)`, `Session::start(env, opening)`, `Session::queue_prompt`; `launch` queues the first prompt; `run` applies `options` for both launches. Update the `add_step_session` call in `daemon.rs` to pass `Opening { step: Some(role), .. }`.
4. `crates/oxd/src/daemon.rs`: `new_session` checks the prompt, converts `config` to option values, and passes both to `add_session`, which queues the prompt, sets the title, and hands the opening to `start` or `start_in_worktree`. `crates/oxd/Cargo.toml`: add `acp-bridge`. Add a `TODO` in `daemon.rs` restore: a never-started session's first prompt is not shown after a restart.
5. `crates/oxctl/src/main.rs` and the test helpers: pass no prompt and no config. Add the integration tests.
6. `crates/claude-code-acp/src/main.rs`, `bridge.rs`, `runner.rs`: the `--describe` mode; protocol tests.
7. `crates/pi-acp/src/main.rs`, `bridge.rs`, `pi.rs`: the `--describe` mode; protocol tests.
8. `scripts/smoke-acp.py`: `check_describe`.
9. `crates/ox/src/lib.rs`: `prompt_blocks`; `new_session` takes `text`, `attachments`, `config`; `agent_controls`, registered in the invoke handler.
10. `ui/src/lib/protocol.ts`: `ConfigSetting`, `Controls`.
11. `ui/src/features/composer/useUnsent.ts` (from `useDrafts.ts`): rename, persist to `localStorage`, prune against sessions and draft IDs.
12. `ui/src/features/composer/controls.ts`: `useControls` and `useLastUsed`.
13. `ui/src/features/sessions/useDrafts.ts`: the draft list in `localStorage`, `create`, `remove`, `setConfig`, dropping empty unviewed drafts and drafts of removed workspaces.
14. `ui/src/components/SelectPicker.tsx`, `ui/src/features/composer/ModelPicker.tsx`: `disabled`.
15. `ui/src/features/composer/Composer.tsx`: split into `Composer` and `SessionComposer`, with the starting session's cached pickers; add `DraftComposer.tsx` with the send flow.
16. `ui/src/features/sessions/SessionList.tsx`: the menu items create drafts and view them; `DraftRow` (memoized, gray dot labeled "Draft", `draftTitle(text)`, Delete Draft); drafts first in each workspace.
17. `ui/src/features/sessions/DraftHeader.tsx` and `ui/src/App.tsx`: render a viewed draft with its right panel; add drafts to `views`; wire `useUnsent`, `useDrafts`, `useControls`, `useLastUsed`.
18. UI tests, performance case, `ui/e2e/helpers.ts`.

## Documentation updates

- `docs/dev/glossary.md`: add **Draft**.
- `docs/dev/architecture.md`: local presentation state includes drafts, unsent text, and cached controls; the desktop core runs each bridge's `--describe` to learn a draft's controls; a new session can carry its first prompt and option values, which the daemon records at once and sends when the agent starts.
- `docs/worktrees.md`: Session in Worktree creates the worktree and branch when the draft's first message is sent.
- `spec/sessions.md`: Create a session describes the draft, its controls and the workspace's files beside it for referring to files in the prompt, the first send, and draft persistence.
- `spec/claude.md`: the bridge's `--describe` mode.
- `spec/conversations.md`: call the composer's content the message rather than the draft, and state that unsent messages survive restarts.
