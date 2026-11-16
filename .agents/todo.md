# To do

## Workflow run and session UI

- [ ] Show the status of the local checkout in the UI: current branch, staged
  and unstaged changes, and whether the session is in a worktree. Regular
  sessions show none of this. The run dashboard shows some of it for the run's
  worktree only. Nothing shows the base checkout's status.

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

## Open questions

The functional spec describes current behavior. Investigate these before
changing behavior, and update the affected spec alongside any change.

- [ ] **Embed desktop image and PDF attachments?** Desktop attachments
  currently send local file references, including for images and PDFs.
  Decide whether supported images and PDFs should instead be embedded, and
  what should happen for agents that do not support that content. See
  [Conversations](../spec/conversations.md#compose-and-send).
- [ ] **Investigate withdrawn permission requests (suspected bug).** An agent
  can withdraw a permission request, but Ox leaves it pending until answered,
  the turn is cancelled, or the agent or session is stopped. Forms are
  removed when withdrawn. Determine whether permission withdrawals should
  likewise cancel and remove the request, including what happens when an
  answer arrives at the same time. See
  [Permissions and questions](../spec/requests.md#answer-a-permission-request).
- [ ] **Align form validation across clients (low priority).** The desktop
  enforces text patterns and formats; command-line answers do not. Shared
  checks already cover field names, required fields, value types, choices,
  lengths, ranges, and choice counts. Investigate consistent validation
  across clients, including invalid patterns and supported formats. See
  [Permissions and questions](../spec/requests.md#rules).
- [ ] **Decide what an empty MCP server list means on resume.** The Claude
  Code bridge replaces the previous servers when given a nonempty list but
  retains them when given an empty list, so an empty list cannot clear the
  configuration. Check the ACP contract and decide whether empty should clear
  the servers or whether retaining them is intended. See
  [Claude Code bridge](../spec/claude.md#mcp-servers).

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
