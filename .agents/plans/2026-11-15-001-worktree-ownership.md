# Separate worktree ownership from agent execution

## Goal

Deliver task 1 of the brief's delivery sequence. A worktree becomes its own daemon record, live object, and listing entry, and a session refers to it by ID. The worktree owns its branch comparison and its removal. `Session` keeps agent execution only. Delete and archive share one unsaved-work check. New Task works through the same screens as before.

**Brief:** `.agents/briefs/2026-11-14-001-user-defined-workflows.md`

## Related code

- `crates/oxd/src/session.rs`: `Session` holds `worktree: Mutex<Option<SavedWorktree>>`, the comparison fields (`comparison`, `comparing`, `recompare`, `comparer`), `compare_when_asked`, `compare`, `recompare`, `set_compare`, `spawn`, and `start_in_worktree` (add, roll back, forget, launch). `update` asks for a comparison on `TurnEnded`. `info_of` projects the worktree and comparison into `SessionInfo`.
- `crates/oxd/src/daemon.rs`:
  - `new_session` and `add_session` plan the worktree, create the branch, and save it.
  - `delete_session` and `archive_session` each check for unsaved work, stop the agent, and remove the worktree.
  - `unarchive_session` restores the worktree.
  - `Daemon::new` and `clean_up` handle unstarted sessions that have worktrees.
  - `recompare` and `set_compare` act through sessions.
  - `list`, `listing_changes`, and `Sent` cover workspaces and sessions.
- `crates/oxd/src/conn.rs`: calls `Daemon::handle`.
- `crates/oxd/src/worktree.rs`: git operations on a `SavedWorktree`: `plan` (which names the directory and branch from the session ID), `create_branch`, `add`, `restore`, `roll_back`, `unsaved`, `unsaved_work`, `remove_worktree`, and `apply_branch_rule`.
- `crates/oxd/src/branches.rs`: `compare` computes a `SavedWorktree`'s `Comparison`.
- `crates/oxd/src/store.rs`: the `worktrees` table, keyed by `session_id`, and the `compare_refs` table. `SavedSession.worktree`, `SavedWorktree`, `add_session`, `set_compare`, `remove_worktree`, and `remove_session`.
- `crates/ox-protocol/src/messages.rs`: `WorktreeInfo`, `SessionInfo.worktree` and `.comparison`, `Listing`, `ListingChange`, `Method::SessionSetCompare` and `SessionRefresh`, the `Checkout::Worktree` doc, and the serialization tests for these methods.
- `crates/ox-protocol/src/client.rs`: `set_compare` and `refresh`.
- `crates/oxctl/src/main.rs`: `Command::Compare` reads `info.worktree` and `info.comparison`.
- `ui/src/lib/protocol.ts`: mirrors `WorktreeInfo`, `SessionInfo`, and `ListingChange`.
- `ui/src/features/sessions/useWorkspaces.ts`: `apply` folds listing changes into workspaces and sessions.
- UI files that read `session.worktree` and `session.comparison`:
  - `ui/src/features/sessions/localMerge.ts`, in `localMerge`
  - `ui/src/features/sessions/MergeButton.tsx`, for the branch to merge
  - `ui/src/features/sessions/SessionList.tsx`, in `SessionRow`'s status icon
  - `ui/src/App.tsx`, for the selected session's Merge button
- `crates/oxd/tests/worktrees.rs`: integration coverage for creation, rollback, startup cleanup, delete, archive, comparison, and its publication through `watch`. About 65 assertions read `session.worktree` or `comparison`, through `session_worktree`, `until_compared`, and `info`. Lines 714–733 and 1045 build names from `session.id[..8]`.
- `ui/e2e/specs/worktree.e2e.ts`, `merge.e2e.ts`, `archive.e2e.ts`: New Task creation, comparison and merge, and archive through the app. `ui/e2e/helpers.ts` calls `oxctl compare <session>`.

## Decisions

- **Protocol.**
  - `WorktreeInfo` gains `id` and `comparison: Option<Comparison>`. The doc on when the daemon recomputes the comparison moves from `SessionInfo` to `WorktreeInfo`.
  - `SessionInfo` replaces `worktree` and `comparison` with `worktree_id: Option<String>`.
  - `Listing` gains `worktrees`.
  - `ListingChange` gains `Worktree { worktree: WorktreeInfo }` and `WorktreeRemoved { worktree_id }`.
  - `listing_changes` emits removals first, then workspaces, then worktrees, then sessions, so a client has each worktree before any session that refers to it.
  - `session.set_compare` and `session.refresh` become `worktree.set_compare { worktree_id, compare }` and `worktree.refresh { worktree_id }`. Both answer `WorktreeInfo`. `Client::set_compare` and `Client::refresh` take a worktree ID and return `WorktreeInfo`.
  - `session.delete` and `session.archive` keep their shape. Deleting or archiving a session removes the worktree it owns.
  - No `worktree.list` method is added. Clients get worktrees from `watch`.
- **`oxctl compare <session>`.** The command keeps its arguments and output, so `ui/e2e/helpers.ts` is unchanged. It finds the session's `worktree_id` through `client.list()`, then prints the result of `client.refresh(worktree_id)`.
- **`Worktree` live object.** Add `pub struct Worktree` to `worktree.rs`, beside the git operations it calls. It holds:
  - `id`
  - `saved: Mutex<SavedWorktree>`
  - the comparison state that moves from `Session`: `comparison`, `comparing`, `recompare`, and the `comparer` task
  - `version: AtomicU64`
  - the daemon's `changed` sender

  `Worktree::new(saved, changed) -> Arc<Self>` spawns the comparer. The methods are:
  - `info()`, which returns `WorktreeInfo` with the comparison, unset when `compare` is unset
  - `versioned_info()`
  - `saved()`
  - `compare()`
  - `recompare()`
  - `set_compare(compare)`
  - `remove(git, discard, stop)`
  - `restore(git)`
  - `apply_branch_rule(git)`
  - `close()`, which aborts the comparer

  Each change to the comparison or the comparison ref bumps `version` and sends `changed`. `branches::compare` moves into `worktree.rs` as the comparer's helper.
- **Daemon registry.** `Registry` gains `worktrees: HashMap<String, Arc<Worktree>>`, and `Sent` gains `worktrees: HashMap<String, u64>`. `listing_changes` lists worktrees as it lists sessions: it reads only those whose version changed, and reports removed ones. `Daemon::recompare(common_dir)` notifies the registry's worktrees in that repository.
- **Session reference.** `Session` holds `worktree: Mutex<Option<Arc<Worktree>>>` and loses every comparison field and method. `info_of` reports `worktree_id`. On `TurnEnded`, `update` calls the worktree's `recompare()`. `info_version` stays one counter, because a worktree's changes no longer alter `SessionInfo`.
- **Starting a worktree session.**
  - `Session::spawn` splits into `Session::create`, which builds a `starting` session with no agent task, and `Session::start(env)`, which launches the agent if the session is still `starting`. `start` keeps the existing task-slot guard against a concurrent `close`.
  - `Session::fail_start(message)` clears the worktree and records `Exited(Some(message))`.
  - `add_session` registers the worktree and the session together. It calls `start` immediately for a session in the workspace checkout. For a worktree session, it takes `hold_lifecycle()` and spawns `Daemon::start_in_worktree`.
  - `start_in_worktree` calls `worktree::add`. On failure it calls, in order: `worktree::roll_back`, `store.remove_worktree(worktree id)`, removal of the worktree from the registry, `worktree.close()`, and `session.fail_start`. On success it calls `session.start(env)`.
  - `handle` and `new_session` take `self: &Arc<Self>` so that the task can own the daemon. `conn.rs` passes its `Arc<Daemon>`.
  - The session's lifecycle lock still guards its worktree's addition and removal, because each worktree has exactly one session.
- **Unsaved-work check.** `Worktree::remove(git, discard, stop)` does the following:
  1. Unless `discard` is set, it refuses with `unsaved_work` before awaiting `stop`.
  2. It awaits `stop`.
  3. It calls `remove_worktree`, which checks again for work that appeared after the agent stopped.

  `delete_session` passes `session.stop(Change::Exited(None))` and then calls `apply_branch_rule`. `archive_session` passes `session.stop(Change::Parked)`. Startup cleanup passes a no-op future with `Git::own()`. `remove_unstarted` is deleted.
- **Schema.** Replace the schema as follows:
  - `worktrees (id TEXT PRIMARY KEY, path, branch, base, created_branch, common_dir, compare TEXT)`
  - `sessions.worktree_id TEXT REFERENCES worktrees(id) ON DELETE SET NULL`
  - Drop `compare_refs`. The `worktrees` table is created before `sessions`.

  `SavedWorktree` gains `id`. `SavedSession.worktree` becomes `worktree_id: Option<String>`. The store changes are:
  - `Store::worktrees()` lists the saved worktrees.
  - `add_session` inserts the worktree row and then the session row in one transaction.
  - `set_compare` and `remove_worktree` take a worktree ID.
  - `remove_session` deletes the session and the worktree it refers to in one transaction.
- **Worktree ID and naming.** `new_session` generates a UUID for the worktree and passes it to `worktree::plan`, which replaces its `session_id` parameter with `worktree_id`. `plan` uses that ID for the directory `<checkout>-<worktree id8>` and the branch `ox/<worktree id8>`. `WorktreePlan::saved()` records the ID.
- **Startup cleanup by worktree.**
  - `Daemon::new` loads the worktrees into the registry and gives each restored session its `Arc<Worktree>`.
  - A worktree in which no session has started goes to `unstarted`, together with its session and that session's lifecycle guard.
  - `clean_up` calls `worktree.remove(&Git::own(), false, async {})`, then `apply_branch_rule`, `recompare`, and `forget`.
  - A session that never started and has no worktree is deleted, as before.
- **`forget`.** `Daemon::forget(id)` removes the session row, which also removes its worktree row. It then removes the session and its worktree from the registry and closes the worktree.
- **`set_compare` and refresh.** `worktree.set_compare` looks up the worktree in the registry and validates the ref as it does now. It saves the ref with `store.set_compare(worktree id, …)` and calls `worktree.set_compare`. `worktree.refresh` calls `worktree.compare()`.
- **UI.**
  - `useWorkspaces` keeps `worktrees` in its listing and applies `worktree` and `worktree_removed` changes. Unchanged entries keep their identity. It returns the worktrees as a `Map` keyed by ID, rebuilt only when a worktree change arrives.
  - `localMerge(session, worktree)` reads `worktree.compare` and `worktree.comparison`.
  - `App` looks up the selected session's worktree and passes it to `localMerge` and `MergeButton`. `MergeButton` takes a `worktree` prop for `sourceBranch` and the delete dialog's branch name.
  - `SessionList` passes each `SessionRow` its worktree. Rows then re-render only when their own worktree changes.

## Check your assumptions

1. **How a reset database is handled.** Kept.
   - **Decided:** The plan adds no migration and no schema-version check. The schema has never had migrations (`Store::open`), and the brief calls existing records test data.
   - **Consequences:** `oxd` refuses to start against an existing database. It reports `reading <socket>.db: no such column …` until that file is deleted. That file is `~/.local/share/ox/oxd.sock.db` by default, or `target/dev/oxd.sock.db` for `make dev`. Worktrees under the old `worktrees/` directory stay on disk and registered in their repositories until removed by hand or with `git worktree prune`.
   - **Alternative:** Store a schema version in `PRAGMA user_version` and recreate the database when it differs.
   - **Trade-offs:** Deleting by hand adds no code but requires a manual step on each machine. An automatic reset handles every machine and later schema changes, but adds a mechanism that silently discards data.
2. **How clients see worktrees.** Changed from keeping the protocol to listing worktrees as their own entries.
   - **Decided:** Worktrees are listing entries with IDs and their own comparison. Sessions carry `worktree_id`. Comparison methods address worktrees.
   - **Consequences:** The protocol, `oxctl compare`'s implementation, and five UI files change. The worktree integration tests read worktrees from listing entries. The New Task screens look and behave as before.
   - **Alternative:** Keep `SessionInfo.worktree` and `comparison`, and confine the change to `oxd`.
   - **Trade-offs:** Separate entries publish one comparison per worktree, which the run's child sessions can share, and keep a worktree visible without its sessions. They cost protocol, UI, and test changes now. Keeping the protocol changes only `oxd` now, but every session that shares a worktree would repeat its comparison.
3. **Where worktree and branch names come from.** Changed from the session ID to the worktree ID.
   - **Decided:** The directory is `<checkout>-<worktree id8>` and the branch is `ox/<worktree id8>`.
   - **Consequences:** A New Task branch's suffix matches its worktree's ID, not its session's. The documentation and comments that name the session ID change.
   - **Alternative:** Name them from the session ID.
   - **Trade-offs:** The worktree ID gives one naming rule whatever owns the worktree, and matches an ID clients see. The session ID keeps the current names, but a run's worktree would need another source.
4. **How New Task is verified through the user flow.** Kept.
   - **Decided:** Run `make e2e`. Its `worktree`, `merge`, and `archive` specs drive New Task creation, comparison and merge, and archive and delete in the built app, alongside the `oxd` worktree integration suite. There is no manual run.
   - **Consequences:** Verification is reproducible. It uses the fake agent, so it does not exercise a real model in a worktree. The smoke runs cover real models outside worktrees.
   - **Alternative:** Also start a New Task with a real agent under `make dev`, then commit, compare, merge, and delete it by hand.
   - **Trade-offs:** The suite alone is repeatable and needs no person. A manual run also catches what the fake agent cannot show, at the cost of manual time.

## Test plan

- `crates/oxd/tests/worktrees.rs` keeps every test's behavior. Its helpers change as follows:
  - `session_worktree` becomes an async `worktree_of(client, &session)`, which reads the session's worktree entry from a new watch's first changes.
  - `until_compared` waits for `ListingChange::Worktree` with the session's `worktree_id`.
  - `set_compare` and `refresh` calls pass the worktree ID.
  - The naming assertions at lines 714–733 and 1045 use the worktree ID.
  - Restart and cleanup tests that compare `kept.worktree` compare the listed worktree entry instead.
- Add to `worktrees.rs`:
  - A new worktree session lists its worktree before the session, and the session's `worktree_id` names it.
  - Deleting the session reports `WorktreeRemoved`.
  - A worktree that cannot be added is removed from the listing, and its session's `worktree_id` is unset.
- Rewrite the `store.rs` unit tests for the new schema:
  - A worktree session round-trips with its worktree, including its comparison ref, through `worktrees()` and `worktree_id`.
  - `set_compare` replaces and clears the ref by worktree ID.
  - `remove_worktree` leaves the session with no `worktree_id`.
  - `remove_session` removes the session's worktree row and events.
  - A session whose worktree row fails to insert is not saved.
- In `messages.rs`, replace the serialization tests for `session.set_compare` and `session.refresh` with tests for `worktree.set_compare` and `worktree.refresh`.
- `make e2e`, covering the `worktree`, `merge`, and `archive` specs and the stress spec's listing budgets.

## Implementation plan

1. `ox-protocol`: in `messages.rs`, make the `WorktreeInfo`, `SessionInfo`, `Listing`, `ListingChange`, and method changes, update the `Checkout::Worktree` doc to name the worktree ID, and update the serialization tests. In `client.rs`, change `set_compare` and `refresh`.
2. `store.rs`:
   - Replace the `worktrees` table, add `sessions.worktree_id`, and drop `compare_refs`.
   - Add `id` to `SavedWorktree`, and replace `SavedSession.worktree` with `worktree_id`.
   - Add `worktrees()`.
   - Change `add_session`, `set_compare`, `remove_worktree`, and `remove_session` as Decisions describes.
   - Rewrite the unit tests.
3. `worktree.rs`: `plan` takes and records the worktree ID and names the directory and branch from it. Move `compare` in from `branches.rs`. Add `Worktree` and its methods, including `remove(git, discard, stop)`. Update the module comment to cover the live worktree.
4. `branches.rs`: remove `compare` and the `SavedWorktree` import, and update the module comment.
5. `session.rs`:
   - Replace the worktree field with `Option<Arc<Worktree>>`, and report `worktree_id` from `info_of`.
   - Remove the comparison fields and methods, `start_in_worktree`, and the worktree branch of `spawn`.
   - Add `create`, `start`, and `fail_start`.
   - Make `restore` take its `Arc<Worktree>`.
   - Recompare through the worktree on `TurnEnded`.
6. `daemon.rs` and `conn.rs`:
   - Registry and `Sent`: add `worktrees`.
   - `list` and `listing_changes`: include worktrees in the order Decisions gives.
   - `new` and `clean_up`: build the worktree registry and clean up by worktree.
   - `handle` and `new_session`: take `self: &Arc<Self>`.
   - `add_session`: generate the worktree ID, create and register the `Worktree`, and call `start` or spawn `start_in_worktree`.
   - Add `start_in_worktree`.
   - `delete_session` and `archive_session`: call `Worktree::remove`.
   - `unarchive_session`: call `Worktree::restore`.
   - Add the `worktree.set_compare` and `worktree.refresh` handlers.
   - `recompare`: act through the registry's worktrees.
   - `forget`: remove and close the worktree.
7. `oxctl`: `Command::Compare` resolves the session's `worktree_id`, then refreshes and prints the worktree.
8. UI: update `protocol.ts`, `useWorkspaces.ts`, `localMerge.ts`, `MergeButton.tsx`, `SessionList.tsx`, and `App.tsx` as Decisions describes. Update the `ox/<id8>` comments in `protocol.ts` and `SessionList.tsx` to name the worktree ID.
9. `crates/oxd/tests/worktrees.rs`: update the helpers and assertions, and add the listing tests.
10. Delete the local development database, `target/dev/oxd.sock.db`. Run `make check`, `make e2e`, `make smoke-claude`, and `make smoke-pi`.

## Documentation updates

- `docs/worktrees.md`: New Task's branch, and each worktree's directory, take the first 8 digits of the worktree's ID, not the session's.
- `docs/dev/architecture.md`:
  - Under "Persistence and recovery", the database holds worktrees, with their comparison refs, in place of "worktree ownership, comparison refs".
  - Under "Repository ownership", the daemon saves each worktree as its own record and lists it to clients as its own entry, which a session refers to. A worktree computes one branch comparison. The session's agent launches once the worktree is added. Restart cleanup removes worktrees in which no agent ever started, together with their sessions, and keeps worktrees whose removal would lose work.
- `docs/dev/glossary.md`: **Comparison ref** is compared with a worktree's branch, not a worktree session's.
