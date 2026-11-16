//! oxd: a per-user daemon that owns ACP agent sessions on behalf of Ox UIs.
//!
//! Usage: `oxd [--socket PATH] [--idle-exit-ms MS] [--park-after-ms MS]`
//!
//! Workspaces, sessions and workflow runs are saved in `<socket>.db`, and
//! their worktrees are added in `worktrees` beside the socket. Workflow
//! definitions are read from `workflows` in Ox's configuration directory.
//!
//! The daemon prints `ready` on stdout once a daemon is listening on the
//! socket: itself, or one that was already running, in which case it exits.

mod branches;
mod conn;
mod daemon;
mod directories;
mod files;
mod instructions;
mod mcp;
mod run;
mod session;
mod shell;
mod state;
mod store;
mod workflow;
mod worktree;

use std::ffi::OsString;
use std::fs::{File, OpenOptions, Permissions, TryLockError};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};

use crate::daemon::Daemon;
use crate::directories::Directories;
use crate::mcp::OxServer;
use crate::store::Store;

const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(5);

/// How long a session stays idle, with no background work, and unviewed
/// before its agent is parked.
const DEFAULT_PARK_AFTER: Duration = Duration::from_mins(15);

/// How long a stopping daemon waits for its clients to receive what is
/// queued for them.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Pause after a failed `accept`. Accepting fails while the process or the
/// system is out of file descriptors, and descriptors other processes free
/// produce no event here.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

struct Args {
    socket: PathBuf,
    idle_exit: Duration,
    park_after: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut socket = None;
    let mut idle_exit = DEFAULT_IDLE_EXIT;
    let mut park_after = DEFAULT_PARK_AFTER;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{} needs a value", arg.display()))
        };
        match arg.to_str() {
            Some("--socket") => socket = Some(PathBuf::from(value()?)),
            Some("--idle-exit-ms") => {
                let value = value()?;
                idle_exit = value
                    .to_str()
                    .and_then(|value| value.parse().ok())
                    .map(Duration::from_millis)
                    .ok_or_else(|| format!("invalid --idle-exit-ms: {}", value.display()))?;
            }
            Some("--park-after-ms") => {
                let value = value()?;
                park_after = value
                    .to_str()
                    .and_then(|value| value.parse().ok())
                    .map(Duration::from_millis)
                    .ok_or_else(|| format!("invalid --park-after-ms: {}", value.display()))?;
            }
            _ => return Err(format!("unexpected argument: {}", arg.display())),
        }
    }
    Ok(Args {
        socket: socket.unwrap_or_else(ox_protocol::default_socket_path),
        idle_exit,
        park_after,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("oxd: {message}");
            return ExitCode::from(2);
        }
    };
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oxd: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> io::Result<()> {
    if let Some(dir) = args.socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Daemons start and exit one at a time under the startup lock, so a
    // daemon that finds the instance lock held knows its holder is
    // listening. The instance lock is held for the daemon's lifetime.
    let startup = open_lock(&with_suffix(&args.socket, ".startup"))?;
    startup.lock()?;
    let instance = open_lock(&with_suffix(&args.socket, ".lock"))?;
    match instance.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            report_ready();
            return Ok(());
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }

    // The database sits beside the socket, so each socket has its own
    // workspaces and sessions. The instance lock makes this daemon its only
    // writer.
    let database = with_suffix(&args.socket, ".db");
    let store = Store::open(&database)
        .map_err(|error| io::Error::other(format!("opening {}: {error}", database.display())))?;
    // Worktrees sit beside the socket too. Git runs them from the
    // repository, so the path is absolute.
    let worktrees = std::path::absolute(args.socket.with_file_name("worktrees"))?;
    let directories = Directories::new()
        .map_err(|error| io::Error::other(format!("starting the file watcher: {error}")))?;
    // Agents run `ox-mcp` in their own directories, so the socket it
    // connects to is absolute.
    let ox_server = Arc::new(OxServer::new(std::path::absolute(&args.socket)?)?);
    let workflows = ox_protocol::config_dir().join("workflows");
    let daemon = Daemon::new(
        store,
        worktrees,
        workflows,
        args.park_after,
        directories,
        ox_server,
    )
    .map_err(|error| io::Error::other(format!("reading {}: {error}", database.display())))?;
    let daemon = Arc::new(daemon);

    match std::fs::remove_file(&args.socket) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    // Connections from other users are refused below, whatever the mode.
    let listener = UnixListener::bind(&args.socket)?;
    std::fs::set_permissions(&args.socket, Permissions::from_mode(0o600))?;

    let mut hangup = signal(SignalKind::hangup())?;
    tokio::spawn(async move { while hangup.recv().await.is_some() {} });
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;

    report_ready();
    startup.unlock()?;
    // SAFETY: `geteuid` has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let idle = daemon.idle(args.idle_exit);
    tokio::pin!(idle);
    let (stopping, stopping_rx) = tokio::sync::watch::channel(false);
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    if stream.peer_cred().is_ok_and(|cred| cred.uid() == uid) {
                        let client = daemon.client();
                        connections.spawn(conn::serve(
                            daemon.clone(),
                            stream,
                            client,
                            stopping_rx.clone(),
                        ));
                    }
                }
                Err(error) => {
                    eprintln!("oxd: accept failed: {error}");
                    pause_after_failed_accept().await;
                }
            },
            Some(_) = connections.join_next() => {}
            () = &mut idle => break,
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }

    // A daemon that starts from here on waits for this one to exit, then
    // takes the instance lock.
    let _startup = tokio::task::spawn_blocking(move || startup.lock().map(|()| startup))
        .await
        .expect("lock task does not panic")?;
    let _ = std::fs::remove_file(&args.socket);
    drop(listener);
    daemon.close_all();
    // The next daemon reads the log once every queued event is committed.
    daemon.flush().await;
    // Each client receives the end of every session it follows. One that
    // stops reading is given up on.
    let _ = stopping.send(true);
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    Ok(())
}

/// Waits [`ACCEPT_RETRY_DELAY`] before accepting again.
#[expect(
    clippy::disallowed_methods,
    reason = "nothing signals that descriptors were freed"
)]
async fn pause_after_failed_accept() {
    tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
}

/// Tells whoever started the daemon that a daemon is listening. Nothing
/// else is written to stdout, so a reader that has gone away does no harm.
fn report_ready() {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "ready");
    let _ = stdout.flush();
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = OsString::from(path.as_os_str());
    path.push(suffix);
    PathBuf::from(path)
}

fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
}
