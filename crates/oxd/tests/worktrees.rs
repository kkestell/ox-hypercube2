#![cfg(unix)]

//! Sessions that run in worktrees, against real repositories.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use ox_protocol::schema::{ContentBlock, SessionUpdate};
use ox_protocol::{
    Branches, Checkout, Client, Comparison, Deleted, Error, EventKind, Listing, ListingChange,
    SessionInfo, SessionState, WorktreeInfo, code,
};
use tempfile::TempDir;
use tokio::time::timeout;

mod common;

use common::*;

/// Whether git succeeds in `dir`.
fn git_succeeds(dir: &Path, args: &[&str]) -> bool {
    git_command(dir, args).output().unwrap().status.success()
}

/// A repository on branch `main` with one commit, and an identity to commit
/// with.
fn repo() -> TempDir {
    repo_with("README", "committed\n")
}

/// A `git` for a client's `PATH` that stops at the first command whose
/// arguments contain `pattern`, until the test releases it, and otherwise
/// runs git.
struct BlockedGit {
    dir: TempDir,
}

impl BlockedGit {
    fn new(pattern: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        mkfifos(dir.path());
        let real = std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let real = String::from_utf8(real.stdout).unwrap();
        let real = real.trim();
        let dir_path = dir.path().to_str().unwrap();
        let stop = format!(
            "case \" $* \" in\n\
             *\"{pattern}\"*)\n\
             if mkdir \"{dir_path}/once\" 2>/dev/null; then\n\
             echo blocked > \"{dir_path}/blocked\"\n\
             read _ < \"{dir_path}/release\"\n\
             fi ;;\n\
             esac\n"
        );
        let script = format!("#!/bin/sh\n{stop}exec {real} \"$@\"\n");
        let git = dir.path().join("git");
        std::fs::write(&git, script).unwrap();
        std::fs::set_permissions(&git, PermissionsExt::from_mode(0o755)).unwrap();
        Self { dir }
    }

    /// A client environment whose `PATH` finds this `git` first.
    fn env(&self) -> BTreeMap<String, String> {
        let path = std::env::var("PATH").unwrap();
        BTreeMap::from([(
            "PATH".to_owned(),
            format!("{}:{path}", self.dir.path().display()),
        )])
    }

    /// Waits until a git command has stopped.
    async fn blocked(&self) {
        blocked(self.dir.path()).await;
    }

    /// Lets the stopped git command run.
    async fn release(&self) {
        release(self.dir.path()).await;
    }
}

/// The branches of the workspace for `path`.
async fn branches(client: &Client, path: &Path) -> Result<Branches, Error> {
    let workspace = workspace_id(client, path).await;
    timeout(TIMEOUT, client.branches(&workspace, BTreeMap::new()))
        .await
        .expect("timed out listing branches")
}

/// A new worktree on the session's `ox/<id8>` branch.
fn on_ox_branch() -> Checkout {
    Checkout::Worktree {
        compare: None,
        branch: None,
    }
}

/// Asks for a session of the fake agent in a workspace for `path`.
async fn new_session(
    client: &Client,
    path: &Path,
    checkout: Checkout,
) -> Result<SessionInfo, Error> {
    new_session_with(client, path, checkout, BTreeMap::new()).await
}

async fn new_session_with(
    client: &Client,
    path: &Path,
    checkout: Checkout,
    env: BTreeMap<String, String>,
) -> Result<SessionInfo, Error> {
    new_session_running(client, path, checkout, env, vec![FAKE_AGENT.into()]).await
}

async fn new_session_running(
    client: &Client,
    path: &Path,
    checkout: Checkout,
    env: BTreeMap<String, String>,
    command: Vec<String>,
) -> Result<SessionInfo, Error> {
    let workspace = workspace_id(client, path).await;
    client
        .new_session(&workspace, command, env, checkout, Vec::new(), None)
        .await
}

/// Starts a session of the fake agent holding `agent` in a new worktree of
/// the repository at `path`, and waits for the agent to start.
async fn started_holding(client: &Client, path: &Path, agent: &mut ExitWatch) -> SessionInfo {
    let session = new_session_running(
        client,
        path,
        on_ox_branch(),
        BTreeMap::new(),
        agent.command(),
    )
    .await
    .unwrap();
    let session = started(client, &session).await;
    agent.started().await;
    session
}

/// Deletes a session, with git run with the daemon's environment.
async fn delete(client: &Client, session_id: &str, discard: bool) -> Result<Deleted, Error> {
    timeout(TIMEOUT, client.delete(session_id, discard, BTreeMap::new()))
        .await
        .expect("timed out deleting the session")
}

/// Commits a new file in the checkout at `dir`.
fn commit_file(dir: &Path, name: &str) {
    std::fs::write(dir.join(name), format!("{name}\n")).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "--quiet", "-m", name]);
}

fn branch_exists(dir: &Path, branch: &str) -> bool {
    git_succeeds(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
}

/// Asserts that the worktree is gone from disk and from the repository at
/// `repo`.
fn assert_removed(repo: &Path, worktree: &WorktreeInfo) {
    assert!(
        !worktree.path.exists(),
        "{} exists",
        worktree.path.display()
    );
    let listed = worktree_paths(repo);
    let name = worktree.path.file_name().unwrap();
    assert!(
        listed.iter().all(|path| path.file_name() != Some(name)),
        "{listed:?}"
    );
}

/// Waits for the agent to answer a prompt, which shows it is running.
async fn answers(client: &Client, session_id: &str) {
    let mut events = client.subscribe(session_id, 0).await.unwrap();
    client.prompt(session_id, text("say 1 0")).await.unwrap();
    let ended = until(&mut events, turn_ended).await.pop().unwrap();
    assert!(
        matches!(ended.kind, EventKind::TurnEnded { error: None, .. }),
        "{ended:?}"
    );
}

/// The session's info from each watch change that names it, from a new
/// watch, until one `done` accepts, which it returns; `None` if the session
/// is not listed or is removed first.
async fn until_session(
    client: &Client,
    session_id: &str,
    done: impl Fn(&SessionInfo) -> bool,
) -> Option<SessionInfo> {
    let mut watch = client.watch().await.unwrap();
    // The watch's first changes list every session.
    let mut listed = false;
    loop {
        let changes = next_changes(&mut watch).await;
        let mut named = false;
        for change in changes {
            match change {
                ListingChange::Session { session } if session.id == session_id => {
                    named = true;
                    if done(&session) {
                        return Some(*session);
                    }
                }
                ListingChange::SessionRemoved { session_id: id } if id == session_id => {
                    return None;
                }
                _ => {}
            }
        }
        if !listed && !named {
            return None;
        }
        listed = true;
    }
}

/// Waits until the session is in `state`.
async fn until_state(client: &Client, session_id: &str, state: SessionState) {
    until_session(client, session_id, |session| session.state == state)
        .await
        .expect("the session is listed");
}

/// Waits for the session's agent to start and returns the session.
async fn started(client: &Client, session: &SessionInfo) -> SessionInfo {
    assert_eq!(session.state, SessionState::Starting);
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let event = timeout(TIMEOUT, events.next())
        .await
        .expect("timed out waiting for the agent")
        .unwrap();
    assert!(
        matches!(event.kind, EventKind::AgentStarted { .. }),
        "{event:?}"
    );
    info(client, &session.id).await.unwrap()
}

/// Waits for the session's startup to fail and returns its error.
async fn exited(client: &Client, session: &SessionInfo) -> String {
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let event = timeout(TIMEOUT, events.next())
        .await
        .expect("timed out waiting for the session to exit")
        .unwrap();
    match event.kind {
        EventKind::AgentExited { error: Some(error) } => error,
        kind => panic!("expected agent_exited with an error, got {kind:?}"),
    }
}

/// The worktrees a new watch lists first.
async fn listed_worktrees(client: &Client) -> Vec<WorktreeInfo> {
    let mut watch = client.watch().await.unwrap();
    next_changes(&mut watch)
        .await
        .into_iter()
        .filter_map(|change| match change {
            ListingChange::Worktree { worktree } => Some(worktree),
            _ => None,
        })
        .collect()
}

/// The session's worktree, as a new watch lists it.
async fn worktree_of(client: &Client, session: &SessionInfo) -> WorktreeInfo {
    let id = session
        .worktree_id
        .as_deref()
        .expect("the session has a worktree");
    listed_worktrees(client)
        .await
        .into_iter()
        .find(|worktree| worktree.id == id)
        .expect("the session's worktree is listed")
}

/// The paths `git worktree list` shows for the repository at `dir`.
fn worktree_paths(dir: &Path) -> Vec<PathBuf> {
    git(dir, &["worktree", "list", "--porcelain"])
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_runs_in_a_new_worktree_on_a_new_branch() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let status = git(repo.path(), &["status", "--porcelain", "--ignored"]);
    let head = git(repo.path(), &["rev-parse", "HEAD"]);

    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let answered = session.worktree_id.clone();
    let session = started(&client, &session).await;
    assert_eq!(session.worktree_id, answered);

    let id = session.worktree_id.clone().unwrap();
    let id8 = &id[..8];
    let name = canonical(repo.path());
    let name = name.file_name().unwrap().to_str().unwrap();
    let path = daemon.worktrees().join(format!("{name}-{id8}"));
    assert_eq!(session.cwd, path);
    assert_eq!(
        worktree_of(&client, &session).await,
        WorktreeInfo {
            id: id.clone(),
            path: path.clone(),
            branch: format!("ox/{id8}"),
            base: head.clone(),
            created_branch: true,
            compare: None,
            comparison: None,
        }
    );
    assert_eq!(git(repo.path(), &["rev-parse", &format!("ox/{id8}")]), head);
    assert_eq!(
        git(&path, &["branch", "--show-current"]),
        format!("ox/{id8}")
    );
    assert!(worktree_paths(repo.path()).contains(&canonical(&path)));
    assert_eq!(
        git(repo.path(), &["status", "--porcelain", "--ignored"]),
        status
    );
}

/// A worktree on an existing local branch starts at the branch's tip, and
/// deleting its session keeps the branch.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_runs_in_a_worktree_on_an_existing_branch() {
    let repo = repo();
    git(repo.path(), &["checkout", "--quiet", "-b", "topic"]);
    commit_file(repo.path(), "topic.txt");
    let tip = git(repo.path(), &["rev-parse", "HEAD"]);
    git(repo.path(), &["checkout", "--quiet", "main"]);
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let on_topic = Checkout::Worktree {
        compare: None,
        branch: Some("topic".into()),
    };

    let session = new_session(&client, repo.path(), on_topic.clone())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    assert_eq!(
        (
            worktree.branch.as_str(),
            worktree.base.as_str(),
            worktree.created_branch
        ),
        ("topic", tip.as_str(), false)
    );
    assert_eq!(git(&worktree.path, &["branch", "--show-current"]), "topic");

    // The branch is checked out in the session's worktree now.
    let refused = refusal(new_session(&client, repo.path(), on_topic).await);
    assert_eq!(refused.code, code::INVALID_REQUEST);
    assert!(
        refused.message.starts_with("topic is checked out at "),
        "{refused:?}"
    );
    let missing = Checkout::Worktree {
        compare: None,
        branch: Some("missing".into()),
    };
    let refused = refusal(new_session(&client, repo.path(), missing).await);
    assert_eq!(refused.message, "missing is not a local branch");
    assert_eq!(sessions(&client).await.len(), 1);

    let deleted = delete(&client, &session.id, false).await.unwrap();
    assert_eq!(deleted, Deleted { kept_branch: None });
    assert_removed(repo.path(), &worktree);
    assert_eq!(git(repo.path(), &["rev-parse", "topic"]), tip);
}

/// A watch lists a session's worktree as its own entry, before the session
/// that names it, and reports it removed with the session, after the
/// session's removal.
#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_is_listed_before_its_session_and_removed_with_it() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let id = session
        .worktree_id
        .clone()
        .expect("the session has a worktree");

    let mut watch = client.watch().await.unwrap();
    let changes = next_changes(&mut watch).await;
    let position = |found: &dyn Fn(&ListingChange) -> bool| {
        changes
            .iter()
            .position(found)
            .unwrap_or_else(|| panic!("not listed: {changes:?}"))
    };
    let worktree = position(
        &|change| matches!(change, ListingChange::Worktree { worktree } if worktree.id == id),
    );
    let listed = position(
        &|change| matches!(change, ListingChange::Session { session: listed } if listed.id == session.id),
    );
    assert!(worktree < listed, "{changes:?}");

    delete(&client, &session.id, false).await.unwrap();
    // The daemon forgets the session and its worktree in one change.
    let removals = loop {
        let changes = next_changes(&mut watch).await;
        let removals: Vec<&ListingChange> = changes
            .iter()
            .filter(|change| {
                matches!(change, ListingChange::SessionRemoved { session_id } if *session_id == session.id)
                    || matches!(change, ListingChange::WorktreeRemoved { worktree_id } if *worktree_id == id)
            })
            .collect();
        if !removals.is_empty() {
            break removals
                .into_iter()
                .map(|change| matches!(change, ListingChange::SessionRemoved { .. }))
                .collect::<Vec<_>>();
        }
    };
    assert_eq!(removals, [true, false], "the session's removal comes first");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subdirectory_workspace_runs_in_the_same_subdirectory() {
    let repo = repo();
    std::fs::create_dir(repo.path().join("sub")).unwrap();
    std::fs::write(repo.path().join("sub/file"), "in sub\n").unwrap();
    git(repo.path(), &["add", "sub"]);
    git(repo.path(), &["commit", "--quiet", "-m", "sub"]);
    let sub = canonical(&repo.path().join("sub"));
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    let session = new_session(&client, &sub, on_ox_branch()).await.unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    assert_eq!(session.cwd, worktree.path.join("sub"));
    assert!(session.cwd.join("file").is_file());

    // A subdirectory with nothing committed is not at the starting commit.
    let untracked = repo.path().join("untracked");
    std::fs::create_dir(&untracked).unwrap();
    let branches = git(repo.path(), &["branch", "--list"]);
    let refused = refusal(new_session(&client, &untracked, on_ox_branch()).await);
    assert_eq!(refused.code, code::INVALID_REQUEST);
    assert!(refused.message.contains("untracked"), "{refused:?}");
    assert_eq!(git(repo.path(), &["branch", "--list"]), branches);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_git_cannot_satisfy_is_refused_with_nothing_created() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    let plain = tempfile::tempdir().unwrap();
    let refused = refusal(new_session(&client, plain.path(), on_ox_branch()).await);
    assert_eq!(refused.code, code::INVALID_REQUEST);
    assert!(
        refused.message.contains("not in a Git working tree"),
        "{refused:?}"
    );

    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--quiet", "--bare"]);
    let refused = refusal(new_session(&client, bare.path(), on_ox_branch()).await);
    assert_eq!(refused.code, code::INVALID_REQUEST);
    assert!(
        refused.message.contains("not in a Git working tree"),
        "{refused:?}"
    );

    let unborn = tempfile::tempdir().unwrap();
    init(unborn.path());
    let refused = refusal(new_session(&client, unborn.path(), on_ox_branch()).await);
    assert_eq!(refused.code, code::INVALID_REQUEST);
    assert!(refused.message.contains("no commits"), "{refused:?}");
    assert_eq!(git(unborn.path(), &["branch", "--list"]), "");

    assert!(sessions(&client).await.is_empty());
    assert!(!daemon.worktrees().exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_that_cannot_be_added_is_undone() {
    let repo = repo();
    let daemon = Daemon::start().await;
    std::fs::create_dir(daemon.worktrees()).unwrap();
    std::fs::set_permissions(daemon.worktrees(), PermissionsExt::from_mode(0o555)).unwrap();
    let client = daemon.client().await;

    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    assert_eq!(session.state, SessionState::Starting);
    let branch = format!("ox/{}", &session.worktree_id.as_ref().unwrap()[..8]);
    let error = exited(&client, &session).await;
    assert!(error.contains("worktree add"), "{error}");
    let after = info(&client, &session.id).await.unwrap();
    assert_eq!(after.state, SessionState::Exited);
    assert_eq!(after.worktree_id, None);
    assert!(listed_worktrees(&client).await.is_empty());
    assert_eq!(git(repo.path(), &["branch", "--list", &branch]), "");
    std::fs::set_permissions(daemon.worktrees(), PermissionsExt::from_mode(0o755)).unwrap();

    drop(client);
    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    assert_eq!(info(&client, &session.id).await, Some(after));
}

/// Asks for a session of the fake agent in a new worktree of the
/// repository at `path`, with git run with `env` and the first prompt
/// `script`.
async fn new_prompted_session(
    client: &Client,
    path: &Path,
    env: BTreeMap<String, String>,
    script: &str,
) -> SessionInfo {
    let workspace = workspace_id(client, path).await;
    client
        .new_session(
            &workspace,
            vec![FAKE_AGENT.into()],
            env,
            on_ox_branch(),
            Vec::new(),
            Some(text(script)),
        )
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_sessions_first_prompt_runs_in_the_worktree_once_added() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let blocked = BlockedGit::new(" worktree add ");

    let session = new_prompted_session(&client, repo.path(), blocked.env(), "shell \"pwd\"").await;
    blocked.blocked().await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::PromptStarted { .. }
    ));
    blocked.release().await;

    let turn = until(&mut events, turn_ended).await;
    let output: String = turn
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Update(SessionUpdate::AgentMessageChunk(chunk)) => match &chunk.content {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        Path::new(output.trim()).canonicalize().unwrap(),
        session.cwd.canonicalize().unwrap()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_that_cannot_be_added_ends_the_first_prompts_turn() {
    let repo = repo();
    let daemon = Daemon::start().await;
    std::fs::create_dir(daemon.worktrees()).unwrap();
    std::fs::set_permissions(daemon.worktrees(), PermissionsExt::from_mode(0o555)).unwrap();
    let client = daemon.client().await;

    let session = new_prompted_session(&client, repo.path(), BTreeMap::new(), "pid").await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::PromptStarted { .. }
    ));
    let ended = next(&mut events).await;
    assert!(
        matches!(&ended.kind, EventKind::TurnEnded { error: Some(error), .. } if error.contains("worktree add")),
        "{ended:?}"
    );
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::AgentExited { error: Some(_) }
    ));
    std::fs::set_permissions(daemon.worktrees(), PermissionsExt::from_mode(0o755)).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linked_worktree_workspace_names_the_worktree_after_itself() {
    let repo = repo();
    let linked_dir = tempfile::tempdir().unwrap();
    let linked = linked_dir.path().join("linked");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "linked",
            linked.to_str().unwrap(),
        ],
    );
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    let session = new_session(&client, &linked, on_ox_branch()).await.unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    assert_eq!(
        worktree.path.file_name().unwrap().to_str().unwrap(),
        format!("linked-{}", &worktree.id[..8])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_ignores_the_clients_repository_variables() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let env = BTreeMap::from([("GIT_DIR".to_owned(), "/nonexistent".to_owned())]);

    let session = new_session_with(&client, repo.path(), on_ox_branch(), env)
        .await
        .unwrap();
    let session = started(&client, &session).await;
    assert!(worktree_of(&client, &session).await.path.is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_session_survives_a_restart_and_loads_only_while_its_directory_exists() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    drop(client);

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    let restored = info(&client, &session.id).await.unwrap();
    assert_eq!(restored.state, SessionState::Exited);
    assert_eq!(restored.worktree_id, session.worktree_id);
    assert_eq!(worktree_of(&client, &restored).await, worktree);
    assert_eq!(restored.cwd, session.cwd);

    std::fs::remove_dir_all(&session.cwd).unwrap();
    let error = client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap_err();
    let Error::Remote(refused) = error else {
        panic!("expected a daemon error, got {error}");
    };
    assert_eq!(refused.code, code::INVALID_STATE);
    assert!(
        refused.message.contains(session.cwd.to_str().unwrap()),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_worktree_leaves_a_branch_another_worktree_checked_out() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let blocked = BlockedGit::new(" worktree add ");

    let session = new_session_with(&client, repo.path(), on_ox_branch(), blocked.env())
        .await
        .unwrap();
    let branch = format!("ox/{}", &session.worktree_id.as_ref().unwrap()[..8]);
    blocked.blocked().await;
    // Another worktree checks the new branch out before the session's
    // worktree is added.
    let other = tempfile::tempdir().unwrap();
    let other = other.path().join("other");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            other.to_str().unwrap(),
            &branch,
        ],
    );
    let mut watch = client.watch().await.unwrap();
    blocked.release().await;

    // Every listing names only listed worktrees, through the session's exit.
    let mut listing = Listing::default();
    loop {
        for change in next_changes(&mut watch).await {
            apply(&mut listing, change);
        }
        for listed in &listing.sessions {
            if let Some(id) = &listed.worktree_id {
                assert!(
                    listing.worktrees.iter().any(|worktree| worktree.id == *id),
                    "session {} names worktree {id}, which is not listed",
                    listed.id
                );
            }
        }
        if listing
            .sessions
            .iter()
            .any(|listed| listed.id == session.id && listed.state == SessionState::Exited)
        {
            break;
        }
    }
    let error = exited(&client, &session).await;
    assert!(error.contains("worktree add"), "{error}");
    assert_eq!(info(&client, &session.id).await.unwrap().worktree_id, None);
    assert!(branch_exists(repo.path(), &branch));
    assert_eq!(git(&other, &["branch", "--show-current"]), branch);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_removed_during_a_creation() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let workspace = client.add_workspace(repo.path().to_owned()).await.unwrap();

    // Removed before the session is saved: the creation is refused and its
    // branch deleted.
    let blocked = BlockedGit::new(" branch --no-track ");
    let creating = new_session_with(&client, repo.path(), on_ox_branch(), blocked.env());
    let removing = async {
        blocked.blocked().await;
        client.remove_workspace(&workspace.id).await.unwrap();
        blocked.release().await;
    };
    let (refused, ()) = tokio::join!(creating, removing);
    let refused = refusal(refused);
    assert_eq!(refused.code, code::NOT_FOUND, "{refused:?}");
    assert_eq!(git(repo.path(), &["branch", "--list"]), "* main");
    assert!(sessions(&client).await.is_empty());
    assert!(workspaces(&client).await.is_empty());

    // Removed once the session is saved: the removal is refused.
    let workspace = client.add_workspace(repo.path().to_owned()).await.unwrap();
    let blocked = BlockedGit::new(" worktree add ");
    let session = new_session_with(&client, repo.path(), on_ox_branch(), blocked.env())
        .await
        .unwrap();
    blocked.blocked().await;
    let error = client.remove_workspace(&workspace.id).await.unwrap_err();
    let Error::Remote(refused) = error else {
        panic!("expected a daemon error, got {error}");
    };
    assert_eq!(refused.code, code::INVALID_STATE);
    blocked.release().await;
    started(&client, &session).await;
    assert_eq!(workspaces(&client).await, [workspace]);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_workspace_waits_for_an_unarchive_under_way() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    client
        .archive(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    let workspace = workspace_id(&client, repo.path()).await;

    let blocked = BlockedGit::new(" worktree add ");
    let unarchiving = client.unarchive(&session.id, blocked.env());
    let removing = async {
        blocked.blocked().await;
        let removal = client.remove_workspace(&workspace);
        tokio::pin!(removal);
        // Polling the removal once sends it.
        tokio::select! {
            biased;
            removed = &mut removal => panic!("removed during the unarchive: {removed:?}"),
            () = std::future::ready(()) => {}
        }
        // The daemon reads a connection's requests in order, so this listing
        // is answered after the removal arrived.
        assert_eq!(workspaces(&client).await.len(), 1);
        blocked.release().await;
        removal.await
    };
    let (unarchived, removed) = tokio::join!(unarchiving, removing);
    assert!(!unarchived.unwrap().archived);
    let refused = refusal(removed);
    assert_eq!(refused.code, code::INVALID_STATE, "{refused:?}");
    assert_eq!(workspaces(&client).await.len(), 1);
    assert!(!info(&client, &session.id).await.unwrap().archived);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_session_that_never_started_keeps_its_first_prompt_across_a_restart() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let workspace = workspace_id(&client, repo.path()).await;
    let session = client
        .new_session(
            &workspace,
            vec![FAKE_AGENT.into(), "--no-sessions".into()],
            BTreeMap::new(),
            on_ox_branch(),
            Vec::new(),
            Some(text("hello")),
        )
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::AgentExited { .. })
    })
    .await;
    drop(events);
    drop(client);

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    let restored = info(&client, &session.id).await.unwrap();
    assert_eq!(restored.state, SessionState::Exited);
    assert_eq!(restored.worktree_id, session.worktree_id);
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let first = next(&mut events).await;
    assert_eq!(
        first.kind,
        EventKind::PromptStarted {
            prompt: text("hello"),
            from: None,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn branches_name_the_branch_the_checkout_is_on() {
    let repo = repo();
    git(repo.path(), &["branch", "other"]);
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    let listed = branches(&client, repo.path()).await.unwrap();
    assert_eq!(listed.head.as_deref(), Some("main"));

    // A subdirectory of the checkout names the same head.
    std::fs::create_dir(repo.path().join("sub")).unwrap();
    let from_subdirectory = branches(&client, &repo.path().join("sub")).await.unwrap();
    assert_eq!(from_subdirectory, listed);

    // A detached checkout is on no branch.
    git(repo.path(), &["switch", "--quiet", "--detach"]);
    let detached = branches(&client, repo.path()).await.unwrap();
    assert_eq!(detached.head, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn branches_are_refused_outside_a_working_tree() {
    let plain = tempfile::tempdir().unwrap();
    let bare = tempfile::tempdir().unwrap();
    git(bare.path(), &["init", "--quiet", "--bare"]);
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    for path in [plain.path(), bare.path()] {
        let refused = refusal(branches(&client, path).await);
        assert_eq!(refused.code, code::INVALID_REQUEST, "{refused:?}");
        assert!(refused.message.contains("is not in a Git working tree"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn workspaces_say_whether_they_are_in_a_git_working_tree() {
    let repo = repo();
    std::fs::create_dir(repo.path().join("sub")).unwrap();
    let plain = tempfile::tempdir().unwrap();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    for path in [repo.path().to_owned(), repo.path().join("sub")] {
        let workspace = client.add_workspace(path).await.unwrap();
        assert!(workspace.git_working_tree, "{workspace:?}");
    }
    let workspace = client
        .add_workspace(repo.path().join(".git"))
        .await
        .unwrap();
    assert!(!workspace.git_working_tree);
    client.remove_workspace(&workspace.id).await.unwrap();
    let workspace = client.add_workspace(plain.path().to_owned()).await.unwrap();
    assert!(!workspace.git_working_tree);
    let flags = |changes: Vec<ListingChange>| {
        changes
            .into_iter()
            .filter_map(|change| match change {
                ListingChange::Workspace { workspace } => Some(workspace.git_working_tree),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut watch = client.watch().await.unwrap();
    assert_eq!(flags(next_changes(&mut watch).await), [true, true, false]);

    // The flag follows `git init` once the workspaces are listed again, such
    // as when a client connects.
    init(plain.path());
    let _connected = daemon.client().await;
    assert_eq!(flags(next_changes(&mut watch).await), [true]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_git_directory_git_rejects_is_refused_with_gits_reason() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".git"),
        format!("gitdir: {}\n", dir.path().join("missing").display()),
    )
    .unwrap();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let workspace = client.add_workspace(dir.path().to_owned()).await.unwrap();
    assert!(workspace.git_working_tree);

    // Git words the reason differently across versions; the refusal carries
    // whatever this git says.
    let output = git_command(dir.path(), &["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let reason = String::from_utf8(output.stderr).unwrap().trim().to_owned();
    assert!(!reason.is_empty());

    let refused = refusal(new_session(&client, dir.path(), on_ox_branch()).await);
    let listing = refusal(branches(&client, dir.path()).await);
    for refused in [refused, listing] {
        assert_eq!(refused.code, code::INVALID_REQUEST, "{refused:?}");
        assert!(
            refused
                .message
                .ends_with(&format!(" is not in a Git working tree: {reason}")),
            "{refused:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_delete_removes_the_worktree_and_the_branch() {
    let repo = repo();
    std::fs::write(repo.path().join(".gitignore"), "ignored\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "--quiet", "-m", "ignore"]);
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let mut agent = ExitWatch::new(daemon.dir.path());
    let session = started_holding(&client, repo.path(), &mut agent).await;
    let worktree = worktree_of(&client, &session).await;
    // Ignored files go with the worktree.
    std::fs::write(worktree.path.join("ignored"), "build output\n").unwrap();
    let status = git(repo.path(), &["status", "--porcelain", "--ignored"]);
    let head = git(repo.path(), &["rev-parse", "HEAD"]);

    let deleted = delete(&client, &session.id, false).await.unwrap();
    assert_eq!(deleted, Deleted { kept_branch: None });
    agent.exited().await;
    assert_removed(repo.path(), &worktree);
    assert!(!branch_exists(repo.path(), &worktree.branch));
    assert_eq!(git(repo.path(), &["branch", "--list"]), "* main");
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(repo.path(), &["status", "--porcelain", "--ignored"]),
        status
    );
    assert!(sessions(&client).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_merged_elsewhere_is_deleted_and_one_with_its_own_commits_is_kept() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    // Merged into `main`.
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    commit_file(&worktree.path, "merged");
    git(
        repo.path(),
        &["merge", "--quiet", "--ff-only", &worktree.branch],
    );
    let deleted = delete(&client, &session.id, false).await.unwrap();
    assert_eq!(deleted.kept_branch, None);
    assert_removed(repo.path(), &worktree);
    assert!(!branch_exists(repo.path(), &worktree.branch));

    // Its commit is on no other branch, also when the delete discards.
    for discard in [false, true] {
        let session = new_session(&client, repo.path(), on_ox_branch())
            .await
            .unwrap();
        let session = started(&client, &session).await;
        let worktree = worktree_of(&client, &session).await;
        commit_file(&worktree.path, if discard { "discarded" } else { "kept" });
        let tip = git(&worktree.path, &["rev-parse", "HEAD"]);
        let deleted = delete(&client, &session.id, discard).await.unwrap();
        assert_eq!(
            deleted.kept_branch.as_deref(),
            Some(worktree.branch.as_str())
        );
        assert_removed(repo.path(), &worktree);
        assert_eq!(git(repo.path(), &["rev-parse", &worktree.branch]), tip);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_is_kept_until_its_commits_are_pushed() {
    let repo = repo();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare"]);
    git(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    let daemon = Daemon::start().await;
    let client = daemon.client().await;

    for push in [true, false] {
        let session = new_session(&client, repo.path(), on_ox_branch())
            .await
            .unwrap();
        let session = started(&client, &session).await;
        let worktree = worktree_of(&client, &session).await;
        commit_file(&worktree.path, if push { "pushed" } else { "unpushed" });
        if push {
            git(
                &worktree.path,
                &["push", "--quiet", "origin", &worktree.branch],
            );
        }
        let deleted = delete(&client, &session.id, false).await.unwrap();
        assert_removed(repo.path(), &worktree);
        if push {
            assert_eq!(deleted.kept_branch, None);
            assert!(!branch_exists(repo.path(), &worktree.branch));
        } else {
            assert_eq!(
                deleted.kept_branch.as_deref(),
                Some(worktree.branch.as_str())
            );
            assert!(branch_exists(repo.path(), &worktree.branch));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn uncommitted_changes_refuse_the_delete_until_it_discards_them() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let mut agent = ExitWatch::new(daemon.dir.path());
    let session = started_holding(&client, repo.path(), &mut agent).await;
    let worktree = worktree_of(&client, &session).await;
    std::fs::write(worktree.path.join("untracked"), "work\n").unwrap();

    let refused = refusal(delete(&client, &session.id, false).await);
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert!(
        refused.message.contains("uncommitted changes")
            && refused.message.contains(worktree.path.to_str().unwrap()),
        "{refused:?}"
    );
    // Nothing was stopped or removed.
    answers(&client, &session.id).await;
    assert!(worktree.path.join("untracked").is_file());
    assert!(info(&client, &session.id).await.is_some());

    let deleted = delete(&client, &session.id, true).await.unwrap();
    assert_eq!(deleted.kept_branch, None);
    agent.exited().await;
    assert_removed(repo.path(), &worktree);
    assert!(!branch_exists(repo.path(), &worktree.branch));
    assert!(sessions(&client).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn commits_on_no_branch_refuse_the_delete() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    git(&worktree.path, &["switch", "--quiet", "--detach"]);
    commit_file(&worktree.path, "detached");

    let refused = refusal(delete(&client, &session.id, false).await);
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert!(
        refused.message.contains("commits on no branch"),
        "{refused:?}"
    );
    assert!(worktree.path.is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn submodule_commits_on_no_remote_branch_refuse_the_delete() {
    let submodule = repo();
    let repo = repo();
    git(
        repo.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            submodule.path().to_str().unwrap(),
            "sub",
        ],
    );
    git(repo.path(), &["commit", "--quiet", "-m", "submodule"]);
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let start = async || {
        let session = new_session(&client, repo.path(), on_ox_branch())
            .await
            .unwrap();
        let session = started(&client, &session).await;
        let worktree = worktree_of(&client, &session).await;
        git(
            &worktree.path,
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--quiet",
                "--init",
            ],
        );
        (session, worktree)
    };

    // Initialized and clean.
    let (session, worktree) = start().await;
    delete(&client, &session.id, false).await.unwrap();
    assert_removed(repo.path(), &worktree);

    // A commit in the submodule that its remote does not have, recorded in
    // the session's branch, so nothing else is unsaved.
    let (session, worktree) = start().await;
    let inner = worktree.path.join("sub");
    identify(&inner);
    commit_file(&inner, "unpushed");
    git(&worktree.path, &["commit", "--quiet", "-am", "bump"]);
    let refused = refusal(delete(&client, &session.id, false).await);
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert_eq!(
        refused.message,
        "deleting the worktree would lose submodule sub has commits on no remote branch"
    );
    assert!(inner.is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_deleted_outside_ox_is_unregistered() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    std::fs::remove_dir_all(&worktree.path).unwrap();
    assert!(
        worktree_paths(repo.path())
            .contains(&canonical(&daemon.worktrees()).join(worktree.path.file_name().unwrap()))
    );

    let deleted = delete(&client, &session.id, false).await.unwrap();
    assert_eq!(deleted.kept_branch, None);
    assert_removed(repo.path(), &worktree);
    assert!(!branch_exists(repo.path(), &worktree.branch));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_whose_repository_moved_is_removed_only_with_discard() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("repo");
    std::fs::create_dir(&path).unwrap();
    init(&path);
    commit_file(&path, "README");
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, &path, on_ox_branch()).await.unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    std::fs::rename(&path, parent.path().join("moved")).unwrap();

    let refused = refusal(delete(&client, &session.id, false).await);
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert!(refused.message.contains("repository"), "{refused:?}");
    assert!(worktree.path.is_dir());

    // Ox cannot check the branch in a repository it cannot find, and the
    // branch is still there in the moved one.
    let deleted = delete(&client, &session.id, true).await.unwrap();
    assert_eq!(
        deleted.kept_branch.as_deref(),
        Some(worktree.branch.as_str())
    );
    assert!(!worktree.path.exists());
    assert!(sessions(&client).await.is_empty());
    assert!(branch_exists(
        &parent.path().join("moved"),
        &worktree.branch
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_locked_worktree_fails_the_delete_and_the_session_loads_again() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    // The fake agent resumes a session from the log a prompt writes in the
    // worktree, which the delete has to discard.
    answers(&client, &session.id).await;
    git(
        repo.path(),
        &["worktree", "lock", worktree.path.to_str().unwrap()],
    );

    let refused = refusal(delete(&client, &session.id, true).await);
    assert_eq!(refused.code, code::GIT, "{refused:?}");
    let after = info(&client, &session.id).await.unwrap();
    assert_eq!(after.state, SessionState::Exited);
    assert!(worktree.path.is_dir());

    git(
        repo.path(),
        &["worktree", "unlock", worktree.path.to_str().unwrap()],
    );
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    until_state(&client, &session.id, SessionState::Idle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_another_worktree_has_checked_out_is_kept() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    git(&worktree.path, &["switch", "--quiet", "--detach"]);
    let other = tempfile::tempdir().unwrap();
    let other = other.path().join("other");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            "--quiet",
            other.to_str().unwrap(),
            &worktree.branch,
        ],
    );

    let deleted = delete(&client, &session.id, false).await.unwrap();
    assert_eq!(
        deleted.kept_branch.as_deref(),
        Some(worktree.branch.as_str())
    );
    assert_removed(repo.path(), &worktree);
    assert_eq!(git(&other, &["branch", "--show-current"]), worktree.branch);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delete_during_startup_waits_for_the_worktree() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let blocked = BlockedGit::new(" worktree add ");
    let session = new_session_with(&client, repo.path(), on_ox_branch(), blocked.env())
        .await
        .unwrap();
    let worktree = worktree_of(&client, &session).await;
    blocked.blocked().await;

    // The daemon reads a connection's requests in order and answers a watch
    // at once, so a watch answered after the delete was sent shows that the
    // delete arrived while the startup was stopped.
    let releasing = async {
        sessions(&client).await;
        blocked.release().await;
    };
    let (deleted, ()) = tokio::join!(delete(&client, &session.id, false), releasing);
    assert_eq!(deleted.unwrap().kept_branch, None);
    assert_removed(repo.path(), &worktree);
    assert!(!branch_exists(repo.path(), &worktree.branch));
    assert!(sessions(&client).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_workspace_subdirectory_is_gone_is_deleted() {
    let repo = repo();
    std::fs::create_dir(repo.path().join("sub")).unwrap();
    std::fs::write(repo.path().join("sub/file"), "in sub\n").unwrap();
    git(repo.path(), &["add", "sub"]);
    git(repo.path(), &["commit", "--quiet", "-m", "sub"]);
    let sub = canonical(&repo.path().join("sub"));
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let session = new_session(&client, &sub, on_ox_branch()).await.unwrap();
        sessions.push(started(&client, &session).await);
    }
    let untouched = worktree_of(&client, &sessions[0]).await;
    let committed = worktree_of(&client, &sessions[1]).await;
    commit_file(&committed.path, "work");
    std::fs::remove_dir_all(&sub).unwrap();

    let deleted = delete(&client, &sessions[0].id, false).await.unwrap();
    assert_eq!(deleted.kept_branch, None);
    assert_removed(repo.path(), &untouched);
    assert!(!branch_exists(repo.path(), &untouched.branch));

    let deleted = delete(&client, &sessions[1].id, false).await.unwrap();
    assert_eq!(
        deleted.kept_branch.as_deref(),
        Some(committed.branch.as_str())
    );
    assert_removed(repo.path(), &committed);
    assert!(branch_exists(repo.path(), &committed.branch));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worktree_whose_registration_was_removed_is_removed_only_with_discard() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    let admin = git(&worktree.path, &["rev-parse", "--absolute-git-dir"]);
    std::fs::remove_dir_all(admin).unwrap();

    let refused = refusal(delete(&client, &session.id, false).await);
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert!(
        refused.message.contains("no longer recognizes"),
        "{refused:?}"
    );
    assert!(worktree.path.is_dir());

    delete(&client, &session.id, true).await.unwrap();
    assert!(!worktree.path.exists());
    assert!(sessions(&client).await.is_empty());
}

async fn archive(client: &Client, session_id: &str, discard: bool) -> Result<SessionInfo, Error> {
    timeout(
        TIMEOUT,
        client.archive(session_id, discard, BTreeMap::new()),
    )
    .await
    .expect("timed out archiving the session")
}

async fn unarchive(client: &Client, session_id: &str) -> Result<SessionInfo, Error> {
    timeout(TIMEOUT, client.unarchive(session_id, BTreeMap::new()))
        .await
        .expect("timed out unarchiving the session")
}

/// Subscribes to the session after `seq` and returns its events up to the one
/// `last` accepts.
async fn events_until(
    client: &Client,
    session_id: &str,
    seq: u64,
    last: impl Fn(&EventKind) -> bool,
) -> Vec<ox_protocol::Event> {
    let mut events = client.subscribe(session_id, seq).await.unwrap();
    until(&mut events, last).await
}

/// A session of the fake agent in a new worktree that has had one turn, with
/// the agent's transcript committed on its branch so that a resumed agent
/// finds it in the restored worktree.
async fn session_with_a_turn(
    client: &Client,
    repo: &Path,
) -> (SessionInfo, Vec<ox_protocol::Event>) {
    let session = new_session(client, repo, on_ox_branch()).await.unwrap();
    let session = started(client, &session).await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    client.prompt(&session.id, text("say 1 0")).await.unwrap();
    let seen = until(&mut events, turn_ended).await;
    let worktree = worktree_of(client, &session).await;
    git(&worktree.path, &["add", "."]);
    git(&worktree.path, &["commit", "--quiet", "-m", "transcript"]);
    (session, seen)
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_removes_the_worktree_and_keeps_the_branch_and_the_conversation() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, turn) = session_with_a_turn(&client, repo.path()).await;
    let worktree = worktree_of(&client, &session).await;

    let archived = archive(&client, &session.id, false).await.unwrap();
    assert!(archived.archived);
    assert_eq!(archived.state, SessionState::Exited);
    assert_removed(repo.path(), &worktree);
    assert!(branch_exists(repo.path(), &worktree.branch));
    assert!(info(&client, &session.id).await.unwrap().archived);

    let parked = |kind: &EventKind| matches!(kind, EventKind::AgentParked);
    let history = events_until(&client, &session.id, 0, parked).await;
    assert!(history.len() > turn.len());
    assert_eq!(history[..turn.len()], turn);
    drop(client);

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    let restored = info(&client, &session.id).await.unwrap();
    assert!(restored.archived);
    assert_eq!(restored.state, SessionState::Exited);
    assert_eq!(events_until(&client, &session.id, 0, parked).await, history);
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Exited
    );

    // An archived session cannot run.
    let refused = refusal(client.load_session(&session.id, BTreeMap::new()).await);
    assert_eq!(refused.code, code::INVALID_STATE);
    assert!(refused.message.contains("archived"), "{refused:?}");
    refusal(client.prompt(&session.id, text("say 1 0")).await);
    let refused = refusal(archive(&client, &session.id, false).await);
    assert_eq!(refused.code, code::INVALID_STATE);

    // Unarchiving brings the worktree back on its branch, and the session
    // resumes without replaying its history.
    let unarchived = unarchive(&client, &session.id).await.unwrap();
    assert!(!unarchived.archived);
    assert_eq!(unarchived.state, SessionState::Exited);
    assert_eq!(
        git(&worktree.path, &["branch", "--show-current"]),
        worktree.branch
    );
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let last = history.last().unwrap().seq;
    let resumed = events_until(&client, &session.id, last, |kind| {
        matches!(
            kind,
            EventKind::AgentStarted { .. } | EventKind::AgentExited { .. }
        )
    })
    .await;
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    assert!(matches!(resumed[0].kind, EventKind::AgentStarted { .. }));
    let next = resumed[0].seq;
    let mut events = client.subscribe(&session.id, next).await.unwrap();
    client.prompt(&session.id, text("say 1 0")).await.unwrap();
    let ended = until(&mut events, turn_ended).await.pop().unwrap();
    assert!(
        matches!(ended.kind, EventKind::TurnEnded { error: None, .. }),
        "{ended:?}"
    );

    let refused = refusal(unarchive(&client, &session.id).await);
    assert_eq!(refused.code, code::INVALID_STATE);
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_refuses_unsaved_work_unless_it_is_discarded() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let mut agent = ExitWatch::new(daemon.dir.path());
    let session = started_holding(&client, repo.path(), &mut agent).await;
    let worktree = worktree_of(&client, &session).await;
    std::fs::write(worktree.path.join("unsaved"), "work\n").unwrap();

    let refused = refusal(archive(&client, &session.id, false).await);
    assert_eq!(refused.code, code::UNSAVED_WORK);
    assert!(refused.message.contains("uncommitted"), "{refused:?}");
    assert!(worktree.path.join("unsaved").exists());
    let listed = info(&client, &session.id).await.unwrap();
    assert!(!listed.archived);
    assert_eq!(listed.state, SessionState::Idle);

    let archived = archive(&client, &session.id, true).await.unwrap();
    assert!(archived.archived);
    agent.exited().await;
    assert_removed(repo.path(), &worktree);
    assert!(branch_exists(repo.path(), &worktree.branch));
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_a_running_session_ends_its_turn() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    client.prompt(&session.id, text("wait")).await.unwrap();
    until_state(&client, &session.id, SessionState::Running).await;

    archive(&client, &session.id, true).await.unwrap();
    let ended = until(&mut events, turn_ended).await.pop().unwrap();
    let EventKind::TurnEnded { error, .. } = ended.kind else {
        unreachable!("`until` stops at the turn's end");
    };
    assert_eq!(error.as_deref(), Some("the agent was stopped"));
}

#[tokio::test(flavor = "multi_thread")]
async fn unarchiving_needs_a_branch_git_can_check_out() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    for deleted in [true, false] {
        let session = new_session(&client, repo.path(), on_ox_branch())
            .await
            .unwrap();
        let session = started(&client, &session).await;
        let worktree = worktree_of(&client, &session).await;
        archive(&client, &session.id, true).await.unwrap();
        if deleted {
            git(repo.path(), &["branch", "-D", &worktree.branch]);
        } else {
            git(repo.path(), &["checkout", "--quiet", &worktree.branch]);
        }

        let refused = refusal(unarchive(&client, &session.id).await);
        assert_eq!(refused.code, code::GIT, "{refused:?}");
        assert!(info(&client, &session.id).await.unwrap().archived);
        assert!(!worktree.path.exists());
        git(repo.path(), &["checkout", "--quiet", "main"]);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unarchiving_again_finishes_when_the_worktree_is_already_back() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    archive(&client, &session.id, true).await.unwrap();
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            worktree.path.to_str().unwrap(),
            &worktree.branch,
        ],
    );

    let unarchived = unarchive(&client, &session.id).await.unwrap();
    assert!(!unarchived.archived);
    assert_eq!(
        git(&worktree.path, &["branch", "--show-current"]),
        worktree.branch
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_archived_session_removes_it_and_applies_the_branch_rule() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = worktree_of(&client, &session).await;
    archive(&client, &session.id, true).await.unwrap();

    let deleted = delete(&client, &session.id, false).await.unwrap();
    assert_eq!(deleted, Deleted { kept_branch: None });
    assert!(info(&client, &session.id).await.is_none());
    assert!(!branch_exists(repo.path(), &worktree.branch));
    assert_removed(repo.path(), &worktree);
}

/// A new worktree on the session's `ox/<id8>` branch, compared with
/// `reference`.
fn compared_with(reference: &str) -> Checkout {
    Checkout::Worktree {
        compare: Some(reference.into()),
        branch: None,
    }
}

/// Moves `branch` of the workspace for `path` from `expected` to `to`, from
/// a source branch at `to`, with git run with `env`.
async fn move_branch(
    client: &Client,
    path: &Path,
    branch: &str,
    to: &str,
    expected: &str,
    env: BTreeMap<String, String>,
) -> Result<(), Error> {
    git(path, &["branch", "--force", "source", to]);
    let workspace = workspace_id(client, path).await;
    timeout(
        TIMEOUT,
        client.fast_forward_branch(&workspace, branch, expected, "source", to, env),
    )
    .await
    .expect("timed out moving the branch")
}

/// The commit `rev` names in the repository at `dir`.
fn commit_of(dir: &Path, rev: &str) -> String {
    git(dir, &["rev-parse", &format!("{rev}^{{commit}}")])
}

/// A commit with the tree of `parent` and the file `name` added, on no
/// branch, made without touching the checkout at `dir`.
fn commit_beside(dir: &Path, parent: &str, name: &str) -> String {
    let blob = {
        let mut child = git_command(dir, &["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(
            child.stdin.as_mut().unwrap(),
            format!("{name}\n").as_bytes(),
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let index = tempfile::NamedTempFile::new().unwrap();
    let index = index.path().to_str().unwrap();
    let with_index = |args: &[&str]| {
        let output = git_command(dir, args)
            .env("GIT_INDEX_FILE", index)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    with_index(&["read-tree", parent]);
    with_index(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{blob},{name}"),
    ]);
    let tree = with_index(&["write-tree"]);
    git(dir, &["commit-tree", &tree, "-p", parent, "-m", name])
}

/// Waits until a watch delivers the session's worktree with a comparison
/// `done` accepts, and returns the worktree.
async fn until_compared(
    client: &Client,
    session: &SessionInfo,
    done: impl Fn(&Comparison) -> bool,
) -> WorktreeInfo {
    let id = session
        .worktree_id
        .as_deref()
        .expect("the session has a worktree");
    let mut watch = client.watch().await.unwrap();
    loop {
        for change in next_changes(&mut watch).await {
            if let ListingChange::Worktree { worktree } = change
                && worktree.id == id
                && worktree.comparison.as_ref().is_some_and(&done)
            {
                return worktree;
            }
        }
    }
}

/// Whether the comparison counts `ahead` and `behind`.
fn counts(ahead: u64, behind: u64) -> impl Fn(&Comparison) -> bool {
    move |comparison| {
        matches!(comparison, Comparison::Compared { ahead: a, behind: b, .. }
            if *a == ahead && *b == behind)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_no_checkout_has_moves_only_from_where_it_is_expected() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = commit_of(repo.path(), "HEAD");
    git(repo.path(), &["branch", "topic"]);
    commit_file(repo.path(), "second");
    let second = commit_of(repo.path(), "HEAD");

    let refused = refusal(
        move_branch(
            &client,
            repo.path(),
            "topic",
            "main",
            &second,
            BTreeMap::new(),
        )
        .await,
    );
    assert_eq!(refused.code, code::INVALID_STATE, "{refused:?}");
    assert_eq!(commit_of(repo.path(), "topic"), first);

    move_branch(
        &client,
        repo.path(),
        "topic",
        "main",
        &first,
        BTreeMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(commit_of(repo.path(), "topic"), second);

    let side = commit_beside(repo.path(), &first, "side");
    let refused = refusal(
        move_branch(
            &client,
            repo.path(),
            "topic",
            &side,
            &second,
            BTreeMap::new(),
        )
        .await,
    );
    assert_eq!(refused.code, code::INVALID_REQUEST, "{refused:?}");
    assert_eq!(commit_of(repo.path(), "topic"), second);
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_a_checked_out_branch_moves_its_checkout() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");

    move_branch(
        &client,
        repo.path(),
        "main",
        &topic,
        &first,
        BTreeMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(commit_of(repo.path(), "HEAD"), topic);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("added")).unwrap(),
        "added\n"
    );
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_keeps_uncommitted_and_untracked_work() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");

    std::fs::write(repo.path().join("README"), "changed\n").unwrap();
    let refused = refusal(
        move_branch(
            &client,
            repo.path(),
            "main",
            &topic,
            &first,
            BTreeMap::new(),
        )
        .await,
    );
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert_eq!(commit_of(repo.path(), "main"), first);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("README")).unwrap(),
        "changed\n"
    );
    git(repo.path(), &["checkout", "README"]);

    // An untracked file where the move adds one fails it in git, and the
    // branch goes back.
    std::fs::write(repo.path().join("added"), "mine\n").unwrap();
    let refused = refusal(
        move_branch(
            &client,
            repo.path(),
            "main",
            &topic,
            &first,
            BTreeMap::new(),
        )
        .await,
    );
    assert_eq!(refused.code, code::GIT, "{refused:?}");
    assert_eq!(commit_of(repo.path(), "main"), first);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("added")).unwrap(),
        "mine\n"
    );
    std::fs::remove_file(repo.path().join("added")).unwrap();
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "");

    // An untracked file the move leaves alone does not stop it.
    std::fs::write(repo.path().join("notes"), "notes\n").unwrap();
    move_branch(
        &client,
        repo.path(),
        "main",
        &topic,
        &first,
        BTreeMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(commit_of(repo.path(), "HEAD"), topic);
    assert!(repo.path().join("added").exists());
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), "?? notes");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_keeps_an_ignored_file_it_would_overwrite() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    std::fs::write(repo.path().join(".gitignore"), "added\n").unwrap();
    git(repo.path(), &["add", ".gitignore"]);
    git(repo.path(), &["commit", "--quiet", "-m", "ignore"]);
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");
    std::fs::write(repo.path().join("added"), "mine\n").unwrap();

    let refused = refusal(
        move_branch(
            &client,
            repo.path(),
            "main",
            &topic,
            &first,
            BTreeMap::new(),
        )
        .await,
    );
    assert_eq!(refused.code, code::UNSAVED_WORK, "{refused:?}");
    assert!(refused.message.contains("added"), "{refused:?}");
    assert_eq!(commit_of(repo.path(), "main"), first);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("added")).unwrap(),
        "mine\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_waits_for_an_agent_in_the_checkout_to_end_its_turn() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");
    let session = new_session(&client, repo.path(), Checkout::Workspace)
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    client.prompt(&session.id, text("wait")).await.unwrap();
    until_state(&client, &session.id, SessionState::Running).await;

    let refused = refusal(
        move_branch(
            &client,
            repo.path(),
            "main",
            &topic,
            &first,
            BTreeMap::new(),
        )
        .await,
    );
    assert_eq!(refused.code, code::INVALID_STATE, "{refused:?}");
    assert!(refused.message.contains(&session.id), "{refused:?}");
    assert_eq!(commit_of(repo.path(), "main"), first);

    client.cancel(&session.id).await.unwrap();
    until(&mut events, turn_ended).await;
    move_branch(
        &client,
        repo.path(),
        "main",
        &topic,
        &first,
        BTreeMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(commit_of(repo.path(), "HEAD"), topic);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_prompt_waits_for_a_branch_move_in_its_checkout() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");
    let session = new_session(&client, repo.path(), Checkout::Workspace)
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();

    let blocked = BlockedGit::new(" update-ref ");
    let moving = move_branch(&client, repo.path(), "main", &topic, &first, blocked.env());
    let during = async {
        blocked.blocked().await;
        let refused = refusal(client.prompt(&session.id, text("say 1 0")).await);
        assert_eq!(refused.code, code::INVALID_STATE, "{refused:?}");
        assert!(refused.message.contains("branch move"), "{refused:?}");
        client
            .queue_add(&session.id, text("say 1 0"))
            .await
            .unwrap();
        // Read after the queued prompt on the same connection.
        let listed = info(&client, &session.id).await.unwrap();
        assert_eq!(listed.state, SessionState::Idle);
        assert_eq!(listed.queue.len(), 1);
        blocked.release().await;
    };
    let (moved, ()) = tokio::join!(moving, during);
    moved.unwrap();
    let started = until(&mut events, |kind| {
        matches!(kind, EventKind::PromptStarted { .. })
    })
    .await;
    assert_eq!(
        started.last().unwrap().kind,
        EventKind::PromptStarted {
            prompt: text("say 1 0"),
            from: None,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_imported_during_a_branch_move_refuses_loads_until_it_ends() {
    let repo = repo();
    std::fs::write(
        repo.path().join(".git/info/exclude"),
        "fake-transcripts.json\n",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("fake-transcripts.json"),
        r#"[{"sessionId": "a", "startedAt": 1000, "title": "Session a", "updates": []}]"#,
    )
    .unwrap();
    let daemon = Daemon::start_with(Options {
        bridges: true,
        ..Options::default()
    })
    .await;
    let client = daemon.client().await;
    let workspace = workspace_id(&client, repo.path()).await;
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");

    let blocked = BlockedGit::new(" update-ref ");
    let moving = move_branch(&client, repo.path(), "main", &topic, &first, blocked.env());
    let during = async {
        blocked.blocked().await;
        let imported = client
            .import_claude_sessions(&workspace, BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(imported.imported, 1);
        let session = sessions(&client).await.pop().unwrap();
        client
            .unarchive(&session.id, BTreeMap::new())
            .await
            .unwrap();
        let refused = refusal(client.load_session(&session.id, BTreeMap::new()).await);
        assert_eq!(refused.code, code::INVALID_STATE, "{refused:?}");
        assert!(refused.message.contains("branch move"), "{refused:?}");
        blocked.release().await;
        session
    };
    let (moved, session) = tokio::join!(moving, during);
    moved.unwrap();
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_session_publishes_its_comparison() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let head = commit_of(repo.path(), "HEAD");

    let refused = refusal(new_session(&client, repo.path(), compared_with("nothing")).await);
    assert_eq!(refused.code, code::INVALID_REQUEST, "{refused:?}");
    assert!(sessions(&client).await.is_empty());
    assert_eq!(
        git(repo.path(), &["branch", "--format=%(refname:short)"]),
        "main"
    );

    let session = new_session(&client, repo.path(), compared_with("main"))
        .await
        .unwrap();
    let worktree = until_compared(&client, &session, counts(0, 0)).await;
    assert_eq!(worktree.compare.as_deref(), Some("main"));
    assert_eq!(
        worktree.comparison,
        Some(Comparison::Compared {
            tip: head.clone(),
            compare_tip: head,
            ahead: 0,
            behind: 0,
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_end_compares_again() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), compared_with("main"))
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree = until_compared(&client, &session, counts(0, 0)).await;
    commit_file(&worktree.path, "work");

    answers(&client, &session.id).await;
    let worktree = until_compared(&client, &session, counts(1, 0)).await;
    let Some(Comparison::Compared { tip, .. }) = &worktree.comparison else {
        unreachable!("until_compared returns a compared worktree");
    };
    assert_eq!(*tip, commit_of(&worktree.path, "HEAD"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_compares_the_repositorys_sessions_again() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = commit_of(repo.path(), "HEAD");
    let topic = commit_beside(repo.path(), &first, "added");
    let session = new_session(&client, repo.path(), compared_with("main"))
        .await
        .unwrap();
    // The session's startup runs git in the repository too, so it finishes
    // before the test changes the repository.
    let session = started(&client, &session).await;
    until_compared(&client, &session, counts(0, 0)).await;

    move_branch(
        &client,
        repo.path(),
        "main",
        &topic,
        &first,
        BTreeMap::new(),
    )
    .await
    .unwrap();
    until_compared(&client, &session, counts(0, 1)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delete_compares_the_repositorys_sessions_again() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let first = new_session(&client, repo.path(), on_ox_branch())
        .await
        .unwrap();
    let first = started(&client, &first).await;
    let branch = worktree_of(&client, &first).await.branch.clone();
    let second = new_session(&client, repo.path(), compared_with(&branch))
        .await
        .unwrap();
    let second = started(&client, &second).await;
    until_compared(&client, &second, counts(0, 0)).await;

    let deleted = delete(&client, &first.id, false).await.unwrap();
    assert_eq!(deleted, Deleted { kept_branch: None });
    let second = until_compared(&client, &second, |comparison| {
        matches!(comparison, Comparison::Failed { .. })
    })
    .await;
    assert_eq!(
        second.comparison,
        Some(Comparison::Failed {
            error: format!("{branch} is not a commit")
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_comparison_is_refreshed_on_request() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), compared_with("main"))
        .await
        .unwrap();
    let session = started(&client, &session).await;
    let worktree_id = until_compared(&client, &session, counts(0, 0)).await.id;

    commit_file(repo.path(), "outside");
    let refreshed = client.refresh(&worktree_id).await.unwrap();
    assert!(
        refreshed.comparison.as_ref().is_some_and(counts(0, 1)),
        "{:?}",
        refreshed.comparison
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_comparison_ref_survives_a_restart_and_is_compared_unasked() {
    let repo = repo();
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = new_session(&client, repo.path(), compared_with("main"))
        .await
        .unwrap();
    started(&client, &session).await;
    until_compared(&client, &session, counts(0, 0)).await;
    drop(client);
    commit_file(repo.path(), "meanwhile");

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    let restored = until_compared(&client, &session, counts(0, 1)).await;
    assert_eq!(restored.compare.as_deref(), Some("main"));
}
