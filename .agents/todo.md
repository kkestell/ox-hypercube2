# To do

## Local merge workflow

Spec: [git-workflows](specs/git-workflows.md). Two `/kplan` → `/kwork` →
`/kreview` cycles. Sizes are rough estimates of lines changed, tests included.

- [x] **Daemon and CLI** (~1,800). Tested through `oxctl`.
  - Move a branch: fast-forward or forced, the expected-commit check,
    updating a checkout that has the branch, refusing unsaved work or a
    running agent.
  - Compare: a comparison ref on worktree sessions, published with the
    session, recomputed on turn end, on moves, deletes and fetches in the
    repository, and on a refresh request.
- [x] **App** (~1,000, about 260 of them deletions).
  - The New Task menu: New Task starts a local merge session; Session in
    Checkout stays.
  - The right-aligned status icon on session rows.
  - The Merge into main button in the conversation header.
  - Removes the existing-branch and new-branch dialogs and `useBranches.ts`.

## Git correctness

Fix branch moves and source validation for local merge before adding the PR
and review workflows. Review rounds will reuse `branch.move` to update the
review checkout. Keep Git and session state in `oxd` and workflow choices in
the clients.

### Branch moves can overwrite local files or run alongside an agent

- [x] Preserve ignored, untracked files that a branch move would overwrite.
- [x] Keep a checkout from starting agent work during a branch move.
  [Desktop merge regressions](../ui/e2e/specs/merge.e2e.ts)

### Local merge can land an obsolete task commit

- [x] Validate the source branch and its expected tip as part of the merge
  request.
  [Desktop merge regressions](../ui/e2e/specs/merge.e2e.ts)

## Desktop workflow bugs

- [x] Keep unsent prompt text and attachments when switching sessions. Store
  drafts in memory by session, and clear them after a successful send or
  session deletion.
  [Desktop draft regression](../ui/e2e/specs/session.e2e.ts)
- [ ] The bundled content policy may block tool output images. The tool image
  decoded in the macOS desktop test. Other platforms have not been checked.
  [Image decoding check](../ui/e2e/specs/session.e2e.ts)
- [x] Keep Delete available while an agent is stuck starting. Disable
  Archive during startup and refuse requests to archive during startup.
  [Desktop startup regression](../ui/e2e/specs/archive.e2e.ts)

## Workflow run and session UI

- [ ] Align the icons in the run view. The checkbox and hourglass icons sit at
  different x-positions, so the icon column and the text start differ between
  rows.
- [x] Give the elapsed time a fixed width (tabular numerals or a reserved
  width) so a changing value does not shift the content to its right.
- [x] List sessions and workflow runs in the left panel newest-created first.
- [ ] Stop collapsing completed steps on the run dashboard. While the run is
  active, scroll to the active step's most recent status update.
- [ ] Use different colors for "working" and "success" in the sidebar. Both
  are green now.
- [ ] Show the status of the local checkout in the UI: current branch, staged
  and unstaged changes, and whether the session is in a worktree. Regular
  sessions show none of this. The run dashboard shows some of it for the run's
  worktree only. Nothing shows the base checkout's status.

## Agent instructions

- [ ] Agents post status updates too seldom. Make them post more reliably, by
  forcing it or by instructing more strongly. The tool description in
  [ox-mcp](../crates/ox-mcp/src/main.rs) is the only instruction today.
- [ ] Inject Ox-specific instructions into agent sessions, such as when to use
  the status update tool. ACP has no supported system prompt field
  (`session.systemPrompt` is unsupported in
  [ACP v2 support](../docs/dev/acp-v2.md)). Options:
  - Claude's `--append-system-prompt` in the
    [bridge runner](../crates/claude-code-acp/src/runner.rs).
  - The MCP server `instructions` field in `ox-mcp`.
  - Text prepended to each workflow step's prompt, which also works for Pi.
  - Pi has no MCP servers, so it needs the prompt option or a flag in its
    bridge.

## Workflow definitions

- [x] Confirm which `permission_mode` values each harness's bridge offers, and
  list them in [Workflows](../docs/workflows.md). The docs show only `manual`.
  Check what a Pi step does when it sets `permission_mode`.

## Persistence and recovery

### An event write failure can break replay after restart

- [ ] Deferred. An injected SQLite write failure left gaps in saved history.
  The writer drops the failed batch, and replay uses `after_seq` as a vector
  offset. A nonzero cursor stalled in the reproduction below. The desktop
  subscribes from zero and can still show incomplete history after a failed
  write. Revisit if a storage failure affects normal desktop use or a
  supported workflow depends on a nonzero cursor.
  [Event writer](../crates/oxd/src/store.rs) ·
  [Replay cursor](../crates/oxd/src/state.rs)

  1. Start the daemon explicitly as described in the
     [reproduction setup](#reproduction-setup). Start a session with
     `oxctl new --workspace "$workspace" -- "$bin/oxd-fake-agent"` and save
     its ID as `session`.
  2. Install this trigger in `"$OX_SOCKET.db"` using SQLite.

     ```sql
     CREATE TRIGGER injected BEFORE INSERT ON events
     WHEN json_extract(NEW.kind, '$.type') = 'prompt_started'
     BEGIN
       SELECT RAISE(ABORT, 'injected event write failure');
     END;
     ```

  3. Follow events with `oxctl attach "$session"` and send
     `oxctl prompt "$session" 'say 1 0'`. After `turn_ended` arrives, send
     the daemon SIGTERM and wait for it to exit. Its stderr logs the failed
     write. Inspect the database with
     `SELECT seq, json_extract(kind, '$.type') FROM events ORDER BY seq;`.
     The reproduced sequence was `1, 3, 4, 5, 6, 7`, ending in `turn_ended`.
     If the prompt and turn end landed in the same failed batch, repeat
     with a fresh session and `oxctl prompt "$session" wait`. Wait for the
     write error in stderr, then run `oxctl cancel "$session"` to record a
     later event in a separate batch.
  4. Restart `oxd` on the same socket and run
     `oxctl attach "$session" --after 6`, using one less than the
     final stored sequence if it differs. The subscription waits even though
     event 7 is already in the database.

### Unarchive can leave the session stuck after a database write fails

- [x] Make unarchive recoverable when Git restores the worktree and SQLite
  fails to clear the archived flag. A retry should be able to finish the
  transition safely. [Unarchive](../crates/oxd/src/daemon.rs)

  1. Start a session with `oxctl new --workspace "$workspace" --worktree --
     "$bin/oxd-fake-agent"` and save its ID as `session`. Record its checkout
     from `oxctl ls`, archive it with `oxctl archive "$session"`, and confirm
     the worktree is gone.
  2. Install this trigger in `"$OX_SOCKET.db"` using SQLite.

     ```sql
     CREATE TRIGGER injected BEFORE DELETE ON archived
     BEGIN
       SELECT RAISE(ABORT, 'injected archive-row write failure');
     END;
     ```

  3. Run `oxctl unarchive "$session"`. It reports the database error. The
     worktree exists again, and `oxctl ls` still reports `archived`.
  4. Run `DROP TRIGGER injected;` and retry unarchive. Git refuses because
     the checkout already exists, and the session remains archived.

## Simplifications

- [x] Request ACP conversation replay only when the saved history needs it.
  Use the existing `AgentStarted` check when choosing whether to send a
  replay cursor. Preserve the fallback for missing history and the live
  state, settings, and command updates on resume.
  [History check](../crates/oxd/src/state.rs) ·
  [Resume request](../crates/oxd/src/session.rs)
- [x] Resolve desktop questions and permissions by request ID. Send the ID
  when a request is resolved so React can remove its existing item. Drop
  the duplicate request map from the Rust transcript fold and keep the
  saved events and CLI stream as they are.
  [Transcript fold](../crates/ox/src/conversation.rs) ·
  [React reducer](../ui/src/features/conversation/useConversation.ts)

## Future work

- The pull request and review workflows, as the spec describes them.
- Workspace settings and the per-workspace default workflow, which matter
  once there is a second workflow to choose.

## Reproduction setup

For the persistence reproductions, run `cargo build -p oxd -p oxctl`
from the repository root. Use a fresh temporary directory and daemon for each
reproduction. The CLI, daemon and fake agent are in `target/debug`.

```sh
bin="$PWD/target/debug"
export PATH="$bin:$PATH"
scratch=$(mktemp -d /tmp/ox-repro.XXXXXX)
export OX_SOCKET="$scratch/oxd.sock"
repo="$scratch/repo"
git init -q -b main "$repo"
git -C "$repo" config user.name "Ox Reproduction"
git -C "$repo" config user.email "ox-repro@example.invalid"
git -C "$repo" commit --allow-empty -qm base
base=$(git -C "$repo" rev-parse HEAD)
workspace=$(oxctl workspace add "$repo")
```

`workspace add` starts the daemon for this socket. For the replay case, start
the daemon explicitly before that command so you have its PID. Use
`"$bin/oxd" --socket "$OX_SOCKET"` and wait for it to print `ready`.
