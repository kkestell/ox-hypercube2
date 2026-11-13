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

## Foundation fixes

Fix branch moves and source validation for local merge before adding the PR
and review workflows. Review rounds will reuse `branch.move` to update the
review checkout. Keep Git and session state in `oxd` and workflow choices in
the clients.

### Reproduction setup

Build with `cargo build -p oxd -p oxctl` from the repository root. Use a fresh
temporary directory and daemon for each reproduction. The CLI, daemon and
fake agent are in `target/debug`.

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

### Branch moves can overwrite local files or run alongside an agent

- [ ] Preserve ignored, untracked files that a branch move would overwrite.
  In the workspace checkout, commit a `.gitignore` containing `local.env`
  and save the new `main` tip as `main_tip`. Create `topic` from that tip,
  force-add and commit `local.env` with the contents `from topic`, and save
  the resulting commit as `topic_tip`. Switch back to `main`, write `keep me`
  to `local.env`, and run
  `oxctl move-branch --workspace "$workspace" --expected "$main_tip" main "$topic_tip"`.
  The move succeeds and the file contains `from topic`. It should refuse the
  move and preserve both the branch and the file. The existing test covers
  an untracked file that Git does not ignore.
  [Branch move](../crates/oxd/src/branches.rs) ·
  [Worktree tests](../crates/oxd/tests/worktrees.rs)
- [ ] Keep a checkout from starting agent work during a branch move. The
  daemon checks whether the checkout is busy once, before invoking `git
  status`, `update-index`, `update-ref`, and `read-tree`. The existing test
  starts the prompt before the move. Reproduce the opposite order using a
  temporary Git wrapper and FIFOs, with the setup above.
  [Busy check](../crates/oxd/src/daemon.rs) ·
  [Branch move](../crates/oxd/src/branches.rs)

  ```sh
  git -C "$repo" switch -qc topic
  printf 'new\n' > "$repo/new"
  git -C "$repo" add new
  git -C "$repo" commit -qm topic
  target=$(git -C "$repo" rev-parse HEAD)
  git -C "$repo" switch -q main
  session=$("$bin/oxctl" new --workspace "$workspace" -- "$bin/oxd-fake-agent")

  export OX_REPRO_REAL_GIT=$(command -v git)
  export OX_REPRO_READY="$scratch/status-ready" OX_REPRO_RELEASE="$scratch/status-release"
  mkfifo "$OX_REPRO_READY" "$OX_REPRO_RELEASE"
  mkdir "$scratch/bin"
  cat > "$scratch/bin/git" <<'EOF'
  #!/bin/sh
  if [ "$3" = --no-optional-locks ] && [ "$4" = status ]; then
      printf 'ready\n' > "$OX_REPRO_READY"
      IFS= read -r release < "$OX_REPRO_RELEASE"
  fi
  exec "$OX_REPRO_REAL_GIT" "$@"
  EOF
  chmod +x "$scratch/bin/git"

  PATH="$scratch/bin:$PATH" "$bin/oxctl" move-branch \
      --workspace "$workspace" --expected "$base" main "$target" &
  move_pid=$!
  read -r ready < "$OX_REPRO_READY"
  "$bin/oxctl" prompt "$session" wait
  "$bin/oxctl" ls
  printf 'release\n' > "$OX_REPRO_RELEASE"
  wait "$move_pid"
  git -C "$repo" rev-parse HEAD
  "$bin/oxctl" ls
  ```

  The first `ls` shows `running`. The move succeeds, `HEAD` reaches `target`,
  and the second `ls` still shows `running`.

### Local merge can land an obsolete task commit

- [ ] Validate the source branch and its expected tip as part of the merge
  request. The button sends the source commit from the last comparison.
  The daemon checks the expected destination commit and accepts the supplied
  source SHA even if the task branch has moved.
  [Merge button](../ui/src/features/sessions/MergeButton.tsx) ·
  [Branch move](../crates/oxd/src/branches.rs)

  1. Create a task with `oxctl new --workspace "$workspace" --worktree
     --compare main -- "$bin/oxd-fake-agent"` and save its ID as `session`.
     Find its checkout with
     `oxctl ls` and make one commit there. Save its SHA as `old_tip`.
  2. Run `oxctl compare "$session"`. It reports the task one commit ahead of
     `main` and publishes `old_tip`.
  3. In the task checkout, reset to `base` and make a different commit.
     Leave `main` at `base` and leave the published comparison as it is.
  4. Send the same move as the button using `oxctl move-branch --workspace
     "$workspace" --expected "$base" main "$old_tip"`. It succeeds and
     `main` points to `old_tip` while the task branch points to the new
     commit. The merge should refuse the stale source.

### An event write failure can break replay after restart

- [ ] Replay by actual event sequence numbers and surface event write failures
  to callers that rely on persistence. The writer logs a failed batch and
  continues, so SQLite can contain gaps. Replay uses `after_seq` as a vector
  offset, and the flush acknowledgment can succeed after a failed batch.
  [Event writer](../crates/oxd/src/store.rs) ·
  [Replay cursor](../crates/oxd/src/state.rs)

  1. Start the daemon explicitly as described above. Start a session with
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

  3. Follow events with `oxctl attach "$session" --json` and send
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
     `oxctl attach "$session" --json --after 6`, using one less than the
     final stored sequence if it differs. The subscription waits even though
     event 7 is already in the database.

### Unarchive can leave the session stuck after a database write fails

- [ ] Make unarchive recoverable when Git restores the worktree and SQLite
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

The review reproduced these failures in temporary repositories. `make
check-rust`, `make check-ui`, and all 48 desktop end-to-end tests passed.
The reproduction used the fake agent. The live Claude smoke test was skipped.

## Future work

- The pull request and review workflows, as the spec describes them.
- Workspace settings and the per-workspace default workflow, which matter
  once there is a second workflow to choose.
