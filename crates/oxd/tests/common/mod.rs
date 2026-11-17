//! Helpers the oxd integration tests share: a daemon to test against, real
//! repositories, a FIFO that follows an agent process, and readers of
//! subscriptions and watches.

#![allow(dead_code, reason = "each test binary uses part of these helpers")]

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ox_protocol::schema::{ContentBlock, TextContent};
use ox_protocol::{
    Client, Error, ErrorBody, Event, EventKind, Listing, ListingChange, SessionInfo, SessionState,
    Stream, Subscription, WorkspaceInfo,
};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The idle exit of a daemon whose test is about it: the daemon's default.
pub const IDLE_EXIT_MS: u64 = 5000;
pub const OXD: &str = env!("CARGO_BIN_EXE_oxd");
pub const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_oxd-fake-agent");

/// The directory the daemon runs from for `Options::bridges`: hard links to
/// `oxd`, to `ox-mcp`, and to the fake agent as `claude-code-acp` and
/// `pi-acp`. It is made again once per test binary, so that it links the
/// binaries just built.
fn bin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "{}-bin",
            std::env::current_exe()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let oxd = Path::new(OXD);
        for (source, name) in [
            (oxd.to_owned(), "oxd"),
            (oxd.with_file_name("ox-mcp"), "ox-mcp"),
            (PathBuf::from(FAKE_AGENT), "claude-code-acp"),
            (PathBuf::from(FAKE_AGENT), "pi-acp"),
        ] {
            std::fs::hard_link(&source, dir.join(name)).unwrap_or_else(|error| {
                panic!(
                    "linking {}: {error}; `cargo build -p ox-mcp` builds ox-mcp",
                    source.display()
                )
            });
        }
        dir
    })
}

/// How a test daemon runs.
#[derive(Clone, Default)]
pub struct Options {
    /// How long the daemon stays idle before it exits. Unset, it runs until
    /// it is stopped.
    pub idle_exit_ms: Option<u64>,
    /// A limit on the daemon's open file descriptors.
    pub fd_limit: Option<u32>,
    pub park_after_ms: Option<u64>,
    /// Runs the daemon from a directory that holds the fake agent under each
    /// harness's bridge name, so the daemon launches it as it launches a
    /// bridge.
    pub bridges: bool,
    /// Variables over the daemon's environment.
    pub env: BTreeMap<String, String>,
    /// Pipes the daemon's standard error for `Daemon::wait_for_log`.
    pub log: bool,
}

/// An oxd on a socket in its own directory, without the user's or the
/// system's git configuration. The directory is its configuration home, so
/// git reads no global ignore or attributes file.
pub struct Daemon {
    pub child: Child,
    stderr: Option<Lines<BufReader<ChildStderr>>>,
    /// Holds the socket and the worktrees.
    pub dir: TempDir,
    pub socket: PathBuf,
    options: Options,
}

impl Daemon {
    pub async fn start() -> Self {
        Self::start_with(Options::default()).await
    }

    pub async fn start_with(options: Options) -> Self {
        Self::start_in(tempfile::tempdir().unwrap(), options).await
    }

    /// Starts oxd on the socket in `dir` and waits for it to report that it
    /// is ready. A daemon started on the socket of one that exited finds its
    /// workspaces and sessions.
    pub async fn start_in(dir: TempDir, options: Options) -> Self {
        let socket = dir.path().join("oxd.sock");
        let oxd = if options.bridges {
            bin_dir().join("oxd")
        } else {
            PathBuf::from(OXD)
        };
        let mut command = match options.fd_limit {
            Some(limit) => {
                let mut command = Command::new("sh");
                command
                    .args(["-c", &format!("ulimit -n {limit} && exec \"$0\" \"$@\"")])
                    .arg(&oxd);
                command
            }
            None => Command::new(&oxd),
        };
        command
            .arg("--socket")
            .arg(&socket)
            // The daemon caps its timers, so the longest delay outlasts any
            // test.
            .arg("--idle-exit-ms")
            .arg(options.idle_exit_ms.unwrap_or(u64::MAX).to_string())
            .args(
                options
                    .park_after_ms
                    .iter()
                    .flat_map(|ms| ["--park-after-ms".to_owned(), ms.to_string()]),
            )
            .env("XDG_CONFIG_HOME", dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .envs(&options.env)
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        if options.log {
            command.stderr(Stdio::piped());
        }
        let mut child = command.spawn().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let ready = timeout(TIMEOUT, stdout.next_line()).await;
        assert_eq!(ready.unwrap().unwrap().as_deref(), Some("ready"));
        let stderr = child
            .stderr
            .take()
            .map(|stderr| BufReader::new(stderr).lines());
        Self {
            child,
            stderr,
            dir,
            socket,
            options,
        }
    }

    pub async fn client(&self) -> Client {
        timeout(TIMEOUT, Client::connect(&self.socket))
            .await
            .expect("timed out connecting")
            .unwrap()
    }

    /// Stops the daemon with SIGTERM, which keeps its sessions saved, and
    /// waits for it to exit.
    pub async fn stop(&mut self) {
        self.stop_signal();
        assert!(self.wait_for_exit().await.success());
    }

    /// Sends the daemon SIGTERM without waiting for it to exit.
    pub fn stop_signal(&self) {
        // SAFETY: sends SIGTERM to the daemon this test started.
        unsafe { libc::kill(self.child.id().unwrap().cast_signed(), libc::SIGTERM) };
    }

    /// Stops the daemon and starts another on its socket, as it ran.
    pub async fn restart(self) -> Self {
        let options = self.options.clone();
        self.restart_with(options).await
    }

    /// Stops the daemon and starts another on its socket with `options`.
    pub async fn restart_with(mut self, options: Options) -> Self {
        self.stop().await;
        self.again(options).await
    }

    /// Starts another daemon on the socket of this one, which has exited.
    pub async fn again(self, options: Options) -> Self {
        Self::start_in(self.dir, options).await
    }

    /// Waits for the daemon to exit, allowing for its idle exit.
    pub async fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        let idle_exit = Duration::from_millis(self.options.idle_exit_ms.unwrap_or(0));
        timeout(TIMEOUT + idle_exit, self.child.wait())
            .await
            .expect("oxd did not exit")
            .unwrap()
    }

    /// Waits for a line of oxd's stderr that contains `text`.
    pub async fn wait_for_log(&mut self, text: &str) {
        let stderr = self.stderr.as_mut().expect("the daemon runs with `log`");
        loop {
            let line = timeout(TIMEOUT, stderr.next_line())
                .await
                .unwrap_or_else(|_| panic!("oxd never logged {text:?}"))
                .unwrap()
                .expect("oxd closed stderr");
            if line.contains(text) {
                return;
            }
        }
    }

    /// The daemon's directory, which sessions can run in.
    pub fn cwd(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    pub fn worktrees(&self) -> PathBuf {
        self.dir.path().join("worktrees")
    }
}

/// Runs git in `dir` without the user's or the system's configuration, and
/// returns its output without the final newline.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_command(dir, args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

pub fn git_command(dir: &Path, args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
    command
}

/// A repository on branch `main` with one commit of `file` holding
/// `contents`, and an identity to commit with.
pub fn repo_with(file: &str, contents: &str) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    init(dir.path());
    std::fs::write(dir.path().join(file), contents).unwrap();
    git(dir.path(), &["add", file]);
    git(dir.path(), &["commit", "--quiet", "-m", "first"]);
    dir
}

pub fn init(dir: &Path) {
    git(dir, &["init", "--quiet", "-b", "main"]);
    identify(dir);
}

/// Gives the repository at `dir` an identity, which git also writes in
/// reflogs.
pub fn identify(dir: &Path) {
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
}

/// A FIFO the fake agent holds open for as long as it runs (`--hold`), which
/// tells the test when the agent starts and when it exits.
pub struct ExitWatch {
    path: PathBuf,
    opened: Option<oneshot::Receiver<()>>,
    done: Option<JoinHandle<()>>,
}

impl ExitWatch {
    pub fn new(dir: &Path) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = dir.join(format!("hold-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let (opened, opened_rx) = oneshot::channel();
        let reader = path.clone();
        // Opening blocks until the agent opens the FIFO; reading ends once
        // every process holding it has exited.
        let done = tokio::task::spawn_blocking(move || {
            let mut fifo = std::fs::File::open(reader).unwrap();
            let _ = opened.send(());
            let mut rest = Vec::new();
            fifo.read_to_end(&mut rest).unwrap();
        });
        Self {
            path,
            opened: Some(opened_rx),
            done: Some(done),
        }
    }

    /// The fake agent's arguments that hold this FIFO.
    pub fn arg(&self) -> [&str; 2] {
        ["--hold", self.path.to_str().unwrap()]
    }

    /// The fake agent's command, holding this FIFO.
    pub fn command(&self) -> Vec<String> {
        std::iter::once(FAKE_AGENT)
            .chain(self.arg())
            .map(String::from)
            .collect()
    }

    /// Waits for the agent process to start.
    pub async fn started(&mut self) {
        let opened = self.opened.take().unwrap();
        timeout(TIMEOUT, opened)
            .await
            .expect("the agent did not start")
            .unwrap();
    }

    /// Waits for the agent process to exit.
    pub async fn exited(mut self) {
        let done = self.done.take().unwrap();
        timeout(TIMEOUT, done)
            .await
            .expect("the agent is still running")
            .unwrap();
    }
}

impl Drop for ExitWatch {
    /// Ends the blocking reader if no agent ever opened the FIFO, so a
    /// failing test does not hang.
    fn drop(&mut self) {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&self.path);
    }
}

/// A prompt of one text block.
pub fn text(text: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text(TextContent::new(text))]
}

pub fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> ErrorBody {
    match result {
        Err(Error::Remote(body)) => body,
        other => panic!("expected a daemon error, got {other:?}"),
    }
}

pub async fn next(subscription: &mut Subscription) -> Event {
    timeout(TIMEOUT, subscription.next())
        .await
        .expect("timed out waiting for an event")
        .expect("subscription ended")
}

/// Events up to and including the first one matching `last`.
pub async fn until(
    subscription: &mut Subscription,
    last: impl Fn(&EventKind) -> bool,
) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        let event = next(subscription).await;
        let done = last(&event.kind);
        events.push(event);
        if done {
            return events;
        }
    }
}

pub fn turn_ended(kind: &EventKind) -> bool {
    matches!(kind, EventKind::TurnEnded { .. })
}

/// The session's events from after `seq` up to its first `turn_ended`.
pub async fn until_turn_ended(client: &Client, session_id: &str, seq: u64) -> Vec<Event> {
    let mut events = client.subscribe(session_id, seq).await.unwrap();
    until(&mut events, turn_ended).await
}

pub async fn next_changes(watch: &mut Stream<Vec<ListingChange>>) -> Vec<ListingChange> {
    timeout(TIMEOUT, watch.next())
        .await
        .expect("timed out waiting for listing changes")
        .expect("the watch ended")
}

/// Every workspace, worktree and session, as a new watch first lists them.
pub async fn listing(client: &Client) -> Listing {
    let mut watch = client.watch().await.unwrap();
    let mut listing = Listing::default();
    for change in next_changes(&mut watch).await {
        apply(&mut listing, change);
    }
    listing
}

/// Loads the exited session `session_id`, as a client does by focusing it,
/// and waits until it is no longer `exited`. The client stays focused on it.
pub async fn load(client: &Client, session_id: &str) {
    let mut watch = client.watch().await.unwrap();
    let mut listing = Listing::default();
    for change in next_changes(&mut watch).await {
        apply(&mut listing, change);
    }
    client
        .focus(Some(session_id), BTreeMap::new())
        .await
        .unwrap();
    while !listing
        .sessions
        .iter()
        .any(|session| session.id == session_id && session.state != SessionState::Exited)
    {
        for change in next_changes(&mut watch).await {
            apply(&mut listing, change);
        }
    }
}

pub async fn sessions(client: &Client) -> Vec<SessionInfo> {
    listing(client).await.sessions
}

pub async fn workspaces(client: &Client) -> Vec<WorkspaceInfo> {
    listing(client).await.workspaces
}

/// The listed session `id`, if it is listed.
pub async fn info(client: &Client, session_id: &str) -> Option<SessionInfo> {
    sessions(client)
        .await
        .into_iter()
        .find(|session| session.id == session_id)
}

/// The ID of the workspace for `path`, which is added if there is none.
pub async fn workspace_id(client: &Client, path: &Path) -> String {
    match workspaces(client)
        .await
        .into_iter()
        .find(|w| w.path == path)
    {
        Some(workspace) => workspace.id,
        None => client.add_workspace(path.to_owned()).await.unwrap().id,
    }
}

/// Applies a watch's change to `listing`, as a client keeps its listing.
pub fn apply(listing: &mut Listing, change: ListingChange) {
    match change {
        ListingChange::Workspace { workspace } => {
            match listing.workspaces.iter_mut().find(|w| w.id == workspace.id) {
                Some(listed) => *listed = workspace,
                None => listing.workspaces.push(workspace),
            }
        }
        ListingChange::WorkspaceRemoved { workspace_id } => {
            listing.workspaces.retain(|w| w.id != workspace_id);
        }
        ListingChange::Worktree { worktree } => {
            match listing.worktrees.iter_mut().find(|w| w.id == worktree.id) {
                Some(listed) => *listed = worktree,
                None => listing.worktrees.push(worktree),
            }
        }
        ListingChange::WorktreeRemoved { worktree_id } => {
            listing.worktrees.retain(|w| w.id != worktree_id);
        }
        ListingChange::Session { session } => {
            match listing.sessions.iter_mut().find(|s| s.id == session.id) {
                Some(listed) => *listed = *session,
                None => listing.sessions.push(*session),
            }
        }
        ListingChange::SessionRemoved { session_id } => {
            listing.sessions.retain(|s| s.id != session_id);
        }
    }
}

/// Makes the FIFOs `blocked` and `release` in `dir`, through which a script
/// says it has stopped and the test lets it go on.
pub fn mkfifos(dir: &Path) {
    for fifo in ["blocked", "release"] {
        let status = std::process::Command::new("mkfifo")
            .arg(dir.join(fifo))
            .status()
            .unwrap();
        assert!(status.success());
    }
}

/// Waits until the script using the FIFOs in `dir` has stopped.
pub async fn blocked(dir: &Path) {
    let fifo = dir.join("blocked");
    let read = tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo));
    let read = timeout(TIMEOUT, read)
        .await
        .expect("the script never stopped");
    assert_eq!(read.unwrap().unwrap(), "blocked\n");
}

/// Lets the script using the FIFOs in `dir` go on.
pub async fn release(dir: &Path) {
    let fifo = dir.join("release");
    let write = tokio::task::spawn_blocking(move || std::fs::write(fifo, "go\n"));
    timeout(TIMEOUT, write)
        .await
        .expect("the script was not waiting")
        .unwrap()
        .unwrap();
}
