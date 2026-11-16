# Ox single-user audit

Reviewed `b081952`, which includes the desktop-first scope in `AGENTS.md`. The
desktop is the user's everyday interface. `oxctl` serves agents testing the same
operations or handling occasional tasks. One user can still have several live
sessions, real Git work to protect, and conversations they expect to reopen.

Three parallel reviews covered the daemon, protocol and CLI, the Claude bridge,
and the desktop and its tests. I checked their findings against production
callers, traced the existing reproductions, and reviewed the build, CI and live
smoke script. Released dependencies, generated files and bundled assets were
outside the source review. The previous audit and project notes informed this
pass. I inspected source and existing fixtures. I did not rerun the recorded
reproductions, builds, tests or real Claude sessions.

## Defects in current workflows

1. **Keep the three local-merge and branch-move fixes at the top.** Merge can
   land an obsolete source commit. A checked-out branch move can overwrite an
   ignored local file. An agent can also begin work in the destination while
   Git is moving it, because the busy check runs once before the mutation. All
   three have recorded reproductions and are already in the foundation work.
   Validate the expected source for GUI merge, protect local files the move
   would replace, and coordinate the move with Ox starting or prompting an
   agent in that checkout. Keep the generic ability to move a branch to an
   explicit commit. Scope the concurrency check to Ox’s operations and cover
   each reproduced case.
   [Branch-move reproductions](../../.agents/todo.md#branch-moves-can-overwrite-local-files-or-run-alongside-an-agent)
   · [Stale-source reproduction](../../.agents/todo.md#local-merge-can-land-an-obsolete-task-commit)

2. **Switching tasks loses the prompt and attachments in progress.** The
   desktop keys the composer by session ID. Switching to another task tears down
   the state holding the unsent text and files. Keep one in-memory draft per
   session and clear it on send or deletion. A switch-away-and-back regression
   covers the everyday case. The existing reload test can still expect an
   empty draft. Source: `ui/src/App.tsx:55`,
   `ui/src/features/composer/Composer.tsx:20`, `ui/e2e/specs/commands.e2e.ts:98`.

3. **The desktop’s content policy blocks tool output images.** The renderer
   builds a `data:` URL, and the Tauri policy has no `img-src` allowance for it.
   The existing browser test checks whether an image element is displayed. Add
   `data:` to the image directive and have that test check that the image
   decodes. Source: `crates/ox/tauri.conf.json:28`,
   `ui/src/features/conversation/Conversation.tsx:138`,
   `ui/e2e/specs/session.e2e.ts:152`.

4. **Make replay find the events SQLite actually stored.** The recorded write
   failure leaves a sequence gap. Replay treats the event sequence as an array
   offset and can miss an event that committed after the gap. Find the first
   stored event with `seq > after_seq` and cover a short sparse sequence. The
   same rule serves ordinary replay. A durable write queue, retries, and a
   broader rewrite of write-failure reporting can wait for a concrete need.
   Source: `crates/oxd/src/state.rs:727`, `crates/oxd/src/store.rs:130`.
   [Recorded failure](../../.agents/todo.md#an-event-write-failure-can-break-replay-after-restart)

5. **Archive can block Delete while the agent starts.** The sidebar allows
   Archive during startup. The daemon then waits for initialization while
   holding the same session lock Delete needs to stop a stuck agent. Refuse
   Archive while the agent is starting and keep Delete available. The existing
   stuck-start fixture can exercise this case. Source:
   `ui/src/features/sessions/SessionList.tsx:180`,
   `crates/oxd/src/daemon.rs:573` and `:611`, `crates/oxd/src/session.rs:358`.

## Simplify without changing the workflow

6. **Ask Claude to replay history only when Ox needs it.** Opening a saved
   session loads its SQLite history first. The daemon then asks for full ACP
   replay even when its existing `AgentStarted` check will discard the updates.
   The bridge reads, translates and sends the main and subagent transcripts.
   Use the existing history check to decide whether to send the replay cursor.
   Keep the fallback for missing history and the live state, settings and
   command updates at resume. The bridge’s separate scan for schedules still
   runs. Source: `crates/oxd/src/daemon.rs:226`,
   `crates/oxd/src/session.rs:793`, `crates/oxd/src/state.rs:650`,
   `crates/claude-code-acp/src/bridge.rs:128` and `:457`.

7. **Resolve desktop questions and permissions by ID.** The desktop’s Rust
   transcript fold keeps a second map of open requests so it can send the entire
   question or permission payload again with `answered: true`. React already
   holds each open request and hides answered items. Send the resolved ID over
   the desktop channel and remove it from the existing reducer. Reuse the
   open/resolve tests and leave the saved events and CLI stream as they are.
   Source: `crates/ox/src/conversation.rs:97` and `:243`, `ui/src/App.tsx:25`,
   `ui/src/features/conversation/useConversation.ts:16`.

## Existing behavior worth a scope decision

These would remove supported behavior. The first three offer the clearest cuts
for how the user uses Ox today. The rest depend on what the desktop or standalone
bridge should support.

### Make `oxctl attach` JSONL-only

The CLI spends about 280 lines rendering a second version of the transcript,
plus a 65-line pretty-print test. The GUI renders its own conversation, and the
smoke script and recorded agent workflows already consume JSONL. Keep the event
stream, replay cursor, and the data agents need to respond to permissions and
forms. Drop the terminal transcript and update the CLI help and architecture
docs. Someone manually reading `attach` would get structured events. Keeping
`--json` as an alias is cheap if it saves updating scripts outside the repo.
Source: `crates/oxctl/src/main.rs:320`, `:528` and `:811`,
`scripts/smoke-acp.py:259`, `docs/architecture.md:85`.

### Have New Task read the current branch only

New Task consumes just `head`. The app sets `fetch=false`, yet the daemon still
finds the common Git directory, enumerates and sorts all refs, and returns local
and remote inventories. The optional all-remotes fetch adds a per-repository
coalescer, another task, subprocess timeout behavior, and integration fixtures.
No current production caller uses the inventory or that fetch option. Read the
checked-out branch directly. Keep branch compare and move. The planned PR and
review workflows need a fresh **named** remote ref. Implement that fetch with
its first caller and give shared fetch machinery a concrete use before adding
it back. Source: `ui/src/features/sessions/SessionList.tsx:94`,
`crates/ox/src/lib.rs:175`, `crates/oxd/src/daemon.rs:494`,
`crates/oxd/src/branches.rs:61`, `crates/oxd/tests/worktrees.rs:1333`.
[Git workflow spec](../../.agents/specs/git-workflows.md)

The same test applies to `oxctl new --branch` guessing a remote, configuring
its upstream, and rolling that change back on failure. Agents can use a local
branch or name a new branch and an explicit start ref. Removing the guessing
would drop a separate policy and about 170 lines of focused tests. Keep the
explicit branch operations used by the planned review flow. Source:
`crates/oxd/src/worktree.rs:397` and `:554`,
`crates/oxd/tests/worktrees.rs:772`, `:1133` and `:1603`.

### Make the live Claude smoke check short by default

`AGENTS.md` requires the full live run after every daemon, protocol, CLI and
bridge change. The 949-line script always runs all scenarios. Its parking check
starts a second daemon, asks Claude to run a 25-second shell command, and waits
for parking. Its archive check overlaps deterministic daemon and GUI coverage.
Keep a short native check that starts Claude, streams a real reply, and checks a
continuation. Exercise native permission, wakeup and other Claude-specific
behavior when changing those paths. Use the deterministic checks for Git,
archive controls and ordinary parking. This gives up full live lifecycle
coverage on every edit and avoids requiring that runtime and model work for a
change such as dropping the terminal formatter. Keep the current verification
rule until this choice is made. Source: `AGENTS.md:142`,
`scripts/smoke-acp.py:334`, `:593`, `:678` and `:760`,
`crates/oxd/tests/daemon.rs:2360`, `ui/e2e/specs/archive.e2e.ts:126`.

### Let the normal Delete action handle failed starts

On restart the daemon identifies sessions without an agent ID, acquires
lifecycle guards before it reports ready, and runs a separate worktree cleanup
with the daemon environment. The CLI also automatically forces deletion when
an agent exits before initialization completes. The executable can write to
the checkout before that handshake. Keep a failed session visible as exited
with its error, and use the ordinary Delete action and its unsaved-work check.
The user would sometimes have a failed row and checkout to remove. Retain the
ordinary cleanup of a worktree creation that fails before the agent runs.
Source: `crates/oxd/src/daemon.rs:75` and `:573`, `crates/oxd/src/main.rs:154`,
`crates/oxctl/src/main.rs:470`.

### Support the question forms Claude generates

The bundled bridge asks for titled single or multiple choices and optional free
text. The desktop and daemon also implement numbers, booleans, date/time,
numeric and length bounds, patterns, defaults and required fields. The fake
agent and GUI tests exercise that broader profile. If Claude is the only agent
the user intends to use, keep ACP elicitation with the Claude-shaped controls,
answer validation, cancellation and replay. Reject richer unsupported forms
clearly and document the supported profile. Agents sending richer forms would
lose interactive use of those forms. Keep the saved event format so old
transcripts can still load. Source: `crates/claude-code-acp/src/questions.rs:54`,
`ui/src/features/composer/Elicitation.tsx:10`, `crates/oxd/src/state.rs:886`,
`ui/e2e/specs/session.e2e.ts:66`, `docs/acp-v2.md:63`.

### Decide how much of a standalone bridge to keep

Ox lists its own sessions and resumes saved agent IDs. Direct ACP clients can
also discover and adopt sessions created by terminal Claude. If that standalone
workflow can go, retain ACP `session/list` with pagination over bridge-owned
records, along with resume and replay for existing IDs. That removes catalog
merge and adoption while preserving records already adopted. New terminal
sessions would disappear from Zed and other direct clients. Independently, Ox
sends no MCP override on new or resume. Retiring the advertised `stdio` server
injection would remove validation, a separate runtime store map, process
replacement for changes, and four protocol fixtures. Claude’s configured MCP
servers and their permissions would still work. Direct clients would lose the
ability to supply their own overrides. Source: `crates/oxd/src/session.rs:793`,
`crates/claude-code-acp/src/bridge.rs:85`, `:170`, `:241` and `:647`,
`crates/claude-code-acp/src/store.rs:73`, `crates/claude-code-acp/README.md:16`.

## Leave for evidence from real use

- **Background scheduling.** Keeping watchers and scheduled work alive was an
  explicit requirement. The bridge reconstructs schedule expiry and wakeup
  state from tool calls and transcripts. A smaller policy could leave a session
  awake once it sees a schedule, through explicit archive or stop, while still
  parking ordinary quiet sessions. That gives up per-schedule status and can
  leave Claude processes running after scheduled work ends. Keep the current
  implementation until those costs can be compared in the user's use. Source:
  `crates/claude-code-acp/src/background.rs:12` and `:153`,
  `crates/oxd/src/state.rs:185`.
  [Original requirement](../../.agents/plans/2026-11-12-007-bridge-background-work.md)
- **Recovery after unarchive partially succeeds.** There is a recorded
  fault-injection case where Git restores the checkout and SQLite still says
  archived, leaving retry stuck. A local TODO is enough unless a real failure
  makes it worth handling. A small retry that verifies the existing registered
  checkout belongs to the saved session would be reasonable. File existence
  alone is insufficient. Source: `crates/oxd/src/daemon.rs:640`.
  [Recorded failure](../../.agents/todo.md#unarchive-can-leave-the-session-stuck-after-a-database-write-fails)
- **Development-only inputs and test fixtures.** Mistyping `--socket` is an
  agent setup issue and belongs below normal desktop failures. The daemon’s
  short retry on `accept` failure is cheap to keep. If the descriptor-exhaustion
  harness becomes expensive to maintain, remove that fixture while keeping the
  retry and normal socket lifecycle coverage. Source: `crates/oxd/src/main.rs:139` and
  `:174`, `crates/oxd/tests/daemon.rs:1223`.
- **Desktop layout.** Keep the resizable panes and agent picker. The right
  pane already has a planned role in the file-tree brief and Git workflows.
  [File-tree brief](../briefs/2026-11-13-001-file-tree.md)
  ·
  [Desktop design spec](../../.agents/specs/git-workflows.md)
