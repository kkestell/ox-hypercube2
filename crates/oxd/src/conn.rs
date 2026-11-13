use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ox_protocol::{
    ErrorBody, EventKind, HelloResult, Method, PROTOCOL_VERSION, Request, ServerMessage, code,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

use crate::daemon::{ClientGuard, Daemon};
use crate::directories::{self, Changes};
use crate::session::Session;

/// Messages queued for a client before its subscriptions wait for it to
/// read.
const OUTGOING_CAPACITY: usize = 256;

/// How long one write to a client may block before the client is
/// disconnected.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// A task that stops when its handle is dropped.
struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Serves one client until it disconnects or stops reading.
pub async fn serve(daemon: Arc<Daemon>, stream: UnixStream, _client: ClientGuard) {
    let (read, write) = stream.into_split();
    let (outgoing, outgoing_rx) = mpsc::channel(OUTGOING_CAPACITY);
    let mut writer = Task(tokio::spawn(write_loop(write, outgoing_rx)));
    // Event subscriptions, watches, and directory watches, by the id of the
    // request that started each.
    let mut subscriptions: HashMap<u64, Task> = HashMap::new();
    let mut lines = BufReader::new(read).lines();
    let mut greeted = false;

    loop {
        let line = tokio::select! {
            line = lines.next_line() => line,
            _ = &mut writer.0 => break,
        };
        let Ok(Some(line)) = line else { break };
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let Some(id) = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|value| value.get("id").and_then(Value::as_u64))
                else {
                    break;
                };
                let error = ErrorBody {
                    code: code::INVALID_REQUEST,
                    message: error.to_string(),
                };
                if outgoing
                    .send(ServerMessage::Error { id, error })
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        let id = request.id;
        let result = match request.method {
            Method::Hello { protocol_version } if protocol_version == PROTOCOL_VERSION => {
                greeted = true;
                Ok(serde_json::to_value(HelloResult {
                    protocol_version: PROTOCOL_VERSION,
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                })
                .expect("protocol types serialize"))
            }
            Method::Hello { protocol_version } => Err(ErrorBody {
                code: code::PROTOCOL_MISMATCH,
                message: format!(
                    "protocol version {protocol_version} is not supported; oxd speaks {PROTOCOL_VERSION}"
                ),
            }),
            _ if !greeted => Err(ErrorBody {
                code: code::INVALID_REQUEST,
                message: "the first message must be hello".into(),
            }),
            Method::SessionSubscribe {
                session_id,
                after_seq,
            } => match subscribable(&daemon, &session_id, after_seq).await {
                Ok(session) => {
                    let ack = ServerMessage::Result {
                        id,
                        result: Value::Null,
                    };
                    if outgoing.send(ack).await.is_err() {
                        break;
                    }
                    let task = tokio::spawn(forward(session, id, after_seq, outgoing.clone()));
                    subscriptions.insert(id, Task(task));
                    continue;
                }
                Err(error) => Err(error),
            },
            Method::Watch => {
                let ack = ServerMessage::Result {
                    id,
                    result: Value::Null,
                };
                if outgoing.send(ack).await.is_err() {
                    break;
                }
                let task = tokio::spawn(watch(daemon.clone(), id, outgoing.clone()));
                subscriptions.insert(id, Task(task));
                continue;
            }
            Method::DirectoryWatch { path } if !path.is_absolute() => Err(ErrorBody {
                code: code::INVALID_REQUEST,
                message: format!("{} is not an absolute path", path.display()),
            }),
            Method::DirectoryWatch { path } => {
                let ack = ServerMessage::Result {
                    id,
                    result: Value::Null,
                };
                if outgoing.send(ack).await.is_err() {
                    break;
                }
                let task = tokio::spawn(directory(daemon.clone(), path, id, outgoing.clone()));
                subscriptions.insert(id, Task(task));
                continue;
            }
            Method::Unsubscribe { subscription } => {
                subscriptions.remove(&subscription);
                Ok(Value::Null)
            }
            // Setting a config option waits on the agent, and creating a
            // session in a worktree, deleting a session, listing or moving
            // branches and comparing a session's branch wait on git, so each runs in its own task and the connection
            // keeps serving other requests. A client that disconnects does
            // not cancel it.
            method @ (Method::SessionSetConfigOption { .. }
            | Method::SessionNew { .. }
            | Method::SessionDelete { .. }
            | Method::SessionArchive { .. }
            | Method::SessionUnarchive { .. }
            | Method::WorkspaceBranches { .. }
            | Method::BranchMove { .. }
            | Method::SessionSetCompare { .. }
            | Method::SessionRefresh { .. }) => {
                let daemon = daemon.clone();
                let outgoing = outgoing.clone();
                tokio::spawn(async move {
                    let message = match daemon.handle(method).await {
                        Ok(result) => ServerMessage::Result { id, result },
                        Err(error) => ServerMessage::Error { id, error },
                    };
                    let _ = outgoing.send(message).await;
                });
                continue;
            }
            method => daemon.handle(method).await,
        };
        let message = match result {
            Ok(result) => ServerMessage::Result { id, result },
            Err(error) => ServerMessage::Error { id, error },
        };
        if outgoing.send(message).await.is_err() {
            break;
        }
    }
}

/// The session, once its history is loaded, if it has events after
/// `after_seq` to send.
async fn subscribable(
    daemon: &Daemon,
    session_id: &str,
    after_seq: u64,
) -> Result<Arc<Session>, ErrorBody> {
    let session = daemon.session(session_id)?;
    session.history().await?;
    session.subscribe(after_seq)?;
    Ok(session)
}

/// Sends subscription `subscription` the session's events after `seq` as
/// they are recorded, at the pace the client reads them, until the session
/// closes.
async fn forward(
    session: Arc<Session>,
    subscription: u64,
    mut seq: u64,
    outgoing: mpsc::Sender<ServerMessage>,
) {
    // A session with a subscription is not parked.
    let _viewer = session.view();
    loop {
        for event in session.events_after(seq).await {
            seq = event.seq;
            let closed = matches!(event.kind, EventKind::SessionClosed);
            let message = ServerMessage::Event {
                subscription,
                event: Box::new(event),
            };
            if outgoing.send(message).await.is_err() || closed {
                return;
            }
        }
    }
}

/// Sends watch `subscription` the workspaces and sessions lists, then both
/// lists each time either differs from the last ones sent.
async fn watch(daemon: Arc<Daemon>, subscription: u64, outgoing: mpsc::Sender<ServerMessage>) {
    let mut changed = daemon.changed();
    let mut sent = None;
    loop {
        // Marking the change seen before reading the lists means a change
        // made while they are built wakes the watch again.
        changed.borrow_and_update();
        let listing = daemon.list();
        if sent.as_ref() != Some(&listing) {
            let message = ServerMessage::List {
                subscription,
                workspaces: listing.workspaces.clone(),
                sessions: listing.sessions.clone(),
            };
            if outgoing.send(message).await.is_err() {
                return;
            }
            sent = Some(listing);
        }
        if changed.changed().await.is_err() {
            return;
        }
    }
}

/// Sends directory watch `subscription` the entries of the directory at
/// `path`, then the entries each time they differ from the last ones sent.
///
/// The stream watches the directory once it can be read, and otherwise its
/// nearest existing ancestor-or-self, moving the watch each time that
/// changes. The watch is registered before the directory is read, so a change
/// during the read wakes the stream again.
async fn directory(
    daemon: Arc<Daemon>,
    path: PathBuf,
    subscription: u64,
    outgoing: mpsc::Sender<ServerMessage>,
) {
    let watch = || {
        let directories = daemon.directories().clone();
        let path = path.clone();
        async move {
            tokio::task::spawn_blocking(move || directories.watch(&path))
                .await
                .expect("watching does not panic")
        }
    };
    let read = || {
        let path = path.clone();
        async move {
            tokio::task::spawn_blocking(move || directories::read(&path))
                .await
                .expect("reading does not panic")
        }
    };
    let mut changes: Changes = watch().await;
    let mut sent = None;
    loop {
        changes.mark_seen();
        let entries = if changes.is_target() {
            read().await
        } else {
            None
        };
        if entries.is_none() {
            // The directory is missing, or has gone since the watch started.
            // Watching where it now resolves replaces the watch only when
            // that moved, so a change already signalled is not lost.
            let next = watch().await;
            if next.path() != changes.path() {
                changes = next;
                continue;
            }
        }
        let entries = entries.unwrap_or_default();
        if sent.as_ref() != Some(&entries) {
            let message = ServerMessage::Directory {
                subscription,
                entries: entries.clone(),
            };
            if outgoing.send(message).await.is_err() {
                return;
            }
            sent = Some(entries);
        }
        changes.changed().await;
    }
}

async fn write_loop(mut write: OwnedWriteHalf, mut outgoing: mpsc::Receiver<ServerMessage>) {
    while let Some(message) = outgoing.recv().await {
        let mut line = serde_json::to_string(&message).expect("protocol types serialize");
        line.push('\n');
        match timeout(WRITE_TIMEOUT, write.write_all(line.as_bytes())).await {
            Ok(Ok(())) => {}
            _ => break,
        }
    }
}
