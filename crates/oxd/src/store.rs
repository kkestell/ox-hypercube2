//! The daemon's saved workspaces, worktrees and sessions, in one `SQLite`
//! database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use ox_protocol::schema::SessionUpdate;
use ox_protocol::{ConfigSetting, Event, EventKind, Unseen};
use rusqlite::{Connection, Transaction, params};

use crate::state::{self, Queue};

/// A workspace as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedWorkspace {
    pub id: String,
    pub path: PathBuf,
}

/// A session as the database holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedSession {
    pub number: u64,
    pub id: String,
    pub workspace_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub agent_session_id: Option<String>,
    /// The ID of the worktree the session runs in.
    pub worktree_id: Option<String>,
    /// The seq of the session's last stored event, 0 if it has none.
    pub last_seq: u64,
    /// Whether the session is archived.
    pub archived: bool,
    /// When the session was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
    pub unseen: Option<Unseen>,
    pub queue: Queue,
    /// The session's config.
    pub config: Vec<ConfigSetting>,
    /// Whether the session's events hold neither an `agent_started` nor a
    /// `prompt_started`, so that loading it asks the agent to replay its
    /// conversation.
    pub needs_replay: bool,
}

/// What a stop left open in a session's events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LeftOpen {
    /// Whether a turn started and did not end.
    pub turn: bool,
    /// The ID of each permission request or elicitation with no resolution,
    /// in seq order, and whether it is a permission request.
    pub requests: Vec<(String, bool)>,
    /// The seq of each `prompt_steered` event that no
    /// `steered_prompt_taken` or `steered_prompt_dropped` names, in order.
    pub steered: Vec<u64>,
}

/// A worktree as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedWorktree {
    pub id: String,
    /// The worktree's top level.
    pub path: PathBuf,
    pub branch: String,
    /// The commit the worktree started at.
    pub base: String,
    /// Whether the daemon created the branch for the worktree.
    pub created_branch: bool,
    /// The ref the branch is compared with, as the client gave it.
    pub compare: Option<String>,
    /// The repository's absolute common Git directory.
    pub common_dir: PathBuf,
}

/// One change to a session's saved data.
#[derive(Clone, Debug, PartialEq)]
pub enum RowChange {
    Title(Option<String>),
    AgentSessionId(String),
    Config(Vec<ConfigSetting>),
    Unseen(Option<Unseen>),
    Archived(bool),
    Queue(Queue),
    /// The session's worktree could not be added, so its row is deleted and
    /// the session keeps no worktree.
    WorktreeForgotten(String),
}

/// The events `Store::events` reads under one hold of the connection.
const READ_BATCH: u64 = 256;

/// One connection to the database. Every query but a read of a session's
/// events is a small local read or write, so it runs inline.
pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    /// Opens the database at `path`, creating it if it does not exist, and
    /// brings its schema to the version this build writes. A database of a
    /// newer schema version than this build reads is refused.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::with(Connection::open(path)?)
    }

    fn with(mut connection: Connection) -> rusqlite::Result<Self> {
        // `auto_vacuum` takes effect only before the first table is created.
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA auto_vacuum = FULL;
            PRAGMA foreign_keys = ON;",
        )?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// A session's stored events, in order. They are read `READ_BATCH` at a
    /// time, releasing the connection between reads, so that a long history
    /// does not hold up every session's writes.
    pub fn events(&self, session_id: &str) -> rusqlite::Result<Vec<Event>> {
        let mut events: Vec<Event> = Vec::new();
        loop {
            let after = events.last().map_or(0, |event| event.seq);
            let batch = self.events_after(session_id, after, READ_BATCH)?;
            let done = batch.len() < usize::try_from(READ_BATCH).expect("a batch fits a usize");
            events.extend(batch);
            if done {
                return Ok(events);
            }
        }
    }

    /// The first `limit` of a session's stored events with `seq > after_seq`,
    /// in order. Seqs an event write lost are skipped.
    pub fn events_after(
        &self,
        session_id: &str,
        after_seq: u64,
        limit: u64,
    ) -> rusqlite::Result<Vec<Event>> {
        let connection = self.lock();
        let mut query = connection.prepare_cached(
            "SELECT seq, recorded_at, kind FROM events
            WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        query
            .query_map(
                params![session_id, seq_value(after_seq), seq_value(limit)],
                |row| {
                    Ok(Event {
                        session_id: session_id.into(),
                        seq: u64::try_from(row.get::<_, i64>(0)?).expect("seqs are positive"),
                        recorded_at: time_value(row, 1)?,
                        kind: json_value(row, 2)?,
                    })
                },
            )?
            .collect()
    }

    /// What a stop left open in each session's events, for the sessions
    /// with anything open. Each query reads only the `events_by_type`
    /// entries of the event types it names, never a whole history.
    pub fn left_open(&self) -> rusqlite::Result<HashMap<String, LeftOpen>> {
        let connection = self.lock();
        let mut open: HashMap<String, LeftOpen> = HashMap::new();
        let turns = connection
            .prepare(LEFT_OPEN_TURNS)?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for session_id in turns {
            open.entry(session_id).or_default().turn = true;
        }
        let requests = connection
            .prepare(LEFT_OPEN_REQUESTS)?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)? == "permission_requested",
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (session_id, request_id, permission) in requests {
            open.entry(session_id)
                .or_default()
                .requests
                .push((request_id, permission));
        }
        let steered = connection
            .prepare(LEFT_OPEN_STEERED)?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    u64::try_from(row.get::<_, i64>(1)?).expect("seqs are positive"),
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (session_id, seq) in steered {
            open.entry(session_id).or_default().steered.push(seq);
        }
        Ok(open)
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.connection.lock().unwrap()
    }

    /// Every workspace, in the order they were added.
    pub fn workspaces(&self) -> rusqlite::Result<Vec<SavedWorkspace>> {
        let connection = self.lock();
        let mut query = connection.prepare("SELECT id, path FROM workspaces ORDER BY rowid")?;
        query
            .query_map([], |row| {
                Ok(SavedWorkspace {
                    id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                })
            })?
            .collect()
    }

    /// Saves a workspace. Returns false, saving nothing, if another
    /// workspace has its path.
    pub fn add_workspace(&self, workspace: &SavedWorkspace) -> rusqlite::Result<bool> {
        let added = self.lock().execute(
            "INSERT INTO workspaces (id, path) VALUES (?1, ?2) ON CONFLICT (path) DO NOTHING",
            params![workspace.id, workspace.path.to_string_lossy()],
        )?;
        Ok(added == 1)
    }

    /// Deletes a workspace with its sessions, their events and their
    /// worktrees. The sessions' agent session IDs are not remembered, so an
    /// import brings them back.
    pub fn remove_workspace(&self, id: &str) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM worktrees
            WHERE id IN (SELECT worktree_id FROM sessions WHERE workspace_id = ?1)",
            [id],
        )?;
        transaction.execute("DELETE FROM sessions WHERE workspace_id = ?1", [id])?;
        transaction.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        transaction.commit()
    }

    /// Every worktree, in the order they were added.
    pub fn worktrees(&self) -> rusqlite::Result<Vec<SavedWorktree>> {
        let connection = self.lock();
        let mut query = connection.prepare(
            "SELECT id, path, branch, base, created_branch, common_dir, compare
            FROM worktrees ORDER BY rowid",
        )?;
        query
            .query_map([], |row| {
                Ok(SavedWorktree {
                    id: row.get(0)?,
                    path: PathBuf::from(row.get::<_, String>(1)?),
                    branch: row.get(2)?,
                    base: row.get(3)?,
                    created_branch: row.get(4)?,
                    common_dir: PathBuf::from(row.get::<_, String>(5)?),
                    compare: row.get(6)?,
                })
            })?
            .collect()
    }

    /// Every session, in the order they were created.
    pub fn sessions(&self) -> rusqlite::Result<Vec<SavedSession>> {
        let connection = self.lock();
        let mut query = connection.prepare(SESSIONS)?;
        query
            .query_map([], |row| {
                Ok(SavedSession {
                    number: u64::try_from(row.get::<_, i64>(0)?).expect("rowids are positive"),
                    id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    command: json_value(row, 3)?,
                    cwd: PathBuf::from(row.get::<_, String>(4)?),
                    title: row.get(5)?,
                    agent_session_id: row.get(6)?,
                    worktree_id: row.get(7)?,
                    last_seq: u64::try_from(row.get::<_, i64>(8)?).expect("seqs are positive"),
                    archived: row.get(9)?,
                    created_at: time_value(row, 10)?,
                    unseen: parse_unseen(row, 11)?,
                    queue: Queue {
                        paused: row.get::<_, Option<bool>>(12)?.unwrap_or(false),
                        prompts: match row.get::<_, Option<String>>(13)? {
                            Some(prompts) => serde_json::from_str(&prompts).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    13,
                                    rusqlite::types::Type::Text,
                                    Box::new(error),
                                )
                            })?,
                            None => Vec::new(),
                        },
                    },
                    config: json_value(row, 14)?,
                    needs_replay: row.get(15)?,
                })
            })?
            .collect()
    }

    /// Saves a session that has not started, created at `created_at`, with
    /// its config and the worktree it runs in, if any, and returns its
    /// number, which is greater than the number of every session saved
    /// before it.
    #[expect(clippy::too_many_arguments, reason = "the session's saved fields")]
    pub fn add_session(
        &self,
        id: &str,
        workspace_id: &str,
        command: &[String],
        cwd: &Path,
        worktree: Option<&SavedWorktree>,
        created_at: u64,
        config: &[ConfigSetting],
    ) -> rusqlite::Result<u64> {
        let command = serde_json::to_string(command).expect("a list of strings serializes");
        let config = serde_json::to_string(config).expect("settings serialize");
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if let Some(worktree) = worktree {
            insert_worktree(&transaction, worktree)?;
        }
        transaction.execute(
            "INSERT INTO sessions (id, workspace_id, command, cwd, worktree_id, created_at, config)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                workspace_id,
                command,
                cwd.to_string_lossy(),
                worktree.map(|worktree| &worktree.id),
                time(created_at),
                config
            ],
        )?;
        // `number` is the table's rowid.
        let number = u64::try_from(transaction.last_insert_rowid()).expect("rowids are positive");
        transaction.commit()?;
        Ok(number)
    }

    /// Saves an imported session with its title, agent session ID, creation
    /// time, config, whether it is archived, and its events, in one
    /// transaction, and returns its number. The session's other fields are
    /// ignored.
    pub fn import_session(
        &self,
        session: &SavedSession,
        events: &[Event],
    ) -> rusqlite::Result<u64> {
        let command =
            serde_json::to_string(&session.command).expect("a list of strings serializes");
        let config = serde_json::to_string(&session.config).expect("settings serialize");
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO sessions
                (id, workspace_id, command, cwd, title, agent_session_id, created_at, config)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session.id,
                session.workspace_id,
                command,
                session.cwd.to_string_lossy(),
                session.title,
                session.agent_session_id,
                time(session.created_at),
                config
            ],
        )?;
        let number = u64::try_from(transaction.last_insert_rowid()).expect("rowids are positive");
        if session.archived {
            transaction.execute(
                "INSERT INTO archived (session_id) VALUES (?1)",
                [&session.id],
            )?;
        }
        insert_events(&transaction, events)?;
        transaction.commit()?;
        Ok(number)
    }

    /// Whether a deleted session had the agent session ID.
    pub fn was_deleted(&self, agent_session_id: &str) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS (SELECT 1 FROM deleted_agent_sessions WHERE agent_session_id = ?1)",
            [agent_session_id],
            |row| row.get(0),
        )
    }

    /// Writes `changes` to session `id`'s saved data and stores `events`, in
    /// one transaction, so that nothing is written if one fails. Events of a
    /// session that is no longer saved are dropped.
    pub fn save_session(
        &self,
        id: &str,
        changes: &[RowChange],
        events: &[Event],
    ) -> rusqlite::Result<()> {
        if changes.is_empty() && events.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        for change in changes {
            match change {
                RowChange::Title(title) => transaction.execute(
                    "UPDATE sessions SET title = ?2 WHERE id = ?1",
                    params![id, title],
                )?,
                RowChange::AgentSessionId(agent_session_id) => transaction.execute(
                    "UPDATE sessions SET agent_session_id = ?2 WHERE id = ?1",
                    params![id, agent_session_id],
                )?,
                RowChange::Config(config) => transaction.execute(
                    "UPDATE sessions SET config = ?2 WHERE id = ?1",
                    params![
                        id,
                        serde_json::to_string(config).expect("settings serialize")
                    ],
                )?,
                RowChange::Unseen(Some(unseen)) => transaction.execute(
                    "INSERT OR REPLACE INTO session_unseen (session_id, unseen) VALUES (?1, ?2)",
                    params![id, unseen_name(*unseen)],
                )?,
                RowChange::Unseen(None) => {
                    transaction.execute("DELETE FROM session_unseen WHERE session_id = ?1", [id])?
                }
                RowChange::Archived(true) => transaction.execute(
                    "INSERT OR IGNORE INTO archived (session_id) VALUES (?1)",
                    [id],
                )?,
                RowChange::Archived(false) => {
                    transaction.execute("DELETE FROM archived WHERE session_id = ?1", [id])?
                }
                RowChange::Queue(queue) => transaction.execute(
                    "INSERT OR REPLACE INTO session_queues (session_id, paused, prompts)
                    VALUES (?1, ?2, ?3)",
                    params![
                        id,
                        queue.paused,
                        serde_json::to_string(&queue.prompts).expect("prompts serialize")
                    ],
                )?,
                RowChange::WorktreeForgotten(worktree_id) => {
                    transaction.execute("DELETE FROM worktrees WHERE id = ?1", [worktree_id])?
                }
            };
        }
        insert_events(&transaction, events)?;
        transaction.commit()
    }

    /// Deletes a session, with its events and the worktree it runs in, and
    /// remembers its agent session ID, if it has one.
    pub fn remove_session(&self, id: &str) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO deleted_agent_sessions (agent_session_id)
            SELECT agent_session_id FROM sessions
            WHERE id = ?1 AND agent_session_id IS NOT NULL",
            [id],
        )?;
        transaction.execute(
            "DELETE FROM worktrees
            WHERE id = (SELECT worktree_id FROM sessions WHERE id = ?1)",
            [id],
        )?;
        transaction.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        transaction.commit()
    }
}

/// Every session, in the order they were created. Whether a session needs
/// replay is found through `events_by_type`.
const SESSIONS: &str = "SELECT number, id, workspace_id, command, cwd, title, agent_session_id,
        worktree_id,
        (SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = sessions.id),
        EXISTS (SELECT 1 FROM archived WHERE session_id = sessions.id),
        created_at,
        (SELECT unseen FROM session_unseen WHERE session_id = sessions.id),
        session_queues.paused, session_queues.prompts, config,
        NOT EXISTS (SELECT 1 FROM events
            WHERE kind ->> '$.type' = 'agent_started' AND session_id = sessions.id)
        AND NOT EXISTS (SELECT 1 FROM events
            WHERE kind ->> '$.type' = 'prompt_started' AND session_id = sessions.id)
    FROM sessions
    LEFT JOIN session_queues ON session_queues.session_id = sessions.id
    ORDER BY number";

/// The sessions whose last `prompt_started` or `turn_started` comes after
/// their last `turn_ended`, by three index lookups each.
const LEFT_OPEN_TURNS: &str = "SELECT id FROM sessions
    WHERE MAX(
        COALESCE((SELECT MAX(seq) FROM events
            WHERE kind ->> '$.type' = 'prompt_started' AND session_id = sessions.id), 0),
        COALESCE((SELECT MAX(seq) FROM events
            WHERE kind ->> '$.type' = 'turn_started' AND session_id = sessions.id), 0)
    ) > COALESCE((SELECT MAX(seq) FROM events
        WHERE kind ->> '$.type' = 'turn_ended' AND session_id = sessions.id), 0)
    ORDER BY number";

/// Each request with no resolution: its session, its ID and the type of the
/// event that requested it, in seq order. A request ID is unique within its
/// session across permission requests and elicitations.
const LEFT_OPEN_REQUESTS: &str = "SELECT session_id, kind ->> '$.data.request_id' AS request_id,
        MIN(kind ->> '$.type')
    FROM events
    WHERE kind ->> '$.type' IN ('permission_requested', 'permission_resolved',
        'elicitation_requested', 'elicitation_resolved')
    GROUP BY session_id, request_id
    HAVING COUNT(*) = 1
        AND MIN(kind ->> '$.type') IN ('permission_requested', 'elicitation_requested')
    ORDER BY session_id, MIN(seq)";

/// Each steered prompt that was neither taken nor dropped: its session and
/// the seq of its `prompt_steered` event, which the answer names.
const LEFT_OPEN_STEERED: &str = "SELECT session_id,
        CASE kind ->> '$.type' WHEN 'prompt_steered' THEN seq
            ELSE kind ->> '$.data.prompt' END AS prompt
    FROM events
    WHERE kind ->> '$.type' IN ('prompt_steered', 'steered_prompt_taken', 'steered_prompt_dropped')
    GROUP BY session_id, prompt
    HAVING COUNT(*) = 1 AND MIN(kind ->> '$.type') = 'prompt_steered'
    ORDER BY session_id, prompt";

/// The steps that bring a database to the schema this build writes. Step
/// `i` takes a database from schema version `i` to `i + 1`; the schema
/// version, kept in `PRAGMA user_version`, is the number of steps applied.
///
/// Stored JSON is schema: changing how `EventKind`, `QueuedPrompt`,
/// `ConfigSetting` or a session's command serializes either keeps reading
/// every older shape or comes with a step that rewrites the rows. Each schema
/// version has a fixture in `tests/fixtures`, which every later build opens.
const MIGRATIONS: &[fn(&Transaction<'_>) -> rusqlite::Result<()>] =
    &[create_tables, add_session_config, add_events_by_type];

/// The schema version this build writes.
fn schema_version() -> i64 {
    i64::try_from(MIGRATIONS.len()).expect("the migrations are few")
}

/// Applies the steps the database has not had and records its new version,
/// in one transaction. Fails, changing nothing, if a step fails or the
/// database's version is newer than this build's.
fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let latest = schema_version();
    if version > latest {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some(format!(
                "the database is schema version {version}; this oxd reads up to {latest}"
            )),
        ));
    }
    if version == latest {
        return Ok(());
    }
    let applied = usize::try_from(version).expect("schema versions are not negative");
    for step in &MIGRATIONS[applied..] {
        step(&transaction)?;
    }
    transaction.pragma_update(None, "user_version", latest)?;
    transaction.commit()
}

/// Schema version 1: the tables as they were before the schema had versions,
/// which databases of that time already hold.
fn create_tables(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS workspaces (
            id   TEXT PRIMARY KEY,
            path TEXT NOT NULL UNIQUE
        );
        CREATE TABLE IF NOT EXISTS worktrees (
            id             TEXT PRIMARY KEY,
            path           TEXT NOT NULL,
            branch         TEXT NOT NULL,
            base           TEXT NOT NULL,
            created_branch INTEGER NOT NULL,
            common_dir     TEXT NOT NULL,
            compare        TEXT
        );
        CREATE TABLE IF NOT EXISTS sessions (
            number           INTEGER PRIMARY KEY AUTOINCREMENT,
            id               TEXT NOT NULL UNIQUE,
            workspace_id     TEXT NOT NULL REFERENCES workspaces(id),
            command          TEXT NOT NULL,  -- JSON array
            cwd              TEXT NOT NULL,
            title            TEXT,
            agent_session_id TEXT,
            worktree_id      TEXT REFERENCES worktrees(id) ON DELETE SET NULL,
            created_at       INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS events (
            session_id  TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            seq         INTEGER NOT NULL,
            recorded_at INTEGER NOT NULL,
            kind        TEXT    NOT NULL,  -- JSON of EventKind
            PRIMARY KEY (session_id, seq)
        ) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS archived (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE
        );
        CREATE TABLE IF NOT EXISTS session_unseen (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
            unseen     TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS session_queues (
            session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
            paused     INTEGER NOT NULL,
            prompts    TEXT NOT NULL  -- JSON array of QueuedPrompt
        );
        -- The agent session IDs of deleted sessions, which an import skips.
        CREATE TABLE IF NOT EXISTS deleted_agent_sessions (
            agent_session_id TEXT PRIMARY KEY
        );",
    )
}

/// Schema version 2: each session's config, a JSON array of
/// `ConfigSetting`. A session takes the config its latest `agent_started` or
/// `config_option_update` event reports, and is left empty without one.
fn add_session_config(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction
        .execute_batch("ALTER TABLE sessions ADD COLUMN config TEXT NOT NULL DEFAULT '[]'")?;
    // With `MAX(seq)`, SQLite takes the bare `kind` from the row with the
    // greatest seq, so each session's latest report is found in one scan.
    let reports: Vec<(String, EventKind)> = transaction
        .prepare(
            "SELECT session_id, kind, MAX(seq) FROM events
            WHERE kind ->> '$.type' = 'agent_started'
                OR kind ->> '$.data.sessionUpdate' = 'config_option_update'
            GROUP BY session_id",
        )?
        .query_map([], |row| Ok((row.get(0)?, json_value(row, 1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut update = transaction.prepare("UPDATE sessions SET config = ?2 WHERE id = ?1")?;
    for (session_id, kind) in reports {
        let options = match kind {
            EventKind::AgentStarted { config_options, .. } => config_options,
            EventKind::Update(SessionUpdate::ConfigOptionUpdate(update)) => update.config_options,
            other => unreachable!("the query selects only option reports, not {other:?}"),
        };
        let config =
            serde_json::to_string(&state::reported_config(&options)).expect("settings serialize");
        update.execute(params![session_id, config])?;
    }
    Ok(())
}

/// Schema version 3: an index of events by type, so that startup finds what
/// a stop left open, and whether a session needs replay, without reading
/// message updates. A query uses it when it filters on the same expression.
fn add_events_by_type(transaction: &Transaction<'_>) -> rusqlite::Result<()> {
    transaction
        .execute_batch("CREATE INDEX events_by_type ON events (kind ->> '$.type', session_id, seq)")
}

fn insert_worktree(
    transaction: &Transaction<'_>,
    worktree: &SavedWorktree,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO worktrees (id, path, branch, base, created_branch, common_dir, compare)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            worktree.id,
            worktree.path.to_string_lossy(),
            worktree.branch,
            worktree.base,
            worktree.created_branch,
            worktree.common_dir.to_string_lossy(),
            worktree.compare
        ],
    )?;
    Ok(())
}

/// Inserts events, dropping those of a session that is not saved.
fn insert_events(transaction: &Transaction<'_>, events: &[Event]) -> rusqlite::Result<()> {
    let mut insert = transaction.prepare_cached(
        "INSERT INTO events (session_id, seq, recorded_at, kind)
        SELECT ?1, ?2, ?3, ?4
        WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?1)",
    )?;
    for event in events {
        let seq = seq_value(event.seq);
        let kind = serde_json::to_string(&event.kind).expect("an event kind serializes");
        insert.execute(params![
            event.session_id,
            seq,
            time(event.recorded_at),
            kind
        ])?;
    }
    Ok(())
}

/// A seq or a count of events, as `SQLite` takes it. Counts past the largest
/// integer `SQLite` holds are capped there.
fn seq_value(seq: u64) -> i64 {
    i64::try_from(seq).unwrap_or(i64::MAX)
}

/// A time in milliseconds since the Unix epoch, as its column holds it.
fn time(millis: u64) -> i64 {
    i64::try_from(millis).expect("times fit an i64")
}

/// The time in column `index`.
fn time_value(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    Ok(u64::try_from(row.get::<_, i64>(index)?).expect("times are positive"))
}

/// An unseen outcome as its column holds it.
fn unseen_name(unseen: Unseen) -> &'static str {
    match unseen {
        Unseen::Finished => "finished",
        Unseen::Error => "error",
    }
}

/// The JSON value in column `index`.
fn json_value<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// The unseen outcome in column `index`, which holds its name or `NULL`.
fn parse_unseen(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<Unseen>> {
    match row.get::<_, Option<String>>(index)?.as_deref() {
        None => Ok(None),
        Some("finished") => Ok(Some(Unseen::Finished)),
        Some("error") => Ok(Some(Unseen::Error)),
        Some(name) => Err(rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            format!("unknown unseen outcome {name:?}").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use ox_protocol::schema::{ContentBlock, TextContent};

    use super::*;

    fn store() -> Store {
        Store::with(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn workspace(id: &str, path: &str) -> SavedWorkspace {
        SavedWorkspace {
            id: id.into(),
            path: path.into(),
        }
    }

    fn command() -> Vec<String> {
        vec!["agent".into(), "--flag".into()]
    }

    #[test]
    fn workspaces_round_trip_in_the_order_they_were_added() {
        let store = store();
        assert!(store.workspaces().unwrap().is_empty());
        let second = workspace("b", "/second");
        let first = workspace("a", "/first");
        assert!(store.add_workspace(&second).unwrap());
        assert!(store.add_workspace(&first).unwrap());
        assert_eq!(store.workspaces().unwrap(), [second, first.clone()]);

        store.remove_workspace("b").unwrap();
        assert_eq!(store.workspaces().unwrap(), [first]);
    }

    #[test]
    fn sessions_round_trip_in_the_order_they_were_created() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        assert!(store.sessions().unwrap().is_empty());
        let first = store
            .add_session(
                "one",
                "w",
                &command(),
                Path::new("/work"),
                None,
                1_700_000_000_000,
                &[],
            )
            .unwrap();
        let second = store
            .add_session(
                "two",
                "w",
                &command(),
                Path::new("/work"),
                None,
                1_700_000_000_001,
                &[],
            )
            .unwrap();
        assert!(first < second);
        assert_eq!(
            store.sessions().unwrap(),
            [
                SavedSession {
                    number: first,
                    id: "one".into(),
                    workspace_id: "w".into(),
                    command: command(),
                    cwd: "/work".into(),
                    title: None,
                    agent_session_id: None,
                    worktree_id: None,
                    last_seq: 0,
                    archived: false,
                    created_at: 1_700_000_000_000,
                    unseen: None,
                    queue: Queue::default(),
                    config: Vec::new(),
                    needs_replay: true,
                },
                SavedSession {
                    number: second,
                    id: "two".into(),
                    workspace_id: "w".into(),
                    command: command(),
                    cwd: "/work".into(),
                    title: None,
                    agent_session_id: None,
                    worktree_id: None,
                    last_seq: 0,
                    archived: false,
                    created_at: 1_700_000_000_001,
                    unseen: None,
                    queue: Queue::default(),
                    config: Vec::new(),
                    needs_replay: true,
                },
            ]
        );
    }

    fn setting(config_id: &str, value: &str) -> ConfigSetting {
        ConfigSetting {
            config_id: config_id.into(),
            value: ox_protocol::ConfigValue::Id(value.into()),
        }
    }

    #[test]
    fn saved_title_agent_session_id_and_config_read_back() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session(
                "s",
                "w",
                &command(),
                Path::new("/work"),
                None,
                0,
                &[setting("model", "smart")],
            )
            .unwrap();
        assert_eq!(
            store.sessions().unwrap()[0].config,
            [setting("model", "smart")]
        );
        let config = vec![setting("model", "fast"), setting("effort", "low")];
        store
            .save_session(
                "s",
                &[
                    RowChange::Title(Some("Fix the build".into())),
                    RowChange::AgentSessionId("agent".into()),
                    RowChange::Config(config.clone()),
                ],
                &[],
            )
            .unwrap();
        let saved = &store.sessions().unwrap()[0];
        assert_eq!(saved.title.as_deref(), Some("Fix the build"));
        assert_eq!(saved.agent_session_id.as_deref(), Some("agent"));
        assert_eq!(saved.config, config);

        store
            .save_session("s", &[RowChange::Title(None)], &[])
            .unwrap();
        assert_eq!(store.sessions().unwrap()[0].title, None);
    }

    #[test]
    fn a_deleted_session_is_gone_and_its_number_is_not_reused() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        let first = store
            .add_session("one", "w", &command(), Path::new("/work"), None, 0, &[])
            .unwrap();
        store.remove_session("one").unwrap();
        assert!(store.sessions().unwrap().is_empty());
        let second = store
            .add_session("two", "w", &command(), Path::new("/work"), None, 0, &[])
            .unwrap();
        assert!(second > first);
    }

    #[test]
    fn removing_a_workspace_deletes_its_sessions_without_remembering_them() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store.add_workspace(&workspace("v", "/other")).unwrap();
        add_worktree_session(&store, "s", &worktree("t"));
        store
            .add_session("o", "v", &command(), Path::new("/other"), None, 0, &[])
            .unwrap();
        store
            .save_session("s", &[RowChange::AgentSessionId("claude".into())], &[])
            .unwrap();
        store.remove_workspace("w").unwrap();
        assert_eq!(store.workspaces().unwrap(), [workspace("v", "/other")]);
        let sessions: Vec<_> = store
            .sessions()
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(sessions, ["o"]);
        assert!(store.worktrees().unwrap().is_empty());
        assert!(!store.was_deleted("claude").unwrap());
    }

    fn worktree(id: &str) -> SavedWorktree {
        SavedWorktree {
            id: id.into(),
            path: format!("/worktrees/work-{id}").into(),
            branch: format!("ox/{id}"),
            base: "abc".into(),
            created_branch: true,
            compare: None,
            common_dir: "/repos/work/.git".into(),
        }
    }

    fn add_worktree_session(store: &Store, id: &str, worktree: &SavedWorktree) {
        store
            .add_session(id, "w", &command(), &worktree.path, Some(worktree), 0, &[])
            .unwrap();
    }

    fn count(store: &Store, table: &str) -> i64 {
        store
            .lock()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn a_forgotten_worktree_leaves_its_session_without_one() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_worktree_session(&store, "s", &worktree("one"));
        store
            .save_session("s", &[RowChange::WorktreeForgotten("one".into())], &[])
            .unwrap();
        assert!(store.worktrees().unwrap().is_empty());
        assert_eq!(store.sessions().unwrap()[0].worktree_id, None);
    }

    #[test]
    fn a_deleted_session_takes_its_worktree_and_events() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_worktree_session(&store, "s", &worktree("one"));
        add_worktree_session(&store, "t", &worktree("two"));
        log(&store, "s", &[exited("s", 1)]);
        log(&store, "t", &[exited("t", 1)]);
        store.remove_session("s").unwrap();
        assert_eq!(store.worktrees().unwrap(), [worktree("two")]);
        assert!(store.events("s").unwrap().is_empty());
        assert_eq!(store.events("t").unwrap().len(), 1);
    }

    #[test]
    fn a_failed_change_leaves_the_others_unwritten() {
        let store = store_with_session();
        // The last write of the transaction fails.
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER injected BEFORE INSERT ON events WHEN NEW.seq = 2
                BEGIN SELECT RAISE(ABORT, 'injected'); END",
            )
            .unwrap();
        store
            .save_session(
                "s",
                &[
                    RowChange::Title(Some("Fix the build".into())),
                    RowChange::Queue(Queue::default()),
                ],
                &[exited("s", 1), exited("s", 2)],
            )
            .unwrap_err();
        assert_eq!(store.sessions().unwrap()[0].title, None);
        assert_eq!(count(&store, "events"), 0);
    }

    /// Event `seq` of `session`, recorded `seq` milliseconds after the
    /// epoch.
    fn event(session: &str, seq: u64, kind: EventKind) -> Event {
        Event {
            session_id: session.into(),
            seq,
            recorded_at: seq,
            kind,
        }
    }

    fn exited(session: &str, seq: u64) -> Event {
        event(session, seq, EventKind::AgentExited { error: None })
    }

    fn store_with_session() -> Store {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session("s", "w", &command(), Path::new("/work"), None, 0, &[])
            .unwrap();
        store
    }

    /// Stores `events` of session `id`, with no row change.
    fn log(store: &Store, id: &str, events: &[Event]) {
        store.save_session(id, &[], events).unwrap();
    }

    #[test]
    fn events_round_trip_in_order() {
        let store = store_with_session();
        log(&store, "s", &[exited("s", 1), exited("s", 2)]);
        log(&store, "s", &[exited("s", 3)]);
        let seqs: Vec<_> = store
            .events("s")
            .unwrap()
            .into_iter()
            .map(|event| (event.seq, event.recorded_at))
            .collect();
        assert_eq!(seqs, [(1, 1), (2, 2), (3, 3)]);
        assert_eq!(store.sessions().unwrap()[0].last_seq, 3);
        assert_eq!(store.events("s").unwrap()[0], exited("s", 1));
    }

    #[test]
    fn a_history_longer_than_a_read_batch_is_read_whole() {
        let store = store_with_session();
        let count = READ_BATCH * 2 + 1;
        let events: Vec<_> = (1..=count).map(|seq| exited("s", seq)).collect();
        log(&store, "s", &events);
        assert_eq!(store.events("s").unwrap(), events);
    }

    #[test]
    fn events_after_skips_a_gap_and_stops_at_the_limit() {
        let store = store_with_session();
        log(
            &store,
            "s",
            &[exited("s", 1), exited("s", 3), exited("s", 4)],
        );
        let seqs = |after, limit| -> Vec<u64> {
            store
                .events_after("s", after, limit)
                .unwrap()
                .into_iter()
                .map(|event| event.seq)
                .collect()
        };
        assert_eq!(seqs(1, 10), [3, 4]);
        assert_eq!(seqs(2, 10), [3, 4]);
        assert_eq!(seqs(0, 2), [1, 3]);
        assert!(seqs(4, 10).is_empty());
    }

    #[test]
    fn events_of_a_deleted_session_are_dropped() {
        let store = store_with_session();
        store.remove_session("s").unwrap();
        log(&store, "s", &[exited("s", 1)]);
        assert!(store.events("s").unwrap().is_empty());
    }

    fn prompt_started() -> EventKind {
        EventKind::PromptStarted {
            prompt: vec![ContentBlock::Text(TextContent::new("Fix the build"))],
            from: None,
        }
    }

    fn turn_ended() -> EventKind {
        EventKind::TurnEnded {
            stop_reason: None,
            error: None,
        }
    }

    fn requested(request_id: &str) -> EventKind {
        EventKind::PermissionRequested {
            request_id: request_id.into(),
            title: "Tool".into(),
            description: None,
            subject: None,
            options: Vec::new(),
            session_request: None,
        }
    }

    fn elicited(request_id: &str) -> EventKind {
        serde_json::from_value(serde_json::json!({
            "type": "elicitation_requested",
            "data": {
                "request_id": request_id,
                "message": "Order",
                "tool_call_id": null,
                "requested_schema": {"type": "object", "properties": {}}
            }
        }))
        .unwrap()
    }

    fn steered() -> EventKind {
        EventKind::PromptSteered {
            prompt: vec![ContentBlock::Text(TextContent::new("Also"))],
            from: None,
        }
    }

    /// Saves session `id` with `kinds` as its events, from seq 1.
    fn session_with(store: &Store, id: &str, kinds: Vec<EventKind>) {
        store
            .add_session(id, "w", &command(), Path::new("/work"), None, 0, &[])
            .unwrap();
        let events: Vec<_> = kinds
            .into_iter()
            .zip(1..)
            .map(|(kind, seq)| event(id, seq, kind))
            .collect();
        log(store, id, &events);
    }

    #[test]
    fn left_open_finds_what_a_stop_left_open_and_nothing_else() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        session_with(
            &store,
            "open",
            vec![
                prompt_started(),
                requested("1"),
                EventKind::PermissionResolved {
                    request_id: "1".into(),
                    outcome: agent_client_protocol::v2::schema::CancelledPermissionOutcome::new()
                        .into(),
                },
                requested("4"),
                elicited("5"),
                steered(),
                steered(),
                EventKind::SteeredPromptTaken { prompt: 6 },
            ],
        );
        session_with(
            &store,
            "agent turn",
            vec![prompt_started(), turn_ended(), EventKind::TurnStarted],
        );
        session_with(
            &store,
            "finished",
            vec![
                prompt_started(),
                steered(),
                EventKind::SteeredPromptDropped { prompt: 2 },
                turn_ended(),
                EventKind::AgentParked,
            ],
        );
        session_with(&store, "empty", Vec::new());
        let open = store.left_open().unwrap();
        assert_eq!(
            open,
            HashMap::from([
                (
                    "open".into(),
                    LeftOpen {
                        turn: true,
                        requests: vec![("4".into(), true), ("5".into(), false)],
                        steered: vec![7],
                    }
                ),
                (
                    "agent turn".into(),
                    LeftOpen {
                        turn: true,
                        ..LeftOpen::default()
                    }
                ),
            ])
        );
    }

    #[test]
    fn a_session_needs_replay_until_its_events_hold_a_start_or_a_prompt() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        session_with(&store, "none", vec![EventKind::AgentParked]);
        session_with(&store, "prompted", vec![prompt_started()]);
        session_with(
            &store,
            "started",
            vec![EventKind::AgentStarted {
                agent_session_id: "agent".into(),
                prompt_capabilities: ox_protocol::schema::PromptCapabilities::default(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            }],
        );
        let replays: Vec<_> = store
            .sessions()
            .unwrap()
            .into_iter()
            .map(|session| (session.id, session.needs_replay))
            .collect();
        assert_eq!(
            replays,
            [
                ("none".into(), true),
                ("prompted".into(), false),
                ("started".into(), false)
            ]
        );
    }

    /// Startup reads only the index entries of the event types each query
    /// names, never every event.
    #[test]
    fn the_startup_queries_search_events_by_type() {
        let store = store();
        let connection = store.lock();
        for query in [
            SESSIONS,
            LEFT_OPEN_TURNS,
            LEFT_OPEN_REQUESTS,
            LEFT_OPEN_STEERED,
        ] {
            let plan: Vec<String> = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
                .unwrap()
                .query_map([], |row| row.get(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert!(
                plan.iter().any(|step| step.contains("events_by_type")),
                "{query}: {plan:?}"
            );
            assert!(
                !plan.iter().any(|step| step.starts_with("SCAN events")),
                "{query}: {plan:?}"
            );
        }
    }

    #[test]
    fn an_imported_session_round_trips_as_archived_with_its_events() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        let session = SavedSession {
            number: 0,
            id: "s".into(),
            workspace_id: "w".into(),
            command: command(),
            cwd: "/work".into(),
            title: Some("Build fix".into()),
            agent_session_id: Some("claude".into()),
            worktree_id: None,
            last_seq: 0,
            archived: true,
            created_at: 1_700_000_000_000,
            unseen: None,
            queue: Queue::default(),
            config: vec![setting("model", "smart")],
            needs_replay: true,
        };
        let events = [exited("s", 1), exited("s", 2)];
        let number = store.import_session(&session, &events).unwrap();
        assert_eq!(
            store.sessions().unwrap(),
            [SavedSession {
                number,
                last_seq: 2,
                ..session
            }]
        );
        assert_eq!(store.events("s").unwrap(), events);
    }

    #[test]
    fn a_deleted_session_leaves_its_agent_session_id_remembered() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session("one", "w", &command(), Path::new("/work"), None, 0, &[])
            .unwrap();
        store
            .add_session("two", "w", &command(), Path::new("/work"), None, 0, &[])
            .unwrap();
        store
            .save_session("one", &[RowChange::AgentSessionId("claude".into())], &[])
            .unwrap();
        assert!(!store.was_deleted("claude").unwrap());
        store.remove_session("one").unwrap();
        assert!(store.was_deleted("claude").unwrap());
        // A session without an agent session ID adds nothing.
        store.remove_session("two").unwrap();
        assert_eq!(count(&store, "deleted_agent_sessions"), 1);
    }

    fn user_version(connection: &Connection) -> i64 {
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    /// An in-memory database loaded from a fixture's SQL.
    fn database(sql: &str) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(sql).unwrap();
        connection
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn a_database_from_before_versions_opens_at_the_latest_version_with_its_rows() {
        let sql = std::fs::read_to_string(fixtures().join("schema-1.sql")).unwrap();
        let connection = database(&sql);
        connection.pragma_update(None, "user_version", 0).unwrap();
        let store = Store::with(connection).unwrap();
        assert_eq!(user_version(&store.lock()), schema_version());
        assert_eq!(store.workspaces().unwrap().len(), 1);
        assert_eq!(store.sessions().unwrap().len(), 2);
        assert_eq!(count(&store, "events"), 34);
    }

    /// The `plain` session's last option report is a `config_option_update`
    /// with only its model; `archived` never started.
    #[test]
    fn a_schema_1_database_gives_each_session_the_config_it_last_reported() {
        let sql = std::fs::read_to_string(fixtures().join("schema-1.sql")).unwrap();
        let store = Store::with(database(&sql)).unwrap();
        let configs: Vec<_> = store
            .sessions()
            .unwrap()
            .into_iter()
            .map(|session| (session.id, session.config))
            .collect();
        assert_eq!(
            configs,
            [
                ("plain".into(), vec![setting("model", "claude-sonnet-5-5")]),
                ("archived".into(), Vec::new()),
            ]
        );
    }

    #[test]
    fn a_database_of_a_newer_schema_is_refused() {
        let connection = Connection::open_in_memory().unwrap();
        let newer = schema_version() + 1;
        connection
            .pragma_update(None, "user_version", newer)
            .unwrap();
        let Err(error) = Store::with(connection) else {
            panic!("a newer database opened");
        };
        assert_eq!(
            error.to_string(),
            format!(
                "the database is schema version {newer}; this oxd reads up to {}",
                schema_version()
            )
        );
    }

    /// What a fixture holds.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Contents {
        workspaces: usize,
        worktrees: usize,
        sessions: usize,
        archived: usize,
        unseen: usize,
        queued: usize,
        events: usize,
        deleted_agent_sessions: usize,
        /// Sessions with a config.
        configured: usize,
    }

    /// Every fixture, each a database of a schema version some build wrote,
    /// opens with this build and reads back whole.
    #[test]
    fn every_schema_fixture_opens_and_reads_back() {
        let contents = Contents {
            workspaces: 1,
            worktrees: 1,
            sessions: 2,
            archived: 1,
            unseen: 1,
            queued: 1,
            events: 34,
            deleted_agent_sessions: 1,
            configured: 1,
        };
        let expected = [
            ("schema-1.sql", contents),
            ("schema-2.sql", contents),
            ("schema-3.sql", contents),
        ];
        let mut names: Vec<String> = std::fs::read_dir(fixtures())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with("schema-"))
            .collect();
        names.sort();
        let listed: Vec<_> = expected.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, listed, "each fixture has its expected contents");

        for (name, contents) in expected {
            let sql = std::fs::read_to_string(fixtures().join(name)).unwrap();
            let store =
                Store::with(database(&sql)).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(user_version(&store.lock()), schema_version(), "{name}");
            let sessions = store.sessions().unwrap();
            let mut events = 0;
            for session in &sessions {
                let read = store
                    .events(&session.id)
                    .unwrap_or_else(|error| panic!("{name}: {}: {error}", session.id));
                assert_eq!(
                    read.last().map_or(0, |event| event.seq),
                    session.last_seq,
                    "{name}: {}",
                    session.id
                );
                events += read.len();
            }
            let read = Contents {
                workspaces: store.workspaces().unwrap().len(),
                worktrees: store.worktrees().unwrap().len(),
                sessions: sessions.len(),
                archived: sessions.iter().filter(|session| session.archived).count(),
                unseen: sessions
                    .iter()
                    .filter(|session| session.unseen.is_some())
                    .count(),
                queued: sessions
                    .iter()
                    .map(|session| session.queue.prompts.len())
                    .sum(),
                events,
                deleted_agent_sessions: usize::try_from(count(&store, "deleted_agent_sessions"))
                    .unwrap(),
                configured: sessions
                    .iter()
                    .filter(|session| !session.config.is_empty())
                    .count(),
            };
            assert_eq!(read, contents, "{name}");
        }
    }
}
