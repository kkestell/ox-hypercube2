# Database migrations and separate app and daemon reinstalls

## Goal

Saved data survives every new build: after a change, `make install` stops an
idle `oxd`, installs, and relaunches Ox, and the new `oxd` opens the existing
database. `make install-ui` reinstalls and relaunches only the app, leaving the
running `oxd` and its sessions undisturbed. An app whose protocol differs from
the running `oxd` says so instead of misbehaving.

## Related code

- `crates/oxd/src/store.rs` — `Store::open`/`Store::with` create the schema with `CREATE TABLE IF NOT EXISTS`; the doc says the schema has no migrations. JSON columns: `sessions.command`, `events.kind` (`EventKind`), `session_unseen.unseen` (`Unseen`), `session_queues.prompts` (`QueuedPrompt`s). The test store is `Store::with(Connection::open_in_memory())` (line ~590).
- `crates/oxd/src/main.rs` — `parse_args`, `run`: the main `select!` breaks on idle, SIGTERM, or SIGINT, then removes the socket, `close_all`, `flush`, and drains connections. `Store::open` is called at line ~141.
- `crates/oxd/src/conn.rs` — the per-connection loop; `Method::Hello` must come first, with `protocol_version == PROTOCOL_VERSION`, else `PROTOCOL_MISMATCH`.
- `crates/oxd/src/daemon.rs` — `Daemon::idle`, `is_idle`, `nothing_running`, `close_all`; sessions' `state()`.
- `crates/ox-protocol/src/messages.rs` — `PROTOCOL_VERSION: u32 = 1` (unchanged since the first commit), `Method::Hello { protocol_version: u32 }`, `HelloResult`, `SessionState`, `EventKind`, `code::PROTOCOL_MISMATCH`.
- `crates/ox-protocol/src/client.rs` — `Client::connect` (sends `hello`), `connect_or_start_with`, `daemon_binary` (oxd beside the current executable), `Error::Remote`.
- `crates/ox/src/lib.rs` — `App::client` connects or starts oxd; the watch reports a failure as `WatchMessage::Error`, which `ui/src/features/sessions/useWorkspaces.ts` shows as "Cannot reach oxd: …" with Retry.
- `crates/oxd/tests/common/mod.rs` — the test daemon harness (`start`, `client`, `stop`, `wait_for_exit`, `wait_for_log`); `crates/oxd/tests/daemon.rs`.
- `Makefile` — `install` builds everything, assembles and signs `target/release/bundle/macos/Ox.app`, and replaces `~/Applications/Ox.app`.
- UI local storage readers: `ui/src/features/composer/useUnsent.ts`, `ui/src/features/composer/controls.ts`, `ui/src/features/sessions/useDrafts.ts`, `ui/src/features/files/treeState.ts`.
- Docs: `README.md` (Install), `docs/dev/development.md`, `docs/dev/architecture.md` (Processes and communication; Persistence and recovery).

## Decisions

### Migrations

- **Schema version.** The database's `PRAGMA user_version` is its schema version. `store.rs` holds `MIGRATIONS: &[fn(&Transaction) -> rusqlite::Result<()>]`; step `i` takes a database from version `i` to `i + 1`, and the schema version this build writes is `MIGRATIONS.len()`.
- **Step 0 is today's schema.** It is the current `CREATE TABLE IF NOT EXISTS` batch, unchanged. A new database runs every step; an existing database (version 0, tables present) runs step 0 as a no-op and is stamped version 1. The connection pragmas (`journal_mode`, `synchronous`, `auto_vacuum`, `foreign_keys`) stay ahead of the steps, so `auto_vacuum` still precedes table creation.
- **One transaction.** `Store::with` runs every pending step and sets `user_version` in a single transaction; a failing step leaves the database as it was and `Store::open` fails.
- **Newer databases are refused.** A `user_version` above `MIGRATIONS.len()` fails `Store::open` with an error naming both versions ("the database is schema version 3; this oxd reads up to 2"). `oxd` logs it and exits as it does for any store error.
- **Stored JSON is schema.** Changing how a stored type serializes is a schema change. That covers `EventKind` and everything inside it, including the ACP `SessionUpdate`s from `agent-client-protocol-schema` (so bumping that crate counts), `Unseen`, `QueuedPrompt`, and the session command. Such a change either reads every older shape (new fields with `#[serde(default)]`, new variants) or comes with a migration step that rewrites the rows. The `EventKind` doc comment and architecture.md state this.
- **Fixtures of every schema version.** `crates/oxd/tests/fixtures/schema-1.sql` is a SQL dump (`sqlite3 <db> .dump`) of a database this build writes, trimmed to: one workspace, a worktree, an archived and an unarchived session, a queue, an unseen outcome, a deleted agent session ID, and events covering every `EventKind` variant plus the `SessionUpdate` kinds the bridges send (agent message and thought chunks, user message chunk, tool call, tool call update, plan, available commands, config options, current mode). It is generated once and never regenerated. A test in `store.rs` loads each `schema-*.sql` into an in-memory database, opens it with `Store::with`, and checks that the workspaces, worktrees, sessions (with archive, unseen, and queue), and every session's events read back, with expected counts. Each later schema version adds its own fixture, so every fixture keeps opening with the newest code.
- **Local storage.** UI local storage values follow the same rule: a shape change reads values the previous build wrote. This is a documented rule only; the four readers are unchanged.

### Protocol identity

- **The protocol version is derived from the protocol source.** `crates/ox-protocol/build.rs` hashes (FNV-1a 64, written inline, with no new dependency) `Cargo.toml` and every file under `src/`, in sorted path order, and emits it as `OX_PROTOCOL_VERSION` (16 hex digits), with `rerun-if-changed` on both. `PROTOCOL_VERSION: &str = env!("OX_PROTOCOL_VERSION")`. `Method::Hello` and `HelloResult` carry `protocol_version: String`. Any edit to `ox-protocol`, comments included, makes an app and a daemon from before and after incompatible; that only means using `make install` instead of `make install-ui`.
- **The app names the remedy.** `App::client` turns `Error::Remote` with code `PROTOCOL_MISMATCH` into "oxd is running a different build of Ox. Run `make install` to restart it." The text is built by the client because the running daemon is the older build.

### Stopping oxd

- **`daemon.stop` is the one method accepted before `hello`.** Its wire form, `{"id": n, "method": "daemon.stop"}`, and its answer stay fixed across builds, so any build's `oxd --stop` can stop any later build's daemon. The `Method` doc says so.
- **Only an idle daemon stops.** A session is busy in any state other than `Idle` or `Exited`. With a busy session, the answer is an `INVALID_STATE` error listing each one as `<title or id> (<state>)`, and the daemon keeps serving. Otherwise the daemon answers success and shuts down exactly as on SIGTERM: `Daemon` gets a stop signal (a `watch` or `Notify`) that `main`'s `select!` waits on beside idle, SIGTERM, and SIGINT.
- **`oxd --stop [--socket PATH]`.** It connects to the socket. If nothing answers, it prints "oxd is not running" and exits 0. Otherwise it writes the request line and reads the answer line, using a plain `UnixStream` with the line framing `conn.rs` uses (no `Client`, which would send `hello`). On an error answer it prints the message and exits 1. On success it reads until the daemon closes the connection, then exits 0.

### Make targets

- **`make install`** (the full reinstall):
  1. Build and assemble the signed bundle in `target/` as now. A build failure stops here with nothing running disturbed.
  2. Quit Ox if `~/Applications/Ox.app/Contents/MacOS/ox` is running (`osascript -e 'tell application "Ox" to quit'`, guarded by `pgrep -f`), and wait for it to exit. The app goes first because a running app restarts oxd from the old bundle as soon as its connection closes.
  3. Run `target/release/oxd --stop`. If it fails, relaunch Ox when it was running and fail with oxd's message.
  4. Replace `~/Applications/Ox.app` and install the config as now.
  5. `open ~/Applications/Ox.app`, which starts the new oxd.
- **`make install-ui`** (only the app):
  1. Fail with "Ox is not installed; run make install" when `~/Applications/Ox.app` is missing.
  2. Build the app bundle (`npm run tauri -- build --bundles app --no-sign`) without building the daemon crates.
  3. Copy `oxd`, `ox-mcp`, `claude-code-acp`, and `pi-acp` from the installed bundle into the new one, keeping their signatures, then sign and verify the bundle.
  4. Quit Ox as above, replace `~/Applications/Ox.app`, and `open` it. The running oxd, and any oxd the app later starts, is the installed one.
- The two targets share the quit, replace, and open steps as Make variables, as `DEV_OXD` is shared now.
- `make help` lists `install-ui`.

## Tests

- `store.rs`: a new database ends at version `MIGRATIONS.len()` with every table. A version-0 database holding today's tables and rows opens at version 1 with its rows intact. A database with a higher `user_version` fails to open, naming both versions. The fixture test above.
- `crates/oxd/tests/daemon.rs`: `daemon.stop` as the first message on an idle daemon is answered with success, and the daemon exits with its socket removed. With a running turn (fake agent), the answer is an error naming the session and its state, and a client connected beforehand keeps working. `oxd --stop` with no daemon exits 0. `oxd --stop` against an idle daemon exits 0 after the daemon has closed the connection.
- `conn.rs` or `messages.rs`: `hello` with a different `protocol_version` gets `PROTOCOL_MISMATCH`. The existing hello serialization test uses the string form.
- `crates/ox`: `App::client`'s mismatch message, if that function can be reached in a unit test without Tauri. Otherwise it is covered by inspection.
- `make install` and `make install-ui` are checked by hand on the real install: `install-ui` with a busy session leaves the session running; `install` with a busy session refuses and relaunches the app; `install` while idle relaunches with the new oxd.

## Docs

- `store.rs` module and `Store::open` docs describe the versioned schema.
- `docs/dev/architecture.md`: Persistence and recovery gains the schema version, migrations, the stored-JSON rule, local storage's rule, and the refusal of newer databases. Processes and communication gains the derived protocol version and `daemon.stop`.
- `docs/dev/development.md`: a section on installing that covers the two targets, when each applies (`install-ui` for changes outside `ox-protocol` and the daemon crates), and what `make install` does with busy sessions.
- `README.md` Install: `make install` stops an idle oxd and relaunches Ox. One line on `make install-ui`.
- `docs/dev/code-style.md` or the `EventKind` doc: changing a stored type comes with a fixture-compatible read or a migration step.

## Rollout

The `oxd` installed today predates `daemon.stop` and the string protocol
version, so the first `make install` after this change fails at the stop step.
For that one install, let every session go idle, quit Ox, and stop the daemon
with `pkill -f 'Ox.app/Contents/MacOS/oxd'` before running `make install`.
Every later install uses the targets alone.
