//! The daemon's saved workspaces, worktrees, sessions and workflow runs, in
//! one `SQLite` database.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use ox_protocol::{CommandInfo, CommandState, Event, EventKind, RunState, StepInfo, StepState};
use rusqlite::{Connection, params};
use tokio::sync::oneshot;

/// A workspace as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedWorkspace {
    pub id: String,
    pub path: PathBuf,
}

/// A session as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// A workflow run as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedRun {
    pub number: u64,
    pub id: String,
    pub workspace_id: String,
    /// The worktree the run owns.
    pub worktree_id: String,
    /// The file name the run was started with.
    pub workflow: String,
    /// The definition's name.
    pub name: String,
    pub input: String,
    pub state: RunState,
    pub error: Option<String>,
    /// The steps, in order.
    pub steps: Vec<StepInfo>,
}

/// The worktree a new session runs in.
#[derive(Clone, Copy)]
pub enum SessionWorktree<'a> {
    /// The workspace's directory.
    None,
    /// A worktree the session owns, saved with it.
    New(&'a SavedWorktree),
    /// The worktree of run `run_id`, whose step `position` the session runs.
    Step {
        run_id: &'a str,
        position: usize,
        worktree_id: &'a str,
    },
}

/// What the event writer thread is sent.
enum Write {
    Events(Vec<Event>),
    /// Acknowledged once everything sent before it is committed.
    Flush(oneshot::Sender<()>),
}

/// One connection to the database. Every query is a small local read or
/// write, so it runs inline. Events are written by a thread of their own, so
/// recording one never waits on the database.
pub struct Store {
    connection: Arc<Mutex<Connection>>,
    writer: Sender<Write>,
}

impl Store {
    /// Opens the database at `path`, creating it and its tables if they do
    /// not exist. The schema has no migrations: a schema change means
    /// deleting the database.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::with(Connection::open(path)?)
    }

    fn with(connection: Connection) -> rusqlite::Result<Self> {
        connection.execute_batch(
            "PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA auto_vacuum = FULL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS workspaces (
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
                worktree_id      TEXT REFERENCES worktrees(id) ON DELETE SET NULL
            );
            CREATE TABLE IF NOT EXISTS runs (
                number       INTEGER PRIMARY KEY AUTOINCREMENT,
                id           TEXT NOT NULL UNIQUE,
                workspace_id TEXT NOT NULL REFERENCES workspaces(id),
                worktree_id  TEXT NOT NULL REFERENCES worktrees(id),
                workflow     TEXT NOT NULL,
                name         TEXT NOT NULL,
                input        TEXT NOT NULL,
                state        TEXT NOT NULL,
                error        TEXT
            );
            CREATE TABLE IF NOT EXISTS run_steps (
                run_id     TEXT    NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
                position   INTEGER NOT NULL,
                name       TEXT    NOT NULL,
                state      TEXT    NOT NULL,
                session_id TEXT    REFERENCES sessions(id) ON DELETE SET NULL,
                output     TEXT,
                PRIMARY KEY (run_id, position)
            );
            CREATE TABLE IF NOT EXISTS run_commands (
                run_id    TEXT    NOT NULL,
                step      INTEGER NOT NULL,
                position  INTEGER NOT NULL,
                kind      TEXT    NOT NULL,
                command   TEXT    NOT NULL,
                state     TEXT    NOT NULL,
                exit_code INTEGER,
                output    TEXT    NOT NULL,
                truncated INTEGER NOT NULL,
                PRIMARY KEY (run_id, step, position),
                FOREIGN KEY (run_id, step) REFERENCES run_steps(run_id, position)
                    ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS events (
                session_id TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                seq        INTEGER NOT NULL,
                kind       TEXT    NOT NULL,  -- JSON of EventKind
                PRIMARY KEY (session_id, seq)
            ) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS archived (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE
            );",
        )?;
        let connection = Arc::new(Mutex::new(connection));
        let (writer, queue) = mpsc::channel();
        let target = connection.clone();
        std::thread::spawn(move || write_events(&target, &queue));
        Ok(Self { connection, writer })
    }

    /// Queues events for writing and returns at once. Events of a session
    /// deleted before they are written are dropped.
    pub fn log(&self, events: Vec<Event>) {
        let _ = self.writer.send(Write::Events(events));
    }

    /// Returns once every event logged before the call is committed.
    pub async fn flush(&self) {
        let (done, committed) = oneshot::channel();
        if self.writer.send(Write::Flush(done)).is_ok() {
            let _ = committed.await;
        }
    }

    /// A session's stored events, in order.
    pub fn events(&self, session_id: &str) -> rusqlite::Result<Vec<Event>> {
        let connection = self.lock();
        let mut query = connection
            .prepare("SELECT seq, kind FROM events WHERE session_id = ?1 ORDER BY seq")?;
        query
            .query_map([session_id], |row| {
                let kind: String = row.get(1)?;
                Ok(Event {
                    session_id: session_id.into(),
                    seq: u64::try_from(row.get::<_, i64>(0)?).expect("seqs are positive"),
                    kind: serde_json::from_str::<EventKind>(&kind).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                })
            })?
            .collect()
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

    pub fn remove_workspace(&self, id: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
        Ok(())
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
        let mut query = connection.prepare(
            "SELECT number, id, workspace_id, command, cwd, title, agent_session_id, worktree_id,
                (SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = sessions.id),
                EXISTS (SELECT 1 FROM archived WHERE session_id = sessions.id)
            FROM sessions
            ORDER BY number",
        )?;
        query
            .query_map([], |row| {
                let command: String = row.get(3)?;
                Ok(SavedSession {
                    number: u64::try_from(row.get::<_, i64>(0)?).expect("rowids are positive"),
                    id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    command: serde_json::from_str(&command).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    cwd: PathBuf::from(row.get::<_, String>(4)?),
                    title: row.get(5)?,
                    agent_session_id: row.get(6)?,
                    worktree_id: row.get(7)?,
                    last_seq: u64::try_from(row.get::<_, i64>(8)?).expect("seqs are positive"),
                    archived: row.get(9)?,
                })
            })?
            .collect()
    }

    /// Saves a session that has not started, with the worktree it runs in,
    /// and returns its number, which is greater than the number of every
    /// session saved before it. A step's session is recorded as its step's.
    pub fn add_session(
        &self,
        id: &str,
        workspace_id: &str,
        command: &[String],
        cwd: &Path,
        worktree: SessionWorktree<'_>,
    ) -> rusqlite::Result<u64> {
        let command = serde_json::to_string(command).expect("a list of strings serializes");
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let worktree_id = match worktree {
            SessionWorktree::None => None,
            SessionWorktree::New(worktree) => {
                insert_worktree(&transaction, worktree)?;
                Some(worktree.id.as_str())
            }
            SessionWorktree::Step { worktree_id, .. } => Some(worktree_id),
        };
        transaction.execute(
            "INSERT INTO sessions (id, workspace_id, command, cwd, worktree_id)
            VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                id,
                workspace_id,
                command,
                cwd.to_string_lossy(),
                worktree_id
            ],
        )?;
        // `number` is the table's rowid.
        let number = u64::try_from(transaction.last_insert_rowid()).expect("rowids are positive");
        if let SessionWorktree::Step {
            run_id, position, ..
        } = worktree
        {
            transaction.execute(
                "UPDATE run_steps SET session_id = ?3 WHERE run_id = ?1 AND position = ?2",
                params![run_id, position_value(position), id],
            )?;
        }
        transaction.commit()?;
        Ok(number)
    }

    /// Saves a run with its worktree and its steps, and returns its number,
    /// which is greater than the number of every run saved before it.
    pub fn add_run(&self, run: &SavedRun, worktree: &SavedWorktree) -> rusqlite::Result<u64> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        insert_worktree(&transaction, worktree)?;
        transaction.execute(
            "INSERT INTO runs (id, workspace_id, worktree_id, workflow, name, input, state, error)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run.id,
                run.workspace_id,
                run.worktree_id,
                run.workflow,
                run.name,
                run.input,
                state_name(run.state),
                run.error
            ],
        )?;
        let number = u64::try_from(transaction.last_insert_rowid()).expect("rowids are positive");
        for (position, step) in run.steps.iter().enumerate() {
            transaction.execute(
                "INSERT INTO run_steps (run_id, position, name, state, session_id, output)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    run.id,
                    position_value(position),
                    step.name,
                    state_name(step.state),
                    step.session_id,
                    step.output
                ],
            )?;
        }
        transaction.commit()?;
        Ok(number)
    }

    /// Every run with its steps, in the order they were started.
    pub fn runs(&self) -> rusqlite::Result<Vec<SavedRun>> {
        let connection = self.lock();
        let mut query = connection.prepare(
            "SELECT number, id, workspace_id, worktree_id, workflow, name, input, state, error
            FROM runs ORDER BY number",
        )?;
        let mut runs = query
            .query_map([], |row| {
                Ok(SavedRun {
                    number: u64::try_from(row.get::<_, i64>(0)?).expect("rowids are positive"),
                    id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    worktree_id: row.get(3)?,
                    workflow: row.get(4)?,
                    name: row.get(5)?,
                    input: row.get(6)?,
                    state: parse_state(row, 7)?,
                    error: row.get(8)?,
                    steps: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut steps = connection.prepare(
            "SELECT name, state, session_id, output FROM run_steps
            WHERE run_id = ?1 ORDER BY position",
        )?;
        let mut commands = connection.prepare(
            "SELECT kind, command, state, exit_code, output, truncated FROM run_commands
            WHERE run_id = ?1 AND step = ?2 ORDER BY position",
        )?;
        for run in &mut runs {
            run.steps = steps
                .query_map([&run.id], |row| {
                    Ok(StepInfo {
                        name: row.get(0)?,
                        state: parse_state(row, 1)?,
                        session_id: row.get(2)?,
                        output: row.get(3)?,
                        commands: Vec::new(),
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            for (position, step) in run.steps.iter_mut().enumerate() {
                step.commands = commands
                    .query_map(params![run.id, position_value(position)], |row| {
                        Ok(CommandInfo {
                            kind: parse_state(row, 0)?,
                            command: row.get(1)?,
                            state: parse_state(row, 2)?,
                            exit_code: row.get(3)?,
                            output: row.get(4)?,
                            truncated: row.get(5)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
            }
        }
        Ok(runs)
    }

    /// Saves command `position` of step `step` of run `run_id`, replacing
    /// what was saved for it.
    pub fn save_command(
        &self,
        run_id: &str,
        step: usize,
        position: usize,
        command: &CommandInfo,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO run_commands
                (run_id, step, position, kind, command, state, exit_code, output, truncated)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run_id,
                position_value(step),
                position_value(position),
                state_name(command.kind),
                command.command,
                state_name(command.state),
                command.exit_code,
                command.output,
                command.truncated
            ],
        )?;
        Ok(())
    }

    pub fn set_run_state(
        &self,
        id: &str,
        state: RunState,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE runs SET state = ?2, error = ?3 WHERE id = ?1",
            params![id, state_name(state), error],
        )?;
        Ok(())
    }

    pub fn set_step(
        &self,
        run_id: &str,
        position: usize,
        state: StepState,
        output: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE run_steps SET state = ?3, output = ?4 WHERE run_id = ?1 AND position = ?2",
            params![run_id, position_value(position), state_name(state), output],
        )?;
        Ok(())
    }

    /// Marks the runs, steps, and commands that were running `interrupted`.
    pub fn interrupt_runs(&self) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE runs SET state = ?2 WHERE state = ?1",
            [
                state_name(RunState::Running),
                state_name(RunState::Interrupted),
            ],
        )?;
        transaction.execute(
            "UPDATE run_steps SET state = ?2 WHERE state = ?1",
            [
                state_name(StepState::Running),
                state_name(StepState::Interrupted),
            ],
        )?;
        transaction.execute(
            "UPDATE run_commands SET state = ?2 WHERE state = ?1",
            [
                state_name(CommandState::Running),
                state_name(CommandState::Interrupted),
            ],
        )?;
        transaction.commit()
    }

    /// Deletes a run, with its steps and their commands, its child sessions
    /// and their events, and its worktree.
    pub fn remove_run(&self, id: &str) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM sessions
            WHERE id IN (SELECT session_id FROM run_steps WHERE run_id = ?1)",
            [id],
        )?;
        let worktree_id: String =
            transaction.query_row("SELECT worktree_id FROM runs WHERE id = ?1", [id], |row| {
                row.get(0)
            })?;
        transaction.execute("DELETE FROM runs WHERE id = ?1", [id])?;
        transaction.execute("DELETE FROM worktrees WHERE id = ?1", [worktree_id])?;
        transaction.commit()
    }

    /// Sets or clears a worktree's comparison ref.
    pub fn set_compare(&self, worktree_id: &str, compare: Option<&str>) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE worktrees SET compare = ?2 WHERE id = ?1",
            params![worktree_id, compare],
        )?;
        Ok(())
    }

    /// Forgets a worktree. The session that ran in it is kept, with no
    /// worktree.
    pub fn remove_worktree(&self, worktree_id: &str) -> rusqlite::Result<()> {
        self.lock()
            .execute("DELETE FROM worktrees WHERE id = ?1", [worktree_id])?;
        Ok(())
    }

    /// Whether any session or run belongs to the workspace.
    pub fn workspace_has_sessions(&self, workspace_id: &str) -> rusqlite::Result<bool> {
        self.lock().query_row(
            "SELECT EXISTS (SELECT 1 FROM sessions WHERE workspace_id = ?1)
                OR EXISTS (SELECT 1 FROM runs WHERE workspace_id = ?1)",
            [workspace_id],
            |row| row.get(0),
        )
    }

    pub fn set_title(&self, id: &str, title: Option<&str>) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE sessions SET title = ?2 WHERE id = ?1",
            params![id, title],
        )?;
        Ok(())
    }

    pub fn set_agent_session_id(&self, id: &str, agent_session_id: &str) -> rusqlite::Result<()> {
        self.lock().execute(
            "UPDATE sessions SET agent_session_id = ?2 WHERE id = ?1",
            params![id, agent_session_id],
        )?;
        Ok(())
    }

    /// Marks a session archived, or not.
    pub fn set_archived(&self, id: &str, archived: bool) -> rusqlite::Result<()> {
        let sql = if archived {
            "INSERT OR IGNORE INTO archived (session_id) VALUES (?1)"
        } else {
            "DELETE FROM archived WHERE session_id = ?1"
        };
        self.lock().execute(sql, [id])?;
        Ok(())
    }

    /// Deletes a session, with its events and the worktree it runs in unless
    /// a run owns the worktree.
    pub fn remove_session(&self, id: &str) -> rusqlite::Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM worktrees
            WHERE id = (SELECT worktree_id FROM sessions WHERE id = ?1)
                AND NOT EXISTS (SELECT 1 FROM runs WHERE worktree_id = worktrees.id)",
            [id],
        )?;
        transaction.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        transaction.commit()
    }
}

fn insert_worktree(
    transaction: &rusqlite::Transaction<'_>,
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

fn position_value(position: usize) -> i64 {
    i64::try_from(position).expect("step positions fit an i64")
}

/// A run's, step's, or command's state, or a command's kind, as its column
/// holds it: its name on the wire.
fn state_name(state: impl serde::Serialize) -> String {
    match serde_json::to_value(state).expect("a state serializes") {
        serde_json::Value::String(name) => name,
        value => unreachable!("a state serializes as a string, not {value}"),
    }
}

/// The state or kind in column `index`.
fn parse_state<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let name: String = row.get(index)?;
    serde_json::from_value(serde_json::Value::String(name)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

/// Writes each batch the queue holds in one transaction, until every sender
/// is dropped. A failed write goes to oxd's log and the thread carries on.
fn write_events(connection: &Mutex<Connection>, queue: &Receiver<Write>) {
    while let Ok(first) = queue.recv() {
        let mut batch = vec![first];
        batch.extend(queue.try_iter());
        let mut flushes = Vec::new();
        let mut connection = connection.lock().unwrap();
        let result = (|| {
            let transaction = connection.transaction()?;
            {
                let mut insert = transaction.prepare_cached(
                    "INSERT INTO events SELECT ?1, ?2, ?3
                    WHERE EXISTS (SELECT 1 FROM sessions WHERE id = ?1)",
                )?;
                for write in &batch {
                    if let Write::Events(events) = write {
                        for event in events {
                            let seq = i64::try_from(event.seq).expect("seqs fit an i64");
                            let kind = serde_json::to_string(&event.kind)
                                .expect("an event kind serializes");
                            insert.execute(params![event.session_id, seq, kind])?;
                        }
                    }
                }
            }
            transaction.commit()
        })();
        drop(connection);
        if let Err(error) = result {
            eprintln!("oxd: writing events: {error}");
        }
        for write in batch {
            if let Write::Flush(done) = write {
                flushes.push(done);
            }
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use ox_protocol::CommandKind;

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
    fn a_workspace_path_is_saved_once() {
        let store = store();
        assert!(store.add_workspace(&workspace("a", "/work")).unwrap());
        assert!(!store.add_workspace(&workspace("b", "/work")).unwrap());
        assert_eq!(store.workspaces().unwrap(), [workspace("a", "/work")]);
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
                SessionWorktree::None,
            )
            .unwrap();
        let second = store
            .add_session(
                "two",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
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
                },
            ]
        );
    }

    #[test]
    fn archiving_is_saved_and_deleted_with_the_session() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session(
                "one",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        assert!(!store.sessions().unwrap()[0].archived);
        store.set_archived("one", true).unwrap();
        assert!(store.sessions().unwrap()[0].archived);
        store.set_archived("one", false).unwrap();
        assert!(!store.sessions().unwrap()[0].archived);
        store.set_archived("one", true).unwrap();
        store.remove_session("one").unwrap();
        assert_eq!(count(&store, "archived"), 0);
    }

    #[test]
    fn title_and_agent_session_id_updates_are_saved() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session(
                "s",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        store.set_title("s", Some("Fix the build")).unwrap();
        store.set_agent_session_id("s", "agent").unwrap();
        let saved = &store.sessions().unwrap()[0];
        assert_eq!(saved.title.as_deref(), Some("Fix the build"));
        assert_eq!(saved.agent_session_id.as_deref(), Some("agent"));

        store.set_title("s", None).unwrap();
        assert_eq!(store.sessions().unwrap()[0].title, None);
    }

    #[test]
    fn a_deleted_session_is_gone_and_its_number_is_not_reused() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        let first = store
            .add_session(
                "one",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        store.remove_session("one").unwrap();
        assert!(store.sessions().unwrap().is_empty());
        let second = store
            .add_session(
                "two",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        assert!(second > first);
    }

    #[test]
    fn a_workspace_with_sessions_cannot_be_deleted() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session(
                "s",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        assert!(store.remove_workspace("w").is_err());
        store.remove_session("s").unwrap();
        store.remove_workspace("w").unwrap();
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
            .add_session(
                id,
                "w",
                &command(),
                &worktree.path,
                SessionWorktree::New(worktree),
            )
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

    /// The worktree, with its comparison ref and repository, comes back as
    /// saved, and the session names it.
    #[test]
    fn a_session_round_trips_with_its_worktree() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store
            .add_session(
                "plain",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        let mut compared = worktree("01234567");
        compared.compare = Some("main".into());
        add_worktree_session(&store, "s", &compared);
        assert_eq!(store.worktrees().unwrap(), [compared]);
        let sessions = store.sessions().unwrap();
        assert_eq!(sessions[0].worktree_id, None);
        assert_eq!(sessions[1].worktree_id.as_deref(), Some("01234567"));
        assert_eq!(sessions[1].cwd, Path::new("/worktrees/work-01234567"));
    }

    #[test]
    fn a_comparison_ref_is_replaced_and_cleared_by_worktree() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_worktree_session(&store, "s", &worktree("one"));
        add_worktree_session(&store, "t", &worktree("two"));
        store.set_compare("one", Some("origin/main")).unwrap();
        let mut compared = worktree("one");
        compared.compare = Some("origin/main".into());
        assert_eq!(store.worktrees().unwrap(), [compared, worktree("two")]);
        store.set_compare("one", None).unwrap();
        assert_eq!(
            store.worktrees().unwrap(),
            [worktree("one"), worktree("two")]
        );
    }

    #[test]
    fn a_removed_worktree_leaves_its_session_without_one() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_worktree_session(&store, "s", &worktree("one"));
        store.remove_worktree("one").unwrap();
        assert!(store.worktrees().unwrap().is_empty());
        assert_eq!(store.sessions().unwrap()[0].worktree_id, None);
    }

    #[tokio::test]
    async fn a_deleted_session_takes_its_worktree_and_events() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_worktree_session(&store, "s", &worktree("one"));
        add_worktree_session(&store, "t", &worktree("two"));
        store.log(vec![exited("s", 1), exited("t", 1)]);
        store.flush().await;
        store.remove_session("s").unwrap();
        assert_eq!(store.worktrees().unwrap(), [worktree("two")]);
        assert!(store.events("s").unwrap().is_empty());
        assert_eq!(store.events("t").unwrap().len(), 1);
    }

    #[test]
    fn a_session_whose_worktree_row_fails_is_not_saved() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_worktree_session(&store, "s", &worktree("one"));
        // The second worktree row conflicts with the first.
        store
            .add_session(
                "t",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::New(&worktree("one")),
            )
            .unwrap_err();
        assert_eq!(count(&store, "sessions"), 1);
        assert_eq!(count(&store, "worktrees"), 1);
    }

    #[test]
    fn whether_a_workspace_has_sessions() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        store.add_workspace(&workspace("v", "/other")).unwrap();
        assert!(!store.workspace_has_sessions("w").unwrap());
        store
            .add_session(
                "s",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        assert!(store.workspace_has_sessions("w").unwrap());
        assert!(!store.workspace_has_sessions("v").unwrap());
    }

    fn step(name: &str, state: StepState) -> StepInfo {
        StepInfo {
            name: name.into(),
            state,
            session_id: None,
            output: None,
            commands: Vec::new(),
        }
    }

    fn command_info(kind: CommandKind, command: &str, state: CommandState) -> CommandInfo {
        CommandInfo {
            kind,
            command: command.into(),
            state,
            exit_code: None,
            output: String::new(),
            truncated: false,
        }
    }

    /// A running run `id` of two pending steps in worktree `id`.
    fn run(id: &str) -> SavedRun {
        SavedRun {
            number: 0,
            id: id.into(),
            workspace_id: "w".into(),
            worktree_id: id.into(),
            workflow: "plan-work".into(),
            name: "Plan and work".into(),
            input: "Fix the build".into(),
            state: RunState::Running,
            error: None,
            steps: vec![
                step("Plan", StepState::Pending),
                step("Work", StepState::Pending),
            ],
        }
    }

    /// Saves run `id` and returns it as saved.
    fn add_run(store: &Store, id: &str) -> SavedRun {
        let mut run = run(id);
        run.number = store.add_run(&run, &worktree(id)).unwrap();
        run
    }

    /// Saves the session of step `position` of run `run_id`.
    fn add_step_session(store: &Store, id: &str, run_id: &str, position: usize) {
        store
            .add_session(
                id,
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::Step {
                    run_id,
                    position,
                    worktree_id: run_id,
                },
            )
            .unwrap();
    }

    #[test]
    fn a_run_round_trips_with_its_steps() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        assert!(store.runs().unwrap().is_empty());
        let first = add_run(&store, "one");
        let mut second = add_run(&store, "two");
        assert!(first.number < second.number);
        add_step_session(&store, "s", "two", 0);
        store
            .set_step("two", 0, StepState::Succeeded, Some("planned"))
            .unwrap();
        store.set_step("two", 1, StepState::Failed, None).unwrap();
        store
            .set_run_state("two", RunState::Failed, Some("boom"))
            .unwrap();
        second.state = RunState::Failed;
        second.error = Some("boom".into());
        second.steps = vec![
            StepInfo {
                name: "Plan".into(),
                state: StepState::Succeeded,
                session_id: Some("s".into()),
                output: Some("planned".into()),
                commands: Vec::new(),
            },
            step("Work", StepState::Failed),
        ];
        assert_eq!(store.runs().unwrap(), [first, second]);
        assert_eq!(
            store.worktrees().unwrap(),
            [worktree("one"), worktree("two")]
        );
        assert_eq!(
            store.sessions().unwrap()[0].worktree_id.as_deref(),
            Some("two")
        );
    }

    #[test]
    fn a_run_s_commands_round_trip_in_order() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        let mut run = add_run(&store, "r");
        let precondition = command_info(CommandKind::Precondition, "true", CommandState::Running);
        let failed = CommandInfo {
            exit_code: Some(1),
            output: "...dirty".into(),
            truncated: true,
            ..command_info(
                CommandKind::Postcondition,
                "test -z dirty",
                CommandState::Failed,
            )
        };
        let shell = command_info(CommandKind::Shell, "make", CommandState::Running);
        store.save_command("r", 0, 0, &precondition).unwrap();
        store.save_command("r", 0, 1, &failed).unwrap();
        store.save_command("r", 1, 0, &shell).unwrap();
        let precondition = CommandInfo {
            state: CommandState::Succeeded,
            exit_code: Some(0),
            ..precondition
        };
        store.save_command("r", 0, 0, &precondition).unwrap();
        run.steps[0].commands = vec![precondition, failed];
        run.steps[1].commands = vec![shell];
        assert_eq!(store.runs().unwrap(), [run]);
    }

    #[test]
    fn removing_a_child_session_keeps_the_run_worktree() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_run(&store, "r");
        add_step_session(&store, "s", "r", 0);
        store.remove_session("s").unwrap();
        assert_eq!(store.worktrees().unwrap(), [worktree("r")]);
        assert_eq!(store.runs().unwrap()[0].steps[0].session_id, None);
    }

    #[tokio::test]
    async fn removing_a_run_takes_its_sessions_steps_and_worktree() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_run(&store, "r");
        let other = add_run(&store, "q");
        add_step_session(&store, "s", "r", 0);
        add_step_session(&store, "t", "r", 1);
        add_step_session(&store, "u", "q", 0);
        let make = command_info(CommandKind::Shell, "make", CommandState::Succeeded);
        store.save_command("r", 0, 0, &make).unwrap();
        store.save_command("q", 0, 0, &make).unwrap();
        store.log(vec![exited("s", 1), exited("u", 1)]);
        store.flush().await;
        store.remove_run("r").unwrap();
        let sessions: Vec<_> = store
            .sessions()
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert_eq!(sessions, ["u"]);
        assert!(store.events("s").unwrap().is_empty());
        assert_eq!(store.worktrees().unwrap(), [worktree("q")]);
        assert_eq!(count(&store, "run_steps"), 2);
        assert_eq!(count(&store, "run_commands"), 1);
        assert_eq!(store.runs().unwrap()[0].id, other.id);
    }

    #[test]
    fn interrupting_runs_marks_only_running_runs_steps_and_commands() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_run(&store, "running");
        add_run(&store, "done");
        for (position, state) in [CommandState::Failed, CommandState::Running]
            .into_iter()
            .enumerate()
        {
            store
                .save_command(
                    "running",
                    1,
                    position,
                    &command_info(CommandKind::Postcondition, "true", state),
                )
                .unwrap();
        }
        store
            .set_step("running", 0, StepState::Succeeded, Some("planned"))
            .unwrap();
        store
            .set_step("running", 1, StepState::Running, None)
            .unwrap();
        store
            .set_step("done", 0, StepState::Cancelled, None)
            .unwrap();
        store
            .set_run_state("done", RunState::Cancelled, None)
            .unwrap();
        store.interrupt_runs().unwrap();
        let states: Vec<_> = store
            .runs()
            .unwrap()
            .into_iter()
            .map(|run| {
                let steps: Vec<_> = run.steps.iter().map(|step| step.state).collect();
                (run.state, steps)
            })
            .collect();
        assert_eq!(
            states,
            [
                (
                    RunState::Interrupted,
                    vec![StepState::Succeeded, StepState::Interrupted]
                ),
                (
                    RunState::Cancelled,
                    vec![StepState::Cancelled, StepState::Pending]
                ),
            ]
        );
        let commands: Vec<_> = store.runs().unwrap()[0].steps[1]
            .commands
            .iter()
            .map(|command| command.state)
            .collect();
        assert_eq!(commands, [CommandState::Failed, CommandState::Interrupted]);
    }

    #[test]
    fn a_workspace_with_a_run_has_sessions() {
        let store = store();
        store.add_workspace(&workspace("w", "/work")).unwrap();
        add_run(&store, "r");
        assert!(store.workspace_has_sessions("w").unwrap());
    }

    fn event(session: &str, seq: u64, kind: EventKind) -> Event {
        Event {
            session_id: session.into(),
            seq,
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
            .add_session(
                "s",
                "w",
                &command(),
                Path::new("/work"),
                SessionWorktree::None,
            )
            .unwrap();
        store
    }

    #[tokio::test]
    async fn events_round_trip_in_order_across_batches() {
        let store = store_with_session();
        store.log(vec![exited("s", 1), exited("s", 2)]);
        store.log(vec![exited("s", 3)]);
        store.flush().await;
        let seqs: Vec<_> = store
            .events("s")
            .unwrap()
            .into_iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(seqs, [1, 2, 3]);
        assert_eq!(store.sessions().unwrap()[0].last_seq, 3);
        assert_eq!(store.events("s").unwrap()[0], exited("s", 1));
    }

    #[tokio::test]
    async fn events_of_a_deleted_session_are_dropped() {
        let store = store_with_session();
        store.remove_session("s").unwrap();
        store.log(vec![exited("s", 1)]);
        store.flush().await;
        assert!(store.events("s").unwrap().is_empty());
    }
}
