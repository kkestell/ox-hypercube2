# Each session owns its state and refuses turns during a branch move

## Goal

Close the first two items under Structure in `.agents/todo.md`, "A session's state has no single owner" and "Callers enforce session preconditions instead of the transitions", and the Daemon bugs they underlie.

When the work is done, everything about a session that changes lives in its `State`, behind one lock. Every change goes through `Session::update`, which applies a transition and then, under that lock, writes the rows the transition produced, queues its events and publishes the result. The daemon no longer writes a session's row or fields directly. Startup restores every saved session the same way. A session knows when its checkout's branch is moving, so `State` refuses a turn or a load during a move, whichever path asks. Workspace removal takes each session's lifecycle guard, like the other lifecycle operations.

### Bugs closed

Each fix removes its `TODO` comment.

- **Restart loses never-started sessions** (`daemon.rs:139`, `state.rs:691`). `Daemon::new` stops deleting sessions and builds every session's state with `State::restored`. A session without an agent session ID is restored as `Phase::Exited(None)`, with its events left unread like any restored session's, so `history` reads its first prompt, the turn's error and `AgentExited` from the store. `State::unstarted` and the deletion branch go. An archived never-started session stays archived across a restart. A never-started worktree session gets its first prompt back. Loading either is still refused with "the session never started, so it has nothing to load".
- **A queued prompt can start while the session's branch is moving** (`state.rs:811`). `State.moving` is set for every session in the checkout when the move is reserved. `start_queued` holds the queue while it is set, through the same `State::refuse_moving` check that `send_now` and `load` use. When the move ends, `Session::end_move` clears the flag through `update`, and `update` then starts the first queued prompt as it does after any change.
- **Removing a workspace can race an archived session's unarchive or delete** (`daemon.rs:630`). `remove_workspace` holds the lifecycle guard of each of the workspace's sessions before it checks them and deletes rows. An unarchive or delete in progress therefore finishes first, and the removal then sees the session unarchived and refuses, or sees it gone. An unarchive or delete waiting behind the removal finds the session closed and gets `not found`.
- **A failed worktree start briefly publishes a dangling worktree ID** (`daemon.rs:859`). `start_in_worktree` removes the worktree from the registry and calls `session.fail_start` in one `Daemon::update`. `fail_start` clears `State.worktree`, records the exit, and writes the worktree row's deletion before it publishes. `listing_changes` takes the registry lock first, so no listing can show the session pointing at a removed worktree.
- **The session lifecycle comment says the session adds its worktree** (Docs and cleanup, `session.rs:87`). The comment is rewritten when `lifecycle` gets its new list of holders.

None of the other listed bugs is closed.

## Related code

- `crates/oxd/src/session.rs:70-118` — `Session`'s fields: `env`, `worktree`, `archived`, `config`, `state`, `commands`, `task`, `viewers`, `parker`, `unseen` (ten mutexes), `focused` (atomic), `lifecycle` (async mutex). `update` (`:812`) and `save` (`:850`) diff `SessionInfo` to write title, agent session ID, unseen and config. `record_start`, `settle_config`, `note_unseen`, `focus`, `view`, `fail_start` (`:289`), `start`/`load` (which hold the task slot against `close`), `hold_lifecycle`, `lifecycle`, and `park_when_idle`.
- `crates/oxd/src/state.rs` — `Phase::after` (`:329`), `State` (`:597`), `State::new`/`restored`/`unstarted` (`:647-703`), `fill`, `queue_changed`/`take_changed_queue`, `start_queued` (`:813`), `load` (`:840`), `send_now` (`:1072`), `record`, and `append` (sets the title). The unit tests construct `State::new("s")` and `restored()`.
- `crates/oxd/src/daemon.rs` — `Registry` (`:28`, `moving_checkouts`), `CheckoutMove`, `refuse_moving`, `Daemon::new` (`:116-186`), `send_unmoved` (`:280`), `handle` (`SessionLoad` at `:343`), `remove_workspace` (`:612`), `add_session` (`:738`), `start_in_worktree` (`:838`), `send_message` (`:867`), `lifecycle` (`:1042`), `delete_session`, `archive_session`, `unarchive_session`, `reserve_checkout` (`:1161`), `add_imported` (`:1260`), `forget`, and `refuse_starting` (`:1516`).
- `crates/oxd/src/store.rs` — `SavedSession`, the per-column setters (`set_title`, `set_agent_session_id`, `set_session_config`, `set_archived`, `set_session_unseen`, `set_session_queue`, `remove_worktree`), `workspace_has_unarchived_sessions`, `remove_workspace`, `remove_session`, and the store tests for each.
- `crates/oxd/src/conn.rs:206-230` — the methods answered in their own task. `WorkspaceRemove` is not one of them yet.
- `crates/oxd/src/branches.rs:76-128` — `move_branch` holds the reservation from before the checkout check until Git finishes.
- `crates/oxd/src/worktree.rs:817` — `Worktree` (`remove`, `restore`, `close`, `recompare`).
- `crates/oxd/tests/daemon.rs` — `workspaces_and_sessions_survive_a_restart` (`:1801`), which asserts that the never-started session is gone, `a_workspace_is_removed_once_its_sessions_are_archived` (`:1736`), `only_an_exited_session_that_started_can_be_loaded` (`:2206`), and `a_first_prompt_ends_when_the_agent_does_not_start` (`:2450`).
- `crates/oxd/tests/worktrees.rs` — `BlockedGit` (`:36`), `move_branch` (`:1580`, passes an empty env), `a_move_waits_for_an_agent_in_the_checkout_to_end_its_turn` (`:1780`), `a_failed_worktree_leaves_a_branch_another_worktree_checked_out` (`:712`), `a_workspace_removed_during_a_creation` (`:747`), `a_delete_during_startup_waits_for_the_worktree` (`:1243`), and the archive and unarchive tests (`:1328-1580`).

## Decisions

- **`State` holds everything about a session that changes.** It gains `worktree: Option<Arc<Worktree>>`, `archived: bool`, `config: Vec<ConfigSetting>`, `unseen: Option<Unseen>`, `focused: usize`, `viewers: usize` and `moving: bool`, besides its phase, title, events and queue. `State::created(&SavedSession, worktree)` builds a just-saved session (`Starting`, with events held). `State::restored(&SavedSession, worktree)` builds one read from the database or imported (`Exited`, with the agent from `agent_session_id` if there is one, and its events unread). Its other fields come from the row. `info_of` reads only `State` and the session's immutable fields.
- **`Session` keeps two sync locks.** Its fields become the immutable ones (`id`, `number`, `workspace_id`, `created_at`, `command`, `cwd`), the handles (`store`, `ox_server`, `changed`), `state: Mutex<State>`, `agent: Mutex<AgentTask>`, `lifecycle`, `history`, `last_seq`, `info_version`, `parkable`, and `parker: OnceLock<AbortHandle>`. `AgentTask { env, commands, task }` holds the running agent's environment, command channel and task. It is locked only after `state`, or on its own. Launching (`start`, `load`) happens under the state lock, so `close`, which records `Closed` under the same lock, cannot miss a new task. The "holding the task slot" workaround goes.
- **Lock order.** Lock order is session lifecycle guard, then registry, then session state, then the agent task. The doc comment on `Session` states it once; the per-field "changed under the state lock" and "locked after" comments go. Nothing takes the registry while holding a session's state.
- **Transitions compute rows as they compute events.** `store::RowChange` names one change to a session's saved data: `Title(Option<String>)`, `AgentSessionId(String)`, `Config(Vec<ConfigSetting>)`, `Unseen(Option<Unseen>)`, `Archived(bool)`, `Queue(Queue)`, and `WorktreeForgotten(String)`. Each `State` transition that changes saved data pushes one to `State.unsaved`, just as `append` pushes events to `unlogged`:
  - `append` pushes the title from a `session_info_update`, and `set_title` pushes it directly.
  - `record(Change::Started)` pushes a changed agent session ID. It also adopts and pushes the reported config of a session without one, as `record_start` does now.
  - `settle_config`, now a `State` method, pushes the config.
  - `note_unseen` and `focus` push the unseen outcome.
  - `set_archived` pushes the archived flag.
  - `fail_start` pushes `WorktreeForgotten`.
  - Every place that sets `queue_changed` today pushes the queue.

  `take_unsaved` keeps only the last change of each kind. `Store::save_session(id, &[RowChange])` writes them in one transaction. It replaces the seven setters, `Session::save`, `queue_changed` and `take_changed_queue`.
- **`update` persists before it publishes.** Under the state lock, `update` runs the following steps in order:
  1. Apply the change.
  2. Run `start_queued`.
  3. Take the new events and give them to `note_unseen`.
  4. Take the rows and write them with `save_session`.
  5. Queue the events with `store.log`.
  6. Ask the worktree to compare again after a turn ends.
  7. Publish `last_seq`, parkability and, when `SessionInfo` changed, `info_version` and `changed`.

  A failed row write is logged and the session carries on, as now. `Session::commit` is `update` that returns the row write's error instead. Archive and unarchive use it, so they still answer a storage error.
- **The registry owns which sessions exist; a session owns its own row.** `add_session`, `add_imported`, `forget` and `remove_workspace` still insert and delete session rows, because they add sessions to the registry or remove them from it. Every other write to a session's data goes through `Session::update`. The daemon's `store.set_archived` and `store.remove_worktree` calls go.
- **The lifecycle guard checks for closing.** `Session::lifecycle()` returns `Result<MutexGuard, ErrorBody>` and gives `not found` once the phase is `Closed`. Deletion and workspace removal close the session before they release its guard, so a waiter sees the closed phase. `Daemon::lifecycle` goes.
- **Lifecycle operations take their own guard.**
  - `Session::load(env)` takes the guard, reads history, checks the directory and applies `State::load`. `SessionLoad` calls it.
  - `Session::load_if_exited(env)` does the same, but returns `Ok` when the session is no longer `exited` once the guard is held. `send_message` calls it.
  - `Session::archive(git, discard)` and `Session::unarchive(git)` take over `archive_session` and `unarchive_session`. They make the same checks, the starting check before and after the guard and the archived check after it, and finish with `commit(|state| state.set_archived(..))`.
  - `delete_session` takes `session.lifecycle()` itself.

  `refuse_starting` goes.
- **Moving is the session's to refuse.** `State::refuse_moving()` returns the existing error, `INVALID_STATE` "a branch move is in progress in this checkout", while `moving` is set. `send_now` (prompts, peer messages, `queue_send`) and `load` call it first, and `start_queued` returns `None` when it fails. `State::begin_move()` sets `moving` on an `idle` or `exited` session and otherwise returns its state. `State::end_move()` clears it.

  `reserve_checkout` does the following under the registry lock:
  1. Refuse a checkout that is already moving.
  2. Call `Session::begin_move` on every session whose `cwd` is in the checkout. If any refuses, end the moves already begun and return the current "session X is Y in Z" error.
  3. Record the checkout in `moving_checkouts`.

  `CheckoutMove` keeps the sessions it marked, and its `drop` removes the checkout and calls `end_move` on each. `send_unmoved` and the `refuse_moving` calls in `SessionLoad` and `send_message` go. `moving_checkouts` stays only so that `add_session` can refuse to create a session in a moving checkout, which has no session yet to know about the move.
- **Workspace removal goes through the sessions' lifecycle.** `remove_workspace` becomes async and is answered in its own task in `conn.rs`. It runs these steps:
  1. Under the registry lock, find the workspace and refuse at once if any of its sessions is not archived, reading `State.archived`. A session still starting in its worktree is not archived, so it is refused at once as now.
  2. Take each session's lifecycle guard, in session number order.
  3. Under the registry lock again, check that the workspace still exists and that the sessions guarded are exactly the workspace's sessions. If a session joined meanwhile, such as an import, release the guards and start over.
  4. Check that every guarded session is still archived.
  5. Delete the rows with `store.remove_workspace` and remove the sessions and their worktrees from the registry.
  6. Close each session and worktree before releasing the guards.

  `Store::workspace_has_unarchived_sessions` goes. The registry and the store agree here, because each session is saved and registered under the same registry lock.
- **A failed worktree start is one registry update.** `start_in_worktree`'s failure path runs `self.update(|registry| { registry.worktrees.remove(..); session.fail_start(message) })`, then `live_worktree.close()`. `fail_start` (`State::fail_start`) clears `worktree`, records `Change::Exited(Some(message))` and pushes `WorktreeForgotten`, which deletes the worktree row. The sessions' `worktree_id` is `ON DELETE SET NULL`.
- **Small field moves.** `focus` and `SessionFocus::drop` change `State.focused` through `update`, and `view` and `Viewer::drop` change `State.viewers`. Parkability is `State::parkable()` (quiet and unviewed). `Session::worktree()`, `archived()`, `env()` and `config()` read the owning lock. `refusals` moves to `state.rs` beside `settle_config`.
- **No protocol change.** Every refusal keeps its code and message, and `SessionInfo` is unchanged.

### Behavior that changes

- A never-started session survives a restart, `exited` with its history, and can be archived or deleted but not loaded. This is the bug fix; restart no longer cleans these sessions up.
- `session.queue_add` during a branch move in the session's checkout is accepted. The prompt waits and starts once the move ends. Before, the request was refused.
- `workspace.remove` waits for an unarchive or delete of one of the workspace's archived sessions that is already in progress, then checks again. It is answered in its own task, so the connection serves later requests meanwhile.
- When saving fails (unlikely): an archive or unarchive whose row write fails still answers the storage error, but the in-memory archived flag has already changed until the next restart. Before, the flag stayed as it was, while the worktree had already been removed or restored. A failed worktree row deletion after a failed worktree start is logged, and is no longer appended to the session's exit message.

### Interactions with the event log (Structure item 3, out of scope)

- Events are still queued for the batch writer after `update` assigns their seq. "Persists before it publishes" covers session rows, not events.
- A never-started session interrupted by a stop relies on `fill` to end its first prompt's turn. Its error therefore appears only once its history is read, like the crash-error bug.
- `history`'s `OnceCell` and `State.events: Option` remain the restored mode that item 3 removes. `State::restored` is the single entry point that item will change.

## Check your assumptions

1. **No stored lifecycle column.**
   - **Decided:** Startup reads `agent_session_id` and the `archived` table directly and restores every session with `State::restored`. Once the deletion branch goes, the only status left to infer is whether the agent ever opened a session. `agent_session_id` records exactly that, and resume needs it anyway.
   - **Consequences:** There is no schema migration or new fixture. The stored status is spread over `agent_session_id` and the `archived` table, as it is now.
   - **Alternative:** Schema step 3 adds `sessions.lifecycle TEXT NOT NULL` (`new`, `started`), backfilled from `agent_session_id`, and folds the `archived` table into an `archived` column, with a `schema-3.sql` fixture. Startup builds the phase from `lifecycle`.
   - **Trade-offs:** The column makes the status readable in SQL and removes a side table. It also keeps a second copy of the "agent opened a session" fact that can disagree with `agent_session_id`, and it costs a migration and a fixture.

2. **Every never-started session is kept across a restart.**
   - **Decided:** Keep them all, archived or not, in the workspace checkout or a worktree. The bug report covers both the archived case and the worktree case, and keeping them makes restart cleanup unnecessary.
   - **Consequences:** Sessions whose agent failed to start stay in the sidebar, `exited` with their error, until the user deletes or archives them. `spec/sessions.md:93` changes from "cleaned up" to "kept".
   - **Alternative:** Keep only archived and worktree never-started sessions, and still delete unarchived ones in the workspace checkout at startup.
   - **Trade-offs:** Deleting keeps the list tidy after a bad command, but a user loses a failed session's first message without asking, and startup keeps a cleanup rule.

3. **A prompt queued during a branch move is accepted and waits.**
   - **Decided:** `queue_add` no longer checks for a move, because queuing no longer starts a turn during one.
   - **Consequences:** Queuing during a move succeeds instead of returning "a branch move is in progress in this checkout". The desktop queues only while a turn runs, and a move refuses a running checkout, so this is reached only when an agent starts a turn of its own during the move.
   - **Alternative:** `QueueAdd` also calls `refuse_moving`.
   - **Trade-offs:** Refusing keeps the old answer exactly but refuses a request the session can now honor. Accepting matches what a queue means.

4. **The per-session lifecycle mutex stays.**
   - **Decided:** Keep the async `lifecycle` mutex as the one thing that orders a session's multi-step operations: worktree startup, load, park, archive, unarchive, delete, and now workspace removal. These operations wait for one another today; `a_delete_during_startup_waits_for_the_worktree` and the spec's "Archive and delete wait for worktree startup or a resume already in progress" rely on that.
   - **Consequences:** Each session keeps two sync locks and this async lock. The ten field mutexes and the focus atomic go.
   - **Alternative:** Replace the mutex with a lifecycle status in `State` (for example `Deleting` or `Archiving`) that refuses a conflicting operation.
   - **Trade-offs:** A status removes the async lock but turns today's waits into refusals, so the app would have to retry deletes during startup. Waiting through a status would rebuild the mutex by hand.

5. **Workspace removal waits instead of refusing a busy session.**
   - **Decided:** Removal waits for an unarchive or delete already in progress on one of its archived sessions, then checks again. This matches how archive and delete wait for a startup in progress.
   - **Consequences:** A removal that overlaps an unarchive answers once the Git work of the unarchive finishes, and then refuses because the session is no longer archived.
   - **Alternative:** Try each guard without waiting, and refuse the removal with `INVALID_STATE` while any session of the workspace has a lifecycle operation in progress.
   - **Trade-offs:** Refusing answers at once and keeps `remove_workspace` synchronous, but it adds a refusal the app does not expect. Waiting gives the same answer the user would get by trying again a moment later.

## Test plan

- `crates/oxd/src/state.rs`:
  - A session restored without an agent session ID is `exited`, refuses `load` with "never started", and `fill` gives it its stored first prompt and error.
  - `begin_move` refuses a `running`, `starting` or awaiting session and returns its state. It accepts `idle` and `exited`.
  - While moving, `request(Prompt)`, `request(Message)`, `request(QueueSend)` and `load` return the move error. `request(QueueAdd)` is accepted, and `start_queued` returns `None` with the prompt still queued. After `end_move`, `start_queued` starts it.
  - Rows: `set_title` and a `session_info_update` push `Title`, starting a session without a config pushes `Config` with the reported values, `set_archived` pushes `Archived`, `fail_start` pushes `WorktreeForgotten`, a queue change pushes `Queue`, and `take_unsaved` keeps one change of each kind, the last.
  - The existing tests are updated to build `State` through `created` and `restored`, and the config adoption and `settle_config` cases move here from the integration-only coverage they have now.
- `crates/oxd/src/store.rs`: `save_session` writes each `RowChange` and the session reads back with it, replacing the tests of the removed setters. `WorktreeForgotten` leaves the session with no worktree. A failed row leaves the transaction's other rows unwritten. The `whether_a_workspace_has_unarchived_sessions` test goes.
- `crates/oxd/tests/daemon.rs`:
  - `workspaces_and_sessions_survive_a_restart` now expects the session that never started after the restart, `exited`, with no agent session ID, and its load refused.
  - A new session of `--no-sessions` with a first prompt is archived, then the daemon restarts. The session is listed archived, and subscribing from 0 gives `PromptStarted`, the erroring `TurnEnded` and `AgentExited`.
- `crates/oxd/tests/worktrees.rs`:
  - A worktree session of `--no-sessions` with a first prompt survives a restart with its first prompt in its history.
  - A queued prompt waits for a branch move. `move_branch` gains an env argument, and the test blocks the move on `" update-ref "` with `BlockedGit` after the reservation:
    1. While the move is blocked, `prompt` is refused with the move error, and `queue_add` succeeds.
    2. A listing read afterwards on the same connection still shows the prompt queued and the session `idle`.
    3. After `release`, the move succeeds and the subscription receives the queued prompt's `PromptStarted`.
  - Removing a workspace waits for an unarchive:
    1. Archive the workspace's only session.
    2. Unarchive it with `BlockedGit(" worktree add ")`, and send `remove_workspace` while the unarchive is blocked.
    3. A listing answered after the removal was sent shows that it arrived. Then release the unarchive.
    4. The unarchive succeeds, the removal answers `INVALID_STATE`, and the workspace and session remain.
  - `a_failed_worktree_leaves_a_branch_another_worktree_checked_out` follows a watch until the session exits and, after each listing message, asserts that every session's `worktree_id` names a listed worktree. The test cannot force the old interleaving; it holds the invariant on every listing.
  - `a_workspace_removed_during_a_creation` keeps its immediate refusal while the worktree is being added.
- Existing coverage stays: `a_move_waits_for_an_agent_in_the_checkout_to_end_its_turn`, `a_delete_during_startup_waits_for_the_worktree`, `delete_while_starting`, `only_an_exited_session_that_started_can_be_loaded`, `a_workspace_is_removed_once_its_sessions_are_archived`, and the archive and unarchive tests.

## Implementation plan

1. `store.rs`: add `RowChange` and `save_session`. Remove `set_title`, `set_agent_session_id`, `set_session_config`, `set_archived`, `set_session_unseen`, `set_session_queue`, `remove_worktree` and `workspace_has_unarchived_sessions`. Update the store tests.
2. `state.rs`: add the moved fields, `created`/`restored(&SavedSession, worktree)`, `unsaved`/`take_unsaved`, `refuse_moving`, `begin_move`, `end_move`, `set_archived`, `fail_start`, `settle_config`, `note_unseen`, focus and viewer counts and `parkable`. Add config adoption to the `Started` path. Delete `unstarted`, `queue_changed` and `take_changed_queue`, and the `TODO` at `:811`. Update the unit tests.
3. `session.rs`: reduce `Session` to the fields above with `AgentTask` and `OnceLock<AbortHandle>`. Rewrite `update` in the stated order, and add `commit`. Rewrite the lifecycle guard to check `Closed`. Add `load`/`load_if_exited`, `archive`/`unarchive` and `begin_move`/`end_move`. Delete `save`, `record_start` and the pub `set_archived`. Rewrite the doc comments, including the `lifecycle` field's list of holders and the lock order on `Session`.
4. `daemon.rs`: restore every saved session through `State::restored` and remove the deletion and its `TODO`. Rewrite `Daemon::new`'s doc. Delete `send_unmoved`, `Daemon::lifecycle` and `refuse_starting`. Route `SessionLoad`, `send_message`, archive and unarchive through the session methods. Rework `reserve_checkout`/`CheckoutMove`, the `start_in_worktree` failure path, and `remove_workspace` (async; remove the `TODO` at `:630`).
5. `conn.rs`: answer `WorkspaceRemove` in its own task and add it to the comment's list.
6. Add and update the tests above.
7. Update the documentation below.

## Documentation updates

- `docs/dev/architecture.md`:
  - "Persistence and recovery": a session's metadata is saved in the step that changes it, before clients are told.
  - "Session lifecycle": the daemon starts the queue's first prompt when the agent is idle, with no pending request, no steered prompt pending, and no branch move under way in its checkout.
  - "Loading and stopping agents": after a restart every saved session returns `exited`, including one whose agent never started, which keeps its history and cannot load. The lifecycle lock coordinates worktree startup, load, parking, archiving, unarchiving, deletion and workspace removal.
  - "Repository ownership": "A session whose agent never started keeps its worktree…" holds across restarts. While a branch moves, the sessions in its checkout refuse prompts and loads, and their queues wait.
- `docs/worktrees.md:49`: after "refuses requests to start, load, or prompt a session in that checkout", add that queued prompts in that checkout wait until the move finishes.
- `spec/sessions.md:93`: a session that never successfully started returns after a service restart as Exited with its first message and error. It cannot be resumed, and it can be archived or deleted.
- `spec/git.md:37`: while a move runs, sessions in that checkout refuse new messages and resumes, and queued messages wait until it finishes.
- `spec/workspaces.md:41`: removing a workspace waits for an unarchive or delete of one of its sessions already in progress, then checks again.
- `.agents/todo.md`: check off the two Structure items, the four Daemon bugs listed under Bugs closed, and the lifecycle-comment item under Docs and cleanup.
