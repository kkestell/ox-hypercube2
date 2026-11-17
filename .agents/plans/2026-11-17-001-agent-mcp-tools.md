# Agent MCP tools: messages, transcripts, and session requests

## Goal

Give agents `ox` tools to list and message other standalone sessions, read any
session's transcript, and ask the user to start a new session.

**Brief:** `.agents/briefs/2026-11-16-006-agent-mcp-tools.md`

## Related code

- `crates/ox-mcp/src/main.rs` — the `ox` MCP server; one `set_title` tool that opens a daemon connection per call.
- `crates/oxd/src/mcp.rs` — `OxServer::for_session` builds the `ox-mcp` command line for a session.
- `crates/oxd/src/state.rs` — `State::request`, `send_now` (starts a turn or steers, and resumes a held queue), `Phase::Starting`, `Change::PermissionRequested`, `Pending`.
- `crates/oxd/src/session.rs` — `Session::launch`, `load`, `prompt`, `permit`, `history`; `run` applies `Opening.options` to options the agent reported when its session opened.
- `crates/oxd/src/daemon.rs` — `handle`, `new_session`, `with_new_worktree`, `add_session`, `add_step_session` (finds a harness's bridge beside `oxd`), `send_unmoved`, `branches`, `lifecycle`.
- `crates/oxd/src/conn.rs` — `answer_later` runs long requests in their own task.
- `crates/oxd/src/worktree.rs` — `plan`, `WorktreePlan::saved` (always `created_branch: true`), `create_branch`, `add`, `roll_back`, `checked_out_at`, `apply_branch_rule` (already keeps a branch the daemon did not create).
- `crates/oxd/src/run.rs` — `configure` sets model and effort by option category; the `'reported` loop waits for Pi's options after the session opens.
- `crates/acp-bridge/src/lib.rs` — `Bridge::new_session` (synchronous) and `serve`, which answers `session/new` and then calls `start_new`.
- `crates/pi-acp/src/bridge.rs` — `new_session` answers without config options; `start_new` starts Pi and publishes them later in a `config_option_update`.
- `crates/ox/src/lib.rs` — `agent_controls` runs a bridge's `--describe`; `Controls`.
- `crates/ox/src/conversation.rs` — `Item::Permission` (title and options only); `Item::Prompt` and `Item::Steered`, folded from `PromptStarted`, `PromptSteered` and `SteeredPromptTaken`.
- `ui/src/features/composer/Permission.tsx`, `ui/src/lib/protocol.ts` — the permission panel and its item type.
- `ui/src/features/conversation/Conversation.tsx` — `ItemView` renders `prompt` and `steered` items.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — `mcp-title` directive and `call_tool`; `crates/oxd/tests/common/mod.rs` — `Options::bridges` links the fake agent as each bridge.
- `~/src/hypercube/internal/mcp/read_session.go` — the compact transcript format and paging this plan follows.

## Decisions

- **Daemon methods.** Each tool maps to one daemon method, called by `ox-mcp` with its own session ID:
  - `session.send_message { session_id, to, text }` → `MessageSent { steered: bool }`.
  - `session.transcript { session_id, before: Option<u64> }` → `TranscriptPage { text, before: Option<u64> }`.
  - `session.request { session_id, prompt, workspace: Option<PathBuf>, worktree: bool, branch, harness, model, effort }` → `Option<SessionInfo>`, `null` when the user declines or the request is cancelled.
  - `list_sessions` reads the first batch of a `watch` in `ox-mcp`; the listing already has workspaces, sessions, worktrees and runs, so no daemon method is needed.
  
  All three new methods go through `answer_later` in `conn.rs`: they wait on agent startup, history loads, git, or the user.
- **Peer message delivery.** `State::send_now` gains a `resume_queue` flag; `Request::Prompt` passes `true`, the new `Request::Message` passes `false`, so a peer message starts a turn or steers exactly as a prompt does but leaves a held queue held. `Daemon::send_message` loads an `exited` recipient (under its lifecycle lock, after `history()`, refusing during a branch move, as `SessionLoad` does), waits with `Session::started()` until it leaves `starting`, then sends through `send_unmoved`. A recipient whose agent exits while starting returns its error to the sender. `Session::started()` subscribes to `last_seq` and rechecks `state()` after each change; it never reads the state inside the watch's `wait_for` closure, because `update` sends on `last_seq` while holding the state lock.
- **Peer message sender.** `EventKind::PromptStarted` and `PromptSteered` gain `from: Option<Sender>`, `Sender { session_id, title: Option<String> }` as of sending, with `#[serde(default, skip_serializing_if = "Option::is_none")]` so stored events still read. The event's `prompt` is the sender's text alone. The agent gets the text after a header, `Message from Ox session <id> ("<title>"). Your reply in this conversation does not reach it; answer with the ox tool send_message.` and a blank line (the title part omitted for an untitled sender); `State::request` returns this agent prompt for `Command::Prompt` while recording the plain text. The desktop core carries `from` on `Item::Prompt` and `Item::Steered`, including a steered prompt that moves when taken. The conversation shows a peer message as a prompt with the sender's title, or `Session <first 8 digits of its ID>` when untitled, on a line above the bubble, styled as the existing muted small text. The compact transcript writes `Message from <id> ("<title>"): <text>` in place of `User:`.
- **Who can message.** Sender and recipient must both be standalone (`Session::is_child()` is `run_focused.is_some()`). Refused: a run's child session (`invalid_request`), the sender itself, an archived session, a session whose agent never started (`Phase::Exited(None)`), and an unknown or deleted ID (`not_found`).
- **Environment.** `Session` keeps the environment its agent was launched with in memory (`env`, set in `launch`). A load that a peer message triggers, and a requested session, use the sending or requesting session's environment, as a client's environment is used for its own loads and sessions.
- **Session requests.** `Daemon::request_session` checks everything before asking: the workspace (by its directory; defaults to the requester's), the harness (defaults to the requester's, from its command; its bridge must exist beside `oxd`), the checkout, and model and effort. For a worktree it calls `worktree::plan` and discards the plan, which checks the working tree, the commits, and a named branch. For the workspace checkout, a named branch must equal `Branches::head`. Model and effort are run through the harness bridge's `--describe` in the workspace directory, matched by option category (`Model`, `ThoughtLevel`) and checked against the option's values; a refusal lists the values. Then `Session::ask` records a permission request on the requester with options `start` ("Start Session", `allow_once`) and `decline` ("Decline", `reject_once`), title `Start a session in <workspace directory name>?` and a description of the harness, model, effort, checkout and branch lines followed by a blank line and the prompt. `start` calls `new_session` with the bridge command, the requester's environment, `Checkout::Worktree { compare: head, branch }` or `Checkout::Workspace`, the prompt as a text block, and the matched `ConfigSetting`s. Any other outcome returns `null` and creates nothing.
- **Existing branch worktrees.** `Checkout::Worktree` gains `branch: Option<String>`. `worktree::plan` takes it: the branch must be a local branch (`refs/heads/<branch>`) checked out in no worktree; the base is its tip and `WorktreePlan.created_branch` is `false`. `with_new_worktree` skips `create_branch`, and `roll_back` deletes the branch only when the plan created it. Deletion already keeps a branch the daemon did not create.
- **Options at session open for Pi.** `Bridge::new_session` returns a future, as `resume_session` does, and `serve` spawns it before calling `start_new`. Pi's `new_session` starts Pi and describes it before answering, so its response carries config options and commands and `Opening.options` applies to Pi; a Pi that fails to start is logged, the response has no options, and the first prompt starts Pi again as today. `start_new` reports idle. Claude's `new_session` is unchanged inside a ready future. The `'reported` wait in `run.rs` goes away.
- **Describe in one place.** `Controls` and `describe(bridge, cwd, env)` move from `crates/ox/src/lib.rs` into `ox-protocol`; the desktop's `agent_controls` and the daemon both call it. `Harness::of_command(&[String])` names the harness whose bridge a command runs; `Daemon::bridge(harness)` replaces the lookup in `add_step_session`.
- **Tools for child sessions.** `OxServer::for_session(id, child)` adds `--child` for a run's child session; `ox-mcp` then removes the `list_sessions`, `send_message` and `start_session` routes, leaving `set_title` and `read_transcript`.
- **Compact transcript.** `oxd/src/transcript.rs` folds a session's events into entries, each keyed by the seq of its first event: `User: <text>` for `PromptStarted` and `PromptSteered`, or `Message from …` for a peer message; `Assistant: <text>` per agent message ID; `Tool: <title>`, with ` (failed)` when its last status is failed; and the turn's end, `(turn finished)` for `end_turn`, `(turn cancelled)`, `(turn failed: <error>)`, or `(turn ended: <stop reason>)`. A page holds the latest entries before `before`, up to 40,000 characters, separated by blank lines; an entry longer than the budget is cut and ends with `…`. `before` in the result is the seq of the page's oldest entry when earlier entries remain. `Session::transcript(before)` loads history and folds under the state lock via `State::events()`.
- **Tool descriptions.**
  - `list_sessions`: "List the Ox workspaces, by directory, and the sessions in them you can message: ID, title, state, harness, model, effort, directory, and branch. An exited session's agent starts again when you message it."
  - `send_message { to, text }`: "Send a message to another Ox session by its ID. An idle session starts a turn with it; a busy one takes it into its current turn. An exited session is started again. Its reply arrives as a message from it, if it sends one."
  - `read_transcript { session, before? }`: Hypercube's `read_session` description with `before` in place of `cursor`, and any session ID.
  - `start_session { prompt, worktree, workspace?, branch?, harness?, model?, effort? }`: "Ask the user to start a new, independent Ox session with prompt as its first message. worktree true runs it in a new worktree, on branch if given (an existing local branch) or a new branch from HEAD; false runs it in the workspace's checkout, whose branch must match branch if given. workspace is a workspace directory from list_sessions and defaults to yours; harness (claude or pi) defaults to yours; model and effort are values the harness offers and default to its own. Waits for the user's answer and returns the new session's ID."

## Check your assumptions

1. **How the user approves a requested session.** Kept.
   - **Decided:** A daemon-made permission request in the requesting session's composer, using the existing permission panel, which now also shows a request's `description` (harness, model, effort, checkout, branch, then the prompt) under its title. Permission requests already carry `description`, and the panel shows only the title today.
   - **Consequences:** The requesting session shows the attention state while it waits. The user approves with Start Session or Decline; stopping the turn cancels the request.
   - **Alternative:** A dedicated approval card with its own event kind and layout.
   - **Trade-offs:** The permission panel reuses the existing state, dot, answer path and run-view rendering; a dedicated card could lay out the settings and prompt more clearly but needs a new event, protocol method and component.

2. **How a peer message appears in the recipient's conversation.** Changed to the alternative: the sender is recorded on the event.
   - **Decided:** `PromptStarted` and `PromptSteered` record the sender (`from`); the event keeps the plain text, and only the agent's copy carries the header with the sender's ID. The conversation shows the sender's title above the prompt, and the transcript writes `Message from <id> ("<title>"): <text>`.
   - **Consequences:** Peer messages read as messages from another session rather than from the user; the change touches the event, the desktop core and the UI.
   - **Alternative:** Record only the header in the prompt text, shown to the user as part of a user bubble.
   - **Trade-offs:** The header needs no protocol or UI change; a `from` field gives a clearer display at the cost of event, desktop core and UI changes.

3. **Pi's `session/new` waits for Pi to start.** Kept.
   - **Decided:** Change the bridge contract so Pi's `session/new` starts Pi and returns its config options. Today `oxd` applies a new session's options only to options reported when the session opens, which Pi does not report, so model and effort given to a new Pi session, including from a desktop draft, are dropped.
   - **Consequences:** Model and effort apply to requested Pi sessions and to Pi drafts. A new Pi session's `agent_started` arrives once Pi has started, about as long as Pi takes to start; the first prompt was waiting for that anyway.
   - **Alternative:** Leave `pi-acp` as it is and accept that model and effort for a requested Pi session are ignored, as they are for Pi drafts today.
   - **Trade-offs:** The change fixes both paths and removes `run.rs`'s wait for Pi's options, at the cost of touching the shared bridge trait; leaving it keeps the change smaller but delivers the brief's model and effort for Claude only.

4. **Run child sessions get only `set_title` and `read_transcript`.** Kept.
   - **Decided:** Children have no `list_sessions`, `send_message` or `start_session`, since the brief limits messaging to standalone sessions.
   - **Consequences:** A workflow step cannot ask the user to start a session.
   - **Alternative:** Also give children `start_session`, with its approval shown in the run view.
   - **Trade-offs:** Withholding it keeps unattended runs from stopping for an approval; offering it lets a step hand off work but can stall a run until the user answers.

5. **Model and effort are checked before the user is asked.** Kept.
   - **Decided:** The daemon runs the harness bridge's `--describe` in the workspace directory and refuses an unknown model or effort, listing the values the harness offers.
   - **Consequences:** A request with a model or effort takes the few seconds `--describe` needs before the approval appears, and agents learn valid values from the refusal.
   - **Alternative:** Pass the values unchecked; a value the bridge refuses leaves the harness's default.
   - **Trade-offs:** Checking gives the agent and user accurate settings at the cost of a delay; passing them through is instant but can start a session on a different model than the user approved.

6. **Defaults for `start_session`.** Kept.
   - **Decided:** `worktree` is required; the workspace and harness default to the requester's; model and effort default to the harness's own.
   - **Consequences:** Agents must choose a checkout explicitly, and a requested session does not inherit the requester's model or effort.
   - **Alternative:** Default `worktree` to true and inherit the requester's model and effort when the harness matches.
   - **Trade-offs:** Explicit checkout avoids a session silently editing the shared checkout or making a branch; inheriting settings saves the agent arguments but carries over choices the user made for another task.

7. **Transcript tool entries show the tool call's title.** Kept.
   - **Decided:** `Tool: <title>` uses the title Ox shows for the call, the tool's name followed by its main argument, such as ``Tool: Read `src/main.rs` ``. ACP tool calls carry no bare tool name.
   - **Consequences:** Entries are slightly longer than Hypercube's name-only lines and say what each tool acted on.
   - **Alternative:** Cut the title at its first code span to keep the name only.
   - **Trade-offs:** The full title is more useful to a reader and needs no parsing; the name alone matches Hypercube exactly but depends on the bridges' title format.

## Test plan

- `state.rs` unit tests: a peer message on an idle session starts a turn and on a running one steers it, each event recording `from` and the plain text while the returned agent prompt carries the header; a peer message after a cancelled turn starts a turn and leaves the held queue held after that turn ends.
- `ox-protocol` serialization test: a stored `prompt_started` without `from` still reads.
- `transcript.rs` unit tests: prompts, steered prompts, peer messages (`Message from …`), chunked agent messages, a failed tool call, and each kind of turn end fold into the expected lines; a page stops at the budget and names `before`; `before` reads the earlier page; an over-budget entry is cut with `…`; an empty history gives no entries.
- `oxd` integration tests in `crates/oxd/tests/agents.rs`, with the fake agent calling `ox-mcp` through `mcp <tool> <JSON>`:
  - `send_message` to an idle session starts a turn: its `prompt_started` has the text and `from` with the sender's ID and title, the agent's `user_message` echo has the header, and the tool result says it started a turn.
  - To a session held in a `wait` turn, it records `prompt_steered`.
  - To a parked session (`park_after_ms: Some(0)`), and to a session restored after a daemon restart, it loads the agent and starts a turn.
  - Refused for an archived session, a session whose agent never started, a run's child session, the sender itself, and an unknown ID, each with its message.
  - `read_transcript` returns the compact text of another session, pages with `before`, and reads an archived session.
  - `list_sessions` lists workspaces and standalone sessions, leaving out the caller, archived sessions, run children and never-started sessions.
  - `start_session` (daemon with `bridges: true`): approving starts a session with the prompt as its first prompt, titled from it, in a worktree on an existing branch with the requested model and effort set; declining creates no session, worktree or branch; a missing branch, a checkout branch that differs from `HEAD`'s, and an unknown model are refused with no permission request (proved by the turn ending with the refusal and no `permission_requested`).
  - A run child's agent gets `unknown tool` for `send_message`.
- `crates/oxd/tests/worktrees.rs`: `session.new` with `Checkout::Worktree { branch }` on an existing local branch adds the worktree on it with `created_branch: false`; a missing branch and a branch checked out elsewhere are refused; deleting the session keeps the branch.
- `pi-acp` `tests/protocol.rs`: `session/new` answers with Pi's config options and commands; update `a_new_session_starts_pi_and_reports_idle`.
- `crates/ox/src/conversation.rs`: a permission request's description reaches `Item::Permission`; a peer message's `from` reaches `Item::Prompt`, and `Item::Steered` and the prompt it becomes when taken.
- UI tests: the permission panel shows a request's description under its title; a peer message shows its sender's title above the prompt, and `Session <id8>` for an untitled sender.

## Implementation plan

1. `ox-protocol`: add `Checkout::Worktree.branch`, `Sender` and `from` on `PromptStarted` and `PromptSteered`, the three methods with their docs, `MessageSent`, `TranscriptPage`, `Harness::of_command`, `Controls` and `describe`, and `Client::send_message`, `transcript`, `request_session`. Update the `session.new` doc for `branch`.
2. `acp-bridge`, `pi-acp`, `claude-code-acp`: make `Bridge::new_session` return a future; Pi starts and describes before answering; update the Pi protocol test.
3. `crates/ox/src/lib.rs`: `agent_controls` calls `ox_protocol::describe`.
4. `oxd/src/worktree.rs`: `plan` with an existing branch, `WorktreePlan.created_branch`, and `roll_back` deleting only a created branch. `daemon.rs`: `new_session` and `with_new_worktree` pass and honor them. Add the worktree tests.
5. `oxd/src/state.rs`: `Request::Message { text, from }`, the `resume_queue` flag on `send_now`, the header on the returned agent prompt, `State::events`; set `from: None` on every other prompt event; unit tests.
6. `oxd/src/session.rs`: `env`, `is_child`, `message`, `started`, `ask`, `transcript`. `oxd/src/transcript.rs` with its unit tests.
7. `oxd/src/daemon.rs`: `bridge`, `send_message`, `transcript`, `request_session`; route the methods in `handle` and through `answer_later` in `conn.rs`. `mcp.rs`: the `child` flag. `run.rs`: remove the `'reported` loop and its TODO.
8. `ox-mcp`: `--child`, the four tools and their descriptions, and the `list_sessions` formatting from the first `watch` batch.
9. Fake agent: replace `mcp-title` with `mcp <tool> <JSON arguments>` (update `agent_sets_title_through_ox_mcp_server`), add `--describe <dir>` printing its config options, and skip a first line starting with `Message from Ox session` so a peer message's text runs as its script. Add `crates/oxd/tests/agents.rs`.
10. `crates/ox/src/conversation.rs` and `ui/src/lib/protocol.ts`: `description` on the permission item and `from` on the prompt and steered items. `Permission.tsx`: show the description under the title, wrapping its lines. `Conversation.tsx`: the sender line above a peer message. UI tests.

## Documentation updates

- `docs/dev/architecture.md`: the ox-mcp section lists the tools, which child sessions lack, and the daemon methods they call; Session lifecycle describes peer messages (recorded with their sender, start or steer without resuming a held queue, loading an exited recipient with the sender's environment) and requested sessions (approval through a permission request, the requester's environment).
- `docs/dev/glossary.md`: add **Peer message**: a message one standalone session's agent sends another through the `ox` tools, recorded with its sender; it starts a turn or steers like a prompt sent now, without resuming a held queue.
- `docs/worktrees.md`: a requested session can run in a worktree on an existing local branch, which Ox does not delete; adjust "Ox creates the branch for every worktree session".
- `docs/dev/acp-v2.md`: the `session/new` row: Pi's bridge starts Pi before answering.
- `README.md`: a feature bullet for agents messaging sessions, reading transcripts, and asking to start sessions.
