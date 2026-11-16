#![cfg(unix)]

//! Workflow runs of the fake agent, against real repositories.
//!
//! The daemon runs from a directory that also holds the fake agent under
//! each harness's bridge name, so steps launch it as they launch a bridge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use ox_protocol::schema::{ContentBlock, SessionConfigKind, SessionUpdate, StopReason};
use ox_protocol::{
    Client, CommandInfo, CommandKind, CommandState, Error, ErrorBody, EventKind, ListingChange,
    MaybeUndefined, RunInfo, RunState, SessionInfo, SessionState, StepState, Watch, WorkflowInfo,
    WorktreeInfo, code,
};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{Duration, timeout};

const TIMEOUT: Duration = Duration::from_secs(10);
const OXD: &str = env!("CARGO_BIN_EXE_oxd");
const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_oxd-fake-agent");

/// How long the daemon waits without clients or work before it exits.
const IDLE_EXIT: Duration = Duration::from_secs(1);

/// The directory the daemon runs from: hard links to `oxd`, to `ox-mcp`,
/// and to the fake agent as `claude-code-acp` and `pi-acp`. It is made
/// again once per test binary, so that it links the binaries just built.
fn bin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("runs-bin");
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

/// Runs git in `dir` without the user's or the system's configuration, and
/// returns its output without the final newline.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
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

/// A repository on branch `main` with one commit that ignores the fake
/// agent's session logs, and an identity to commit with.
fn repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--quiet", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    std::fs::write(dir.path().join(".gitignore"), "fake-agent-*.jsonl\n").unwrap();
    git(dir.path(), &["add", ".gitignore"]);
    git(dir.path(), &["commit", "--quiet", "-m", "first"]);
    dir
}

struct Daemon {
    child: Child,
    /// Holds the socket, the worktrees, and `ox/workflows`, the daemon's
    /// workflows directory.
    dir: TempDir,
    socket: PathBuf,
}

impl Daemon {
    async fn start() -> Self {
        Self::start_in(tempfile::tempdir().unwrap()).await
    }

    /// Starts oxd from `bin_dir` on the socket in `dir`, with `dir` as its
    /// configuration home and without the user's or the system's git
    /// configuration, and waits for it to report that it is ready.
    async fn start_in(dir: TempDir) -> Self {
        let socket = dir.path().join("oxd.sock");
        let mut child = Command::new(bin_dir().join("oxd"))
            .arg("--socket")
            .arg(&socket)
            .arg("--idle-exit-ms")
            .arg(IDLE_EXIT.as_millis().to_string())
            .env("XDG_CONFIG_HOME", dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let ready = timeout(TIMEOUT, stdout.next_line()).await;
        assert_eq!(ready.unwrap().unwrap().as_deref(), Some("ready"));
        Self { child, dir, socket }
    }

    async fn client(&self) -> Client {
        timeout(TIMEOUT, Client::connect(&self.socket))
            .await
            .expect("timed out connecting")
            .unwrap()
    }

    /// Stops the daemon with SIGTERM, which keeps its runs and sessions
    /// saved, and starts another on its socket.
    async fn restart(mut self) -> Self {
        // SAFETY: sends SIGTERM to the daemon this test started.
        unsafe { libc::kill(self.child.id().unwrap().cast_signed(), libc::SIGTERM) };
        let status = timeout(TIMEOUT, self.child.wait())
            .await
            .expect("oxd did not exit")
            .unwrap();
        assert!(status.success());
        Self::start_in(self.dir).await
    }

    /// Saves `yaml` as the definition of workflow `name`.
    fn define(&self, name: &str, yaml: &str) {
        let workflows = self.dir.path().join("ox/workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::write(workflows.join(format!("{name}.yaml")), yaml).unwrap();
    }

    /// Saves a workflow `name` of two steps of the fake agent, `First` with
    /// `first` as its prompt and `Second` with `second`.
    fn define_two(&self, name: &str, first: &str, second: &str) {
        self.define(
            name,
            &format!(
                "name: Two steps
steps:
  - name: First
    agent: {{harness: claude}}
    prompt: {}
  - name: Second
    agent: {{harness: pi}}
    prompt: {}
",
                yaml_string(first),
                yaml_string(second)
            ),
        );
    }
}

/// `text` as a YAML double-quoted scalar, which JSON's strings are.
fn yaml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

async fn add_workspace(client: &Client, path: &Path) -> String {
    client.add_workspace(path.to_owned()).await.unwrap().id
}

async fn start(client: &Client, workspace: &str, workflow: &str, input: &str) -> RunInfo {
    start_refused(client, workspace, workflow, input)
        .await
        .unwrap()
}

async fn start_refused(
    client: &Client,
    workspace: &str,
    workflow: &str,
    input: &str,
) -> Result<RunInfo, Error> {
    timeout(
        TIMEOUT,
        client.start_run(workspace, workflow, input, BTreeMap::new()),
    )
    .await
    .expect("timed out starting the run")
}

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> ErrorBody {
    match result {
        Err(Error::Remote(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn next_changes(watch: &mut Watch) -> Vec<ListingChange> {
    timeout(TIMEOUT, watch.next())
        .await
        .expect("timed out waiting for listing changes")
        .expect("the watch ended")
}

/// Waits until run `id` is listed and `done` holds for it, and returns it.
async fn until_run(client: &Client, id: &str, done: impl Fn(&RunInfo) -> bool) -> RunInfo {
    let mut watch = client.watch().await.unwrap();
    loop {
        for change in next_changes(&mut watch).await {
            if let ListingChange::Run { run } = change
                && run.id == id
                && done(&run)
            {
                return run;
            }
        }
    }
}

async fn until_ended(client: &Client, id: &str) -> RunInfo {
    until_run(client, id, |run| run.state != RunState::Running).await
}

/// Waits until session `id` is listed and `done` holds for it, and returns
/// it.
async fn until_session(
    client: &Client,
    id: &str,
    done: impl Fn(&SessionInfo) -> bool,
) -> SessionInfo {
    let mut watch = client.watch().await.unwrap();
    loop {
        for change in next_changes(&mut watch).await {
            if let ListingChange::Session { session } = change
                && session.id == id
                && done(&session)
            {
                return *session;
            }
        }
    }
}

/// Waits until step `position` of run `id` runs a turn, and returns its
/// session's ID.
async fn until_turn(client: &Client, id: &str, position: usize) -> String {
    let run = until_run(client, id, |run| run.steps[position].session_id.is_some()).await;
    let session = run.steps[position].session_id.clone().unwrap();
    until_session(client, &session, |session| {
        session.state == SessionState::Running
    })
    .await;
    session
}

/// The listed session `id`, if it is listed.
async fn listed_session(client: &Client, id: &str) -> Option<SessionInfo> {
    client
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == id)
}

async fn worktree_of(client: &Client, run: &RunInfo) -> WorktreeInfo {
    let mut watch = client.watch().await.unwrap();
    next_changes(&mut watch)
        .await
        .into_iter()
        .find_map(|change| match change {
            ListingChange::Worktree { worktree } if worktree.id == run.worktree_id => {
                Some(worktree)
            }
            _ => None,
        })
        .expect("the run's worktree is listed")
}

/// The first `turn_ended` event of session `id`.
async fn turn_ended(client: &Client, id: &str) -> EventKind {
    let mut events = client.subscribe(id, 0).await.unwrap();
    loop {
        let event = timeout(TIMEOUT, events.next())
            .await
            .expect("timed out waiting for the turn to end")
            .unwrap();
        if let EventKind::TurnEnded { .. } = event.kind {
            return event.kind;
        }
    }
}

/// The current value of the session's config option `id`.
fn config_value(session: &SessionInfo, id: &str) -> String {
    let option = session
        .config_options
        .iter()
        .find(|option| *option.config_id.0 == *id)
        .unwrap_or_else(|| panic!("no option {id}"));
    let SessionConfigKind::Select(select) = &option.kind else {
        panic!("{id} is not a select option");
    };
    select.current_value.to_string()
}

/// The current time in milliseconds since the Unix epoch.
fn now() -> u64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    u64::try_from(since_epoch.as_millis()).unwrap()
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    !git(repo, &["branch", "--list", branch]).is_empty()
}

/// The launch input reaches the first step, the first step's final reply
/// reaches the second, and each step runs its harness's bridge in the run's
/// worktree with the config the definition sets.
#[tokio::test(flavor = "multi_thread")]
async fn each_step_receives_the_previous_final_reply() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define(
        "hand-off",
        r#"name: Hand off
steps:
  - name: First
    agent:
      harness: claude
      model: fast
      effort: high
      permission_mode: code
    prompt: |-
      messages 1 "earlier"
      text "text \""
  - name: Second
    agent:
      harness: pi
      model: fast
      effort: low
      permission_mode: code
    prompt: text "second "
"#,
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let started = start(&client, &workspace, "hand-off", r#"text "handed-off\"""#).await;
    assert_eq!(started.workflow, "hand-off");
    assert_eq!(started.name, "Hand off");
    assert_eq!(started.state, RunState::Running);

    let run = until_ended(&client, &started.id).await;
    assert_eq!(run.state, RunState::Succeeded, "{run:?}");
    let outputs: Vec<_> = run
        .steps
        .iter()
        .map(|step| (step.state, step.output.as_deref()))
        .collect();
    assert_eq!(
        outputs,
        [
            (StepState::Succeeded, Some(r#"text "handed-off""#)),
            (StepState::Succeeded, Some("second handed-off")),
        ]
    );
    let worktree = worktree_of(&client, &run).await;
    for (step, (bridge, effort)) in run
        .steps
        .iter()
        .zip([("claude-code-acp", "high"), ("pi-acp", "low")])
    {
        let session = listed_session(&client, step.session_id.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(session.cwd, worktree.path);
        assert_eq!(
            session.worktree_id.as_deref(),
            Some(run.worktree_id.as_str())
        );
        assert!(
            session.command[0].ends_with(bridge),
            "{:?}",
            session.command
        );
        assert_eq!(config_value(&session, "model"), "fast");
        assert_eq!(config_value(&session, "effort"), effort);
        assert_eq!(config_value(&session, "mode"), "code");
    }
}

/// Cancelling a run cancels its active step's turn, and later steps do not
/// start. The worktree stays.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_run_stops_at_its_active_step() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define_two("waits", "wait", r#"text "unreached""#);
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "waits", "").await;
    let session = until_turn(&client, &run.id, 0).await;

    client.cancel_run(&run.id).await.unwrap();
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Cancelled);
    assert_eq!(run.error, None);
    assert_eq!(run.steps[0].state, StepState::Cancelled);
    assert_eq!(run.steps[0].session_id.as_deref(), Some(session.as_str()));
    assert_eq!(run.steps[1].state, StepState::Pending);
    assert_eq!(run.steps[1].session_id, None);
    assert_eq!(
        turn_ended(&client, &session).await,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::Cancelled),
            error: None
        }
    );
    assert!(worktree_of(&client, &run).await.path.is_dir());
    assert_eq!(
        refusal(client.cancel_run(&run.id).await).code,
        code::INVALID_STATE
    );
}

/// A run keeps going after the client that started it disconnects, for
/// longer than the daemon's idle exit.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_continues_without_clients() {
    let repo = repo();
    let daemon = Daemon::start().await;
    // The first step streams empty chunks, so it hands the second nothing.
    daemon.define_two("slow", r#"stream 5 700 """#, "pid");
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "slow", "").await;
    drop(client);

    // No client is connected for longer than the idle exit.
    tokio::time::sleep(IDLE_EXIT + Duration::from_millis(500)).await;
    let client = daemon.client().await;
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Succeeded, "{run:?}");
}

/// A run's record and its child sessions' history survive a restart, and a
/// run that was running is interrupted.
#[tokio::test(flavor = "multi_thread")]
async fn runs_survive_a_restart_and_running_ones_are_interrupted() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define_two("quick", r#"text "text \"two\"""#, r#"text "second ""#);
    daemon.define_two("waits", "wait", r#"text "unreached""#);
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let done = start(&client, &workspace, "quick", "").await;
    let done = until_ended(&client, &done.id).await;
    assert_eq!(done.state, RunState::Succeeded, "{done:?}");
    let waiting = start(&client, &workspace, "waits", "").await;
    until_turn(&client, &waiting.id, 0).await;
    drop(client);

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    let runs = until_run(&client, &waiting.id, |_| true).await;
    assert_eq!(runs.state, RunState::Interrupted);
    assert_eq!(runs.steps[0].state, StepState::Interrupted);
    assert_eq!(runs.steps[1].state, StepState::Pending);
    assert_eq!(until_run(&client, &done.id, |_| true).await, done);

    let second = done.steps[1].session_id.as_deref().unwrap();
    let mut events = client.subscribe(second, 0).await.unwrap();
    let mut reply = String::new();
    loop {
        let event = timeout(TIMEOUT, events.next()).await.unwrap().unwrap();
        match event.kind {
            EventKind::Update(ox_protocol::schema::SessionUpdate::AgentMessageChunk(chunk)) => {
                if let ox_protocol::schema::ContentBlock::Text(text) = chunk.content {
                    reply.push_str(&text.text);
                }
            }
            EventKind::TurnEnded { .. } => break,
            _ => {}
        }
    }
    assert_eq!(reply, "second two");
}

/// A run is removed only once it has ended, and with its worktree's work
/// only with `discard`: then its worktree, branch, child sessions and record
/// go, runs first.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_removed_with_its_worktree_and_child_sessions() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define_two("waits", "wait", r#"text "unreached""#);
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "waits", "").await;
    let session = until_turn(&client, &run.id, 0).await;
    let refused = refusal(client.delete_run(&run.id, true, BTreeMap::new()).await);
    assert_eq!(refused.code, code::INVALID_STATE);
    assert_eq!(refused.message, "the run is running");

    client.cancel_run(&run.id).await.unwrap();
    let run = until_ended(&client, &run.id).await;
    let worktree = worktree_of(&client, &run).await;
    std::fs::write(worktree.path.join("notes.txt"), "work\n").unwrap();
    let refused = refusal(client.delete_run(&run.id, false, BTreeMap::new()).await);
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert!(worktree.path.join("notes.txt").is_file());
    assert!(listed_session(&client, &session).await.is_some());
    assert_eq!(until_run(&client, &run.id, |_| true).await, run);

    let mut watch = client.watch().await.unwrap();
    next_changes(&mut watch).await;
    let deleted = client
        .delete_run(&run.id, true, BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(deleted.kept_branch, None);
    assert!(!worktree.path.exists());
    assert!(!branch_exists(repo.path(), &worktree.branch));
    assert!(listed_session(&client, &session).await.is_none());
    let mut removals = Vec::new();
    while removals.len() < 3 {
        removals.extend(next_changes(&mut watch).await.into_iter().filter(|change| {
            matches!(
                change,
                ListingChange::RunRemoved { .. }
                    | ListingChange::SessionRemoved { .. }
                    | ListingChange::WorktreeRemoved { .. }
            )
        }));
    }
    assert_eq!(
        removals,
        [
            ListingChange::RunRemoved {
                run_id: run.id.clone()
            },
            ListingChange::SessionRemoved {
                session_id: session
            },
            ListingChange::WorktreeRemoved {
                worktree_id: worktree.id
            },
        ]
    );
}

/// Deleting a child session leaves the run's worktree and other child
/// sessions, which can still be loaded.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_child_session_leaves_the_run_worktree() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define_two("quick", r#"text "text \"two\"""#, r#"text "second ""#);
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "quick", "").await;
    let run = until_ended(&client, &run.id).await;
    let worktree = worktree_of(&client, &run).await;
    let first = run.steps[0].session_id.clone().unwrap();
    let second = run.steps[1].session_id.clone().unwrap();

    let deleted = client.delete(&first, false, BTreeMap::new()).await.unwrap();
    assert_eq!(deleted.kept_branch, None);
    assert!(worktree.path.is_dir());
    assert!(listed_session(&client, &first).await.is_none());
    let run = until_run(&client, &run.id, |run| run.steps[0].session_id.is_none()).await;
    assert_eq!(run.steps[1].session_id.as_deref(), Some(second.as_str()));
    drop(client);

    // The restarted daemon keeps the run's worktree and lets the remaining
    // child session load in it.
    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    assert!(worktree.path.is_dir());
    client.load_session(&second, BTreeMap::new()).await.unwrap();
    until_session(&client, &second, |session| {
        session.state == SessionState::Idle
    })
    .await;
    let run = until_run(&client, &run.id, |_| true).await;
    assert_eq!(run.steps[0].session_id, None);
}

/// A workflow that is not found or does not parse is refused, creating
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_or_invalid_workflow_is_refused() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define(
        "checked",
        "name: Checked
steps:
  - name: Plan
    agent: {harness: claude}
    prompt: Plan.
    retries: 2
",
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    for (workflow, expected) in [
        ("missing", code::NOT_FOUND),
        ("checked", code::INVALID_REQUEST),
        ("../checked", code::INVALID_REQUEST),
    ] {
        let refused = refusal(start_refused(&client, &workspace, workflow, "").await);
        assert_eq!(refused.code, expected, "{workflow}: {refused:?}");
    }
    let mut watch = client.watch().await.unwrap();
    let listed = next_changes(&mut watch).await;
    assert!(
        !listed
            .iter()
            .any(|change| matches!(change, ListingChange::Worktree { .. })),
        "{listed:?}"
    );
    assert_eq!(git(repo.path(), &["branch", "--list", "ox/*"]), "");
}

/// `workflow.list` lists the definitions in the daemon's workflows
/// directory.
#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_lists_its_workflows() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    assert!(client.list_workflows().await.unwrap().is_empty());
    daemon.define_two("plan-work", "text \"planned\"", "text \"worked\"");
    assert_eq!(
        client.list_workflows().await.unwrap(),
        [WorkflowInfo {
            workflow: "plan-work".into(),
            name: "Two steps".into(),
            error: None,
        }]
    );
}

/// A step whose turn fails, or whose config the agent refuses, fails the run
/// with the reason, and later steps do not start.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_step_fails_the_run() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define_two("fails", "fail boom", r#"text "unreached""#);
    daemon.define(
        "refused",
        r#"name: Refused
steps:
  - name: First
    agent: {harness: claude, effort: extreme}
    prompt: text "unreached"
  - name: Second
    agent: {harness: claude}
    prompt: text "unreached"
"#,
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    for (workflow, expected) in [("fails", "boom"), ("refused", "unknown option or value")] {
        let run = start(&client, &workspace, workflow, "").await;
        let run = until_ended(&client, &run.id).await;
        assert_eq!(run.state, RunState::Failed, "{workflow}: {run:?}");
        let error = run.error.as_deref().unwrap_or_default();
        assert!(error.contains(expected), "{workflow}: {error}");
        assert_eq!(run.steps[0].state, StepState::Failed, "{workflow}");
        assert_eq!(run.steps[0].output, None, "{workflow}");
        assert_eq!(run.steps[1].state, StepState::Pending, "{workflow}");
        assert_eq!(run.steps[1].session_id, None, "{workflow}");
        let started_at = run.steps[0].started_at.expect("the failed step started");
        assert!(
            (run.created_at..=now()).contains(&started_at),
            "{workflow}: {run:?}"
        );
        assert_eq!(run.steps[1].started_at, None, "{workflow}");
    }
}

/// A step's agent posts a status update through its `ox` MCP server, and
/// the step's child session records it with the time it was posted.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_s_agent_posts_status_updates_to_its_child_session() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define(
        "reports",
        "name: Reports
steps:
  - name: Read
    agent: {harness: claude}
    prompt: mcp-status Reading the code
",
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "reports", "").await;
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Succeeded, "{run:?}");
    let step = &run.steps[0];
    assert_eq!(step.output.as_deref(), Some("Status update posted."));
    let started_at = step.started_at.expect("the step started");

    let mut events = client
        .subscribe(step.session_id.as_deref().unwrap(), 0)
        .await
        .unwrap();
    let (text, posted_at) = loop {
        let event = timeout(TIMEOUT, events.next()).await.unwrap().unwrap();
        if let EventKind::StatusUpdate { text, posted_at } = event.kind {
            break (text, posted_at);
        }
    };
    assert_eq!(text, "Reading the code");
    assert!(
        (started_at..=now()).contains(&posted_at),
        "{posted_at} is not between {started_at} and now"
    );
}

/// A fake agent directive with a JSON string argument.
fn directive(name: &str, text: &str) -> String {
    format!("{name} {}", yaml_string(text))
}

/// A command that ended, as a step lists it.
fn ended(
    kind: CommandKind,
    command: &str,
    state: CommandState,
    exit_code: Option<i32>,
    output: &str,
) -> CommandInfo {
    CommandInfo {
        kind,
        command: command.into(),
        state,
        exit_code,
        output: output.into(),
        truncated: false,
    }
}

/// The text of the first `count` user messages of session `id`.
async fn user_messages(client: &Client, id: &str, count: usize) -> Vec<String> {
    let mut events = client.subscribe(id, 0).await.unwrap();
    let mut messages = Vec::new();
    while messages.len() < count {
        let event = timeout(TIMEOUT, events.next())
            .await
            .expect("timed out waiting for user messages")
            .unwrap();
        if let EventKind::Update(SessionUpdate::UserMessage(message)) = event.kind {
            let MaybeUndefined::Value(content) = message.content else {
                panic!("a user message without content");
            };
            let text: Vec<_> = content
                .into_iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.text),
                    _ => None,
                })
                .collect();
            messages.push(text.join("\n"));
        }
    }
    messages
}

/// A shell step's standard output is its output, and both shell steps and
/// agent steps hand their output on. Commands read the run's and the step's
/// input from the environment.
#[tokio::test(flavor = "multi_thread")]
async fn a_shell_step_hands_its_standard_output_on() {
    let repo = repo();
    let daemon = Daemon::start().await;
    // The fake agent runs its input as a script, so the shell step prints a
    // directive.
    let first = r#"printf 'text "shell:%s"' "$OX_STEP_INPUT""#;
    let third = r#"printf %s "$OX_RUN_INPUT""#;
    daemon.define(
        "shell",
        &format!(
            "name: Shell
steps:
  - name: Shell
    shell: {}
  - name: Agent
    agent: {{harness: claude}}
    prompt: {}
  - name: Run input
    shell: {}
",
            yaml_string(first),
            yaml_string(r#"text "agent ""#),
            yaml_string(third),
        ),
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "shell", "in").await;
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Succeeded, "{run:?}");
    let outputs: Vec<_> = run
        .steps
        .iter()
        .map(|step| step.output.as_deref())
        .collect();
    assert_eq!(
        outputs,
        [
            Some(r#"text "shell:in""#),
            Some("agent shell:in"),
            Some("in")
        ]
    );
    let succeeded = |command: &str, output: &str| {
        ended(
            CommandKind::Shell,
            command,
            CommandState::Succeeded,
            Some(0),
            output,
        )
    };
    assert_eq!(
        run.steps[0].commands,
        [succeeded(first, r#"text "shell:in""#)]
    );
    assert_eq!(run.steps[0].session_id, None);
    assert!(run.steps[1].commands.is_empty());
    assert_eq!(run.steps[2].commands, [succeeded(third, "in")]);
}

/// A failed precondition fails the run before the step's agent starts, and a
/// shell step that exits non-zero fails the run, each with the command's
/// result and output.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_precondition_or_shell_step_fails_the_run() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let dirty = "echo dirty >&2; exit 3";
    daemon.define(
        "precondition",
        &format!(
            "name: Precondition
steps:
  - name: Checked
    agent: {{harness: claude}}
    preconditions:
      - shell: {}
    prompt: text \"unreached\"
  - name: Later
    shell: \"true\"
",
            yaml_string(dirty)
        ),
    );
    let boom = "echo boom; exit 2";
    daemon.define(
        "command",
        &format!(
            "name: Command
steps:
  - name: Build
    shell: {}
  - name: Later
    shell: \"true\"
",
            yaml_string(boom)
        ),
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    for (workflow, error, command) in [
        (
            "precondition",
            "a precondition exited with status 3",
            ended(
                CommandKind::Precondition,
                dirty,
                CommandState::Failed,
                Some(3),
                "dirty\n",
            ),
        ),
        (
            "command",
            "the command exited with status 2",
            ended(
                CommandKind::Shell,
                boom,
                CommandState::Failed,
                Some(2),
                "boom\n",
            ),
        ),
    ] {
        let run = start(&client, &workspace, workflow, "").await;
        let run = until_ended(&client, &run.id).await;
        assert_eq!(run.state, RunState::Failed, "{workflow}: {run:?}");
        assert_eq!(run.error.as_deref(), Some(error), "{workflow}");
        assert_eq!(run.steps[0].state, StepState::Failed, "{workflow}");
        assert_eq!(run.steps[0].commands, [command], "{workflow}");
        assert_eq!(run.steps[0].session_id, None, "{workflow}");
        assert_eq!(run.steps[0].output, None, "{workflow}");
        assert_eq!(run.steps[1].state, StepState::Pending, "{workflow}");
        assert!(run.steps[1].commands.is_empty(), "{workflow}");
    }
}

/// An agent step whose postcondition fails gets one repair turn with the
/// failure, and succeeds with the repair turn's reply once the
/// postcondition passes.
#[tokio::test(flavor = "multi_thread")]
async fn a_repair_turn_can_fix_a_failed_postcondition() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let repair = format!(
        "{}\n{}",
        directive("shell", "touch fixed"),
        directive("text", "fixed it")
    );
    let prompt = format!(
        "{}\n{}",
        directive("next", &repair),
        directive("text", "done")
    );
    daemon.define(
        "repair",
        &format!(
            "name: Repair
steps:
  - name: Fix
    agent: {{harness: claude}}
    prompt: {}
    postconditions:
      - shell: test -f fixed
",
            yaml_string(&prompt)
        ),
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "repair", "").await;
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Succeeded, "{run:?}");
    let step = &run.steps[0];
    assert_eq!(step.output.as_deref(), Some("fixed it"));
    let check = |state, exit_code| {
        ended(
            CommandKind::Postcondition,
            "test -f fixed",
            state,
            Some(exit_code),
            "",
        )
    };
    assert_eq!(
        step.commands,
        [
            check(CommandState::Failed, 1),
            check(CommandState::Succeeded, 0)
        ]
    );
    let messages = user_messages(&client, step.session_id.as_deref().unwrap(), 2).await;
    assert_eq!(
        messages[1],
        "A postcondition of this step failed after your turn. Fix what it checks so that it \
        passes, or reply with the reason you cannot.\n\nCommand:\ntest -f fixed\n\n\
        Result: exited with status 1\n"
    );
}

/// The brief's definition, with its checks verbatim and the fake agent's
/// options, and `commit` as the Commit step's script.
fn define_full(daemon: &Daemon, name: &str, commit: &str) {
    let plan = directive(
        "shell",
        "mkdir -p .agents/plans && echo plan > .agents/plans/p.md && git add .agents/plans/p.md",
    );
    let work = directive(
        "shell",
        "mkdir -p .agents/work && echo src > src.txt && echo log > .agents/work/w.md \
        && git add src.txt .agents/work/w.md",
    );
    // Review hands Commit the staged files as directives, which the fake
    // agent runs as part of its input.
    let review = format!(
        "{}\n{}",
        directive(
            "shell",
            r#"git diff --cached --name-only | sed 's/.*/text "&"/'"#
        ),
        directive(
            "shell",
            "mkdir -p .agents/reviews && echo review > .agents/reviews/r.md \
            && git add .agents/reviews/r.md",
        ),
    );
    let definition = r#"name: Plan, Work, Review, Commit

steps:
  - name: Plan
    agent: &claude
      harness: claude
      model: smart
      effort: high
      permission_mode: code
    preconditions:
      - shell: |
          set -eu
          status=$(git status --porcelain --untracked-files=all)
          test -z "$status"
          start_head=$(git rev-parse HEAD)
          start_head_file=$(git rev-parse --git-path ox-workflow-start-head)
          printf '%s\n' "$start_head" > "$start_head_file"
    prompt: PLAN
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/plans/)"

  - name: Work
    agent:
      harness: pi
      model: fast
      effort: high
    prompt: WORK
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/work/)"

  - name: Review
    agent: *claude
    prompt: REVIEW
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/reviews/)"

  - name: Commit
    agent:
      harness: claude
      model: fast
      effort: low
      permission_mode: code
    prompt: COMMIT
    postconditions:
      - shell: |
          set -eu
          start_head_file=$(git rev-parse --git-path ox-workflow-start-head)
          start_head=$(cat "$start_head_file")
          test "$(git rev-parse HEAD^ 2>/dev/null)" = "$start_head"
          status=$(git status --porcelain --untracked-files=all)
          test -z "$status"
          rm "$start_head_file"
"#
    .replace("PLAN", &yaml_string(&plan))
    .replace("WORK", &yaml_string(&work))
    .replace("REVIEW", &yaml_string(&review))
    .replace("COMMIT", &yaml_string(commit));
    daemon.define(name, &definition);
}

/// The brief's full definition runs to the end: Review sees every change
/// staged, and Commit makes one commit of all of it, leaving the worktree
/// clean.
#[tokio::test(flavor = "multi_thread")]
async fn the_full_workflow_commits_every_staged_change() {
    let repo = repo();
    let start_head = git(repo.path(), &["rev-parse", "HEAD"]);
    let daemon = Daemon::start().await;
    define_full(
        &daemon,
        "full",
        &directive("shell", "git commit --quiet -m 'Add the change'"),
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "full", "").await;
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Succeeded, "{run:?}");
    assert_eq!(
        run.steps[2].output.as_deref(),
        Some("text \".agents/plans/p.md\"\ntext \".agents/work/w.md\"\ntext \"src.txt\"\n")
    );
    for step in &run.steps {
        assert!(
            step.commands
                .iter()
                .all(|command| command.state == CommandState::Succeeded),
            "{step:?}"
        );
    }
    let worktree = worktree_of(&client, &run).await;
    assert_eq!(
        git(
            &worktree.path,
            &["rev-list", &format!("{start_head}..HEAD")]
        )
        .lines()
        .count(),
        1
    );
    assert_eq!(
        git(
            &worktree.path,
            &["show", "--name-only", "--format=", "HEAD"]
        ),
        ".agents/plans/p.md\n.agents/reviews/r.md\n.agents/work/w.md\nsrc.txt"
    );
    assert_eq!(
        git(
            &worktree.path,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );
}

/// The full definition's final check fails the run after the repair turn
/// when Commit makes no commit, or leaves the worktree dirty. The step keeps
/// the repair turn's reply.
#[tokio::test(flavor = "multi_thread")]
async fn the_final_check_catches_a_missing_commit_and_a_dirty_worktree() {
    let repo = repo();
    let daemon = Daemon::start().await;
    define_full(
        &daemon,
        "uncommitted",
        &directive("next", &directive("text", "cannot commit")),
    );
    define_full(
        &daemon,
        "dirty",
        &format!(
            "{}\n{}",
            directive(
                "shell",
                "git commit --quiet -m 'Add the change' && touch stray"
            ),
            directive("next", &directive("text", "still dirty")),
        ),
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    for (workflow, reply) in [("uncommitted", "cannot commit"), ("dirty", "still dirty")] {
        let run = start(&client, &workspace, workflow, "").await;
        let run = until_ended(&client, &run.id).await;
        assert_eq!(run.state, RunState::Failed, "{workflow}: {run:?}");
        assert_eq!(
            run.error.as_deref(),
            Some("a postcondition exited with status 1 after the repair turn"),
            "{workflow}"
        );
        let commit = &run.steps[3];
        assert_eq!(commit.state, StepState::Failed, "{workflow}");
        assert_eq!(commit.output.as_deref(), Some(reply), "{workflow}");
        let states: Vec<_> = commit
            .commands
            .iter()
            .map(|command| (command.kind, command.state, command.exit_code))
            .collect();
        let failed = (CommandKind::Postcondition, CommandState::Failed, Some(1));
        assert_eq!(states, [failed, failed], "{workflow}");
    }
}

/// Whether process `pid` exists.
fn process_exists(pid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Cancelling a run stops its running command and the processes the command
/// started.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_run_stops_its_command() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let command = r#"sleep 30 & echo $! > "$OX_RUN_INPUT"; wait"#;
    daemon.define(
        "sleeps",
        &format!(
            "name: Sleeps
steps:
  - name: Sleep
    shell: {}
",
            yaml_string(command)
        ),
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let scratch = tempfile::tempdir().unwrap();
    let pid_file = scratch.path().join("pid");
    let run = start(&client, &workspace, "sleeps", pid_file.to_str().unwrap()).await;
    let pid: libc::pid_t = timeout(TIMEOUT, async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && let Some(pid) = text.strip_suffix('\n')
            {
                return pid.parse().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the command did not start");
    assert!(process_exists(pid));

    client.cancel_run(&run.id).await.unwrap();
    let run = until_ended(&client, &run.id).await;
    assert_eq!(run.state, RunState::Cancelled, "{run:?}");
    assert_eq!(run.steps[0].state, StepState::Cancelled);
    assert_eq!(run.steps[0].commands.len(), 1);
    assert_eq!(run.steps[0].commands[0].state, CommandState::Cancelled);
    assert_eq!(run.steps[0].commands[0].exit_code, None);
    timeout(TIMEOUT, async {
        while process_exists(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the background process is still running");
}

/// A command that is running when the daemon stops is interrupted.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_running_at_a_restart_is_interrupted() {
    let repo = repo();
    let daemon = Daemon::start().await;
    daemon.define(
        "sleeps",
        "name: Sleeps
steps:
  - name: Sleep
    shell: sleep 30
",
    );
    let client = daemon.client().await;
    let workspace = add_workspace(&client, repo.path()).await;
    let run = start(&client, &workspace, "sleeps", "").await;
    until_run(&client, &run.id, |run| !run.steps[0].commands.is_empty()).await;
    drop(client);

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    let run = until_run(&client, &run.id, |_| true).await;
    assert_eq!(run.state, RunState::Interrupted);
    assert_eq!(run.steps[0].state, StepState::Interrupted);
    assert_eq!(
        run.steps[0].commands,
        [ended(
            CommandKind::Shell,
            "sleep 30",
            CommandState::Interrupted,
            None,
            ""
        )]
    );
}
