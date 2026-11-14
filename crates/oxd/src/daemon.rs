use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ox_protocol::{
    Branches, Checkout, Deleted, ErrorBody, Listing, ListingChange, Method, SessionInfo,
    SessionState, WorkspaceInfo, code,
};
use serde_json::Value;
use tokio::sync::{OwnedMutexGuard, watch};

use crate::branches;
use crate::directories::Directories;
use crate::mcp::OxServer;
use crate::session::Session;
use crate::state::Change;
use crate::store::{SavedSession, SavedWorkspace, SavedWorktree, Store};
use crate::worktree::{self, Git, WorktreePlan};

/// The outcome of a fetch, once it has one.
type FetchOutcome = Option<Result<(), String>>;

struct Registry {
    workspaces: Vec<SavedWorkspace>,
    sessions: HashMap<String, Arc<Session>>,
    clients: usize,
    moving_checkouts: HashSet<PathBuf>,
    /// Whether the startup cleanup is still running.
    cleaning: bool,
    /// Counts the changes to the registry.
    version: u64,
}

pub struct Daemon {
    registry: Mutex<Registry>,
    store: Arc<Store>,
    /// The directory that holds sessions' worktrees.
    worktrees_dir: PathBuf,
    /// How long a session stays idle, with no background work, and unviewed
    /// before its agent is parked.
    park_after: Duration,
    /// Sent when a workspace or session is added or removed, a client
    /// connects or disconnects, or a session's `SessionInfo` changes.
    changed: watch::Sender<()>,
    /// The fetches running, by the common Git directory of the repository
    /// each fetches, so that requests for one repository share one fetch.
    fetches: Arc<Mutex<HashMap<PathBuf, watch::Receiver<FetchOutcome>>>>,
    /// The sessions that never started and have a worktree to remove, each
    /// with its lifecycle guard, until `clean_up` takes them.
    unstarted: Mutex<Vec<(Arc<Session>, OwnedMutexGuard<()>)>>,
    /// The directories that directory watches follow.
    directories: Arc<Directories>,
    /// The `ox` MCP server each session's agent is given.
    ox_server: Arc<OxServer>,
}

/// What a watch has sent: the workspaces, as of a version of the registry,
/// and the version of each session's info.
#[derive(Default)]
pub struct Sent {
    registry_version: Option<u64>,
    workspaces: Vec<WorkspaceInfo>,
    sessions: HashMap<String, u64>,
}

/// Counts one connected client for as long as it lives.
pub struct ClientGuard(Arc<Daemon>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.update(|registry| registry.clients -= 1);
    }
}

struct CheckoutMove<'a> {
    daemon: &'a Daemon,
    checkout: PathBuf,
}

impl Drop for CheckoutMove<'_> {
    fn drop(&mut self) {
        self.daemon.lock().moving_checkouts.remove(&self.checkout);
    }
}

impl Registry {
    fn refuse_moving(&self, cwd: &Path) -> Result<(), ErrorBody> {
        if self.moving_checkouts.is_empty() {
            return Ok(());
        }
        let cwd = resolved(cwd);
        if self
            .moving_checkouts
            .iter()
            .any(|checkout| cwd.starts_with(checkout))
        {
            return Err(invalid_state(
                "a branch move is in progress in this checkout",
            ));
        }
        Ok(())
    }
}

impl Daemon {
    /// A daemon with the workspaces and sessions `store` holds, which adds
    /// sessions' worktrees in `worktrees_dir`, and parks the agent of a session
    /// that has been idle, with no background work, and unviewed for
    /// `park_after`, whose directory watches follow `directories`, and whose
    /// sessions' agents are given `ox_server`. Each saved session is
    /// `exited`, with no agent, until it loads. A saved session that never
    /// started has nothing to load. One without a worktree is deleted; one
    /// with a worktree waits, `exited`, for `clean_up`.
    pub fn new(
        store: Store,
        worktrees_dir: PathBuf,
        park_after: Duration,
        directories: Arc<Directories>,
        ox_server: Arc<OxServer>,
    ) -> rusqlite::Result<Self> {
        let store = Arc::new(store);
        let changed = watch::Sender::new(());
        let mut sessions = HashMap::new();
        let mut unstarted = Vec::new();
        let saved_sessions = store.sessions()?;
        for saved in saved_sessions {
            let started = saved.agent_session_id.is_some();
            if !started && saved.worktree.is_none() {
                store.remove_session(&saved.id)?;
                continue;
            }
            let session = Session::restore(
                store.clone(),
                changed.clone(),
                park_after,
                ox_server.clone(),
                saved,
            );
            if !started {
                let guard = session.hold_lifecycle();
                unstarted.push((session.clone(), guard));
            }
            sessions.insert(session.id.clone(), session);
        }
        let registry = Registry {
            workspaces: store.workspaces()?,
            sessions,
            clients: 0,
            moving_checkouts: HashSet::new(),
            cleaning: !unstarted.is_empty(),
            version: 0,
        };
        Ok(Self {
            registry: Mutex::new(registry),
            store,
            worktrees_dir,
            park_after,
            changed,
            fetches: Arc::default(),
            unstarted: Mutex::new(unstarted),
            directories,
            ox_server,
        })
    }

    /// Removes the worktrees and branches of the sessions that never
    /// started, one at a time, with the daemon's own environment, as a delete
    /// without `discard` does, and deletes those sessions. A session whose
    /// worktree would lose work, or cannot be removed, stays to be deleted by
    /// the user. The daemon is not idle until this ends.
    pub async fn clean_up(&self) {
        let unstarted = std::mem::take(&mut *self.unstarted.lock().unwrap());
        for (session, _lifecycle) in unstarted {
            let worktree = session
                .worktree()
                .expect("an unstarted session waiting for cleanup has a worktree");
            match remove_unstarted(&worktree).await {
                Ok(()) => {
                    self.recompare(&worktree.common_dir);
                    if let Err(error) = self.forget(&session.id) {
                        eprintln!(
                            "oxd: deleting session {}, which never started: {}",
                            session.id, error.message
                        );
                        continue;
                    }
                    session.close();
                }
                Err(error) => eprintln!(
                    "oxd: keeping session {}, which never started: {}",
                    session.id, error.message
                ),
            }
        }
        self.update(|registry| registry.cleaning = false);
    }

    pub fn client(self: &Arc<Self>) -> ClientGuard {
        self.update(|registry| registry.clients += 1);
        ClientGuard(self.clone())
    }

    /// Resolves once the daemon has had no clients and no running agents for
    /// `idle_exit`. A session whose agent is not running, `exited`, is saved
    /// and does not keep the daemon alive.
    pub async fn idle(&self, idle_exit: Duration) {
        let mut changed = self.changed.subscribe();
        loop {
            // Marking the change seen before reading the registry means a
            // change made while it is read wakes this again.
            changed.borrow_and_update();
            if self.is_idle() {
                tokio::select! {
                    () = tokio::time::sleep(idle_exit) => return,
                    _ = changed.changed() => {}
                }
            } else {
                let _ = changed.changed().await;
            }
        }
    }

    fn is_idle(&self) -> bool {
        let registry = self.lock();
        !registry.cleaning
            && registry.clients == 0
            && registry
                .sessions
                .values()
                .all(|session| session.state() == SessionState::Exited)
    }

    /// Stops every agent. The sessions stay saved.
    pub fn close_all(&self) {
        let mut sessions = Vec::new();
        self.update(|registry| sessions.extend(registry.sessions.drain().map(|(_, s)| s)));
        for session in sessions {
            session.close();
        }
    }

    /// Returns once every recorded event is in the database.
    pub async fn flush(&self) {
        self.store.flush().await;
    }

    /// A receiver that sees a change each time the lists may have changed.
    pub fn changed(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub fn directories(&self) -> &Arc<Directories> {
        &self.directories
    }

    pub fn session(&self, id: &str) -> Result<Arc<Session>, ErrorBody> {
        self.lock()
            .sessions
            .get(id)
            .cloned()
            .ok_or_else(|| not_found("session", id))
    }

    /// Handles every method except `hello`, `session.subscribe`, `watch`, and
    /// `unsubscribe`, which belong to the connection.
    pub async fn handle(&self, method: Method) -> Result<Value, ErrorBody> {
        match method {
            Method::SessionList => Ok(to_value(self.list().sessions)),
            Method::WorkspaceAdd { path } => Ok(to_value(self.add_workspace(path)?)),
            Method::WorkspaceRemove { workspace_id } => {
                self.remove_workspace(&workspace_id)?;
                Ok(Value::Null)
            }
            Method::WorkspaceList => Ok(to_value(self.list().workspaces)),
            Method::WorkspaceBranches {
                workspace_id,
                fetch,
                env,
            } => Ok(to_value(self.branches(&workspace_id, fetch, env).await?)),
            Method::SessionNew {
                workspace_id,
                command,
                env,
                checkout,
            } => Ok(to_value(
                self.new_session(&workspace_id, command, env, checkout)
                    .await?,
            )),
            Method::SessionLoad { session_id, env } => {
                let session = self.session(&session_id)?;
                let _lifecycle = self.lifecycle(&session).await?;
                session.history().await?;
                let registry = self.lock();
                registry.refuse_moving(session.cwd())?;
                session.load(env)?;
                Ok(to_value(session.info()))
            }
            Method::SessionPrompt { session_id, prompt } => {
                let registry = self.lock();
                let session = registry
                    .sessions
                    .get(&session_id)
                    .ok_or_else(|| not_found("session", &session_id))?;
                registry.refuse_moving(session.cwd())?;
                session.prompt(prompt)?;
                Ok(Value::Null)
            }
            Method::SessionSetConfigOption {
                session_id,
                config_id,
                value,
            } => {
                let session = self.session(&session_id)?;
                Ok(to_value(session.set_config_option(config_id, value).await?))
            }
            Method::SessionCancel { session_id } => {
                self.session(&session_id)?.cancel();
                Ok(Value::Null)
            }
            Method::PermissionRespond {
                session_id,
                request_id,
                option_id,
            } => {
                self.session(&session_id)?.permit(&request_id, &option_id)?;
                Ok(Value::Null)
            }
            Method::ElicitationRespond {
                session_id,
                request_id,
                action,
            } => {
                self.session(&session_id)?
                    .respond_elicitation(&request_id, action)?;
                Ok(Value::Null)
            }
            Method::SessionDelete {
                session_id,
                discard,
                env,
            } => Ok(to_value(
                self.delete_session(&session_id, discard, env).await?,
            )),
            Method::SessionArchive {
                session_id,
                discard,
                env,
            } => Ok(to_value(
                self.archive_session(&session_id, discard, env).await?,
            )),
            Method::SessionUnarchive { session_id, env } => {
                Ok(to_value(self.unarchive_session(&session_id, env).await?))
            }
            Method::BranchMove {
                workspace_id,
                branch,
                to,
                expected,
                source_branch,
                force,
                env,
            } => {
                self.move_branch(
                    &workspace_id,
                    branches::Move {
                        branch: &branch,
                        to: &to,
                        expected: &expected,
                        source_branch: source_branch.as_deref(),
                        force,
                    },
                    env,
                )
                .await?;
                Ok(Value::Null)
            }
            Method::SessionSetCompare {
                session_id,
                compare,
            } => Ok(to_value(self.set_compare(&session_id, compare).await?)),
            Method::SessionRefresh { session_id } => {
                let session = self.session(&session_id)?;
                session.compare().await;
                Ok(to_value(session.info()))
            }
            Method::SessionSetTitle { session_id, title } => {
                self.session(&session_id)?.set_title(&title)?;
                Ok(Value::Null)
            }
            Method::Hello { .. }
            | Method::SessionSubscribe { .. }
            | Method::Watch
            | Method::DirectoryWatch { .. }
            | Method::Unsubscribe { .. } => Err(ErrorBody {
                code: code::INVALID_REQUEST,
                message: "unexpected method".into(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap()
    }

    /// Changes the registry and signals the change.
    fn update<R>(&self, change: impl FnOnce(&mut Registry) -> R) -> R {
        let mut registry = self.lock();
        let result = change(&mut registry);
        registry.version += 1;
        self.changed.send_replace(());
        result
    }

    /// The workspaces in the order they were added and the sessions in the
    /// order they were created. Whether each workspace is in a Git working
    /// tree is worked out now.
    pub fn list(&self) -> Listing {
        let registry = self.lock();
        let workspaces = registry.workspaces.clone();
        let mut sessions: Vec<Arc<Session>> = registry.sessions.values().cloned().collect();
        drop(registry);
        sessions.sort_by_key(|session| session.number);
        Listing {
            workspaces: workspaces.into_iter().map(workspace_info).collect(),
            sessions: sessions.iter().map(|session| session.info()).collect(),
        }
    }

    /// The changes to the workspaces and sessions since `sent`, which then
    /// records them: removals first, then additions and changes, in the order
    /// the listing has them. Only sessions whose info changed are read, and
    /// the workspaces only when the registry changed, such as when a client
    /// connects.
    pub fn listing_changes(&self, sent: &mut Sent) -> Vec<ListingChange> {
        let registry = self.lock();
        let workspaces =
            (sent.registry_version != Some(registry.version)).then(|| registry.workspaces.clone());
        sent.registry_version = Some(registry.version);
        let mut outdated: Vec<Arc<Session>> = registry
            .sessions
            .values()
            .filter(|session| sent.sessions.get(&session.id) != Some(&session.info_version()))
            .cloned()
            .collect();
        let removed: Vec<String> = sent
            .sessions
            .keys()
            .filter(|id| !registry.sessions.contains_key(*id))
            .cloned()
            .collect();
        drop(registry);

        let mut changes = Vec::new();
        for id in removed {
            sent.sessions.remove(&id);
            changes.push(ListingChange::SessionRemoved { session_id: id });
        }
        if let Some(workspaces) = workspaces {
            let workspaces: Vec<WorkspaceInfo> =
                workspaces.into_iter().map(workspace_info).collect();
            for workspace in &sent.workspaces {
                if !workspaces.iter().any(|current| current.id == workspace.id) {
                    changes.push(ListingChange::WorkspaceRemoved {
                        workspace_id: workspace.id.clone(),
                    });
                }
            }
            for workspace in &workspaces {
                if !sent.workspaces.contains(workspace) {
                    changes.push(ListingChange::Workspace {
                        workspace: workspace.clone(),
                    });
                }
            }
            sent.workspaces = workspaces;
        }
        outdated.sort_by_key(|session| session.number);
        for session in outdated {
            let (info, version) = session.versioned_info();
            sent.sessions.insert(session.id.clone(), version);
            changes.push(ListingChange::Session {
                session: Box::new(info),
            });
        }
        changes
    }

    fn add_workspace(&self, path: PathBuf) -> Result<WorkspaceInfo, ErrorBody> {
        if !path.is_absolute() {
            return Err(invalid_request(format!(
                "{} is not an absolute path",
                path.display()
            )));
        }
        if !path.is_dir() {
            return Err(invalid_request(format!(
                "{} is not a directory",
                path.display()
            )));
        }
        let workspace = SavedWorkspace {
            id: uuid::Uuid::new_v4().to_string(),
            path,
        };
        self.update(|registry| {
            if !self
                .store
                .add_workspace(&workspace)
                .map_err(|error| storage(&error))?
            {
                return Err(invalid_request(format!(
                    "{} is already a workspace",
                    workspace.path.display()
                )));
            }
            registry.workspaces.push(workspace.clone());
            Ok(workspace_info(workspace))
        })
    }

    fn remove_workspace(&self, id: &str) -> Result<(), ErrorBody> {
        self.update(|registry| {
            let index = registry
                .workspaces
                .iter()
                .position(|workspace| workspace.id == id)
                .ok_or_else(|| not_found("workspace", id))?;
            // The store has the rows of sessions still being created.
            if self
                .store
                .workspace_has_sessions(id)
                .map_err(|error| storage(&error))?
            {
                return Err(ErrorBody {
                    code: code::INVALID_STATE,
                    message: "the workspace has sessions".into(),
                });
            }
            self.store
                .remove_workspace(id)
                .map_err(|error| storage(&error))?;
            registry.workspaces.remove(index);
            Ok(())
        })
    }

    /// Creates a session in the workspace's directory or in a new worktree.
    /// For a worktree, the branch is resolved and created here, and the
    /// session's startup adds the worktree.
    async fn new_session(
        &self,
        workspace_id: &str,
        command: Vec<String>,
        env: BTreeMap<String, String>,
        checkout: Checkout,
    ) -> Result<SessionInfo, ErrorBody> {
        if command.is_empty() {
            return Err(invalid_request("command is empty".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let Checkout::Worktree { branch, compare } = checkout else {
            return self.add_session(id, workspace_id, command, env, &mut None);
        };
        let workspace = self.workspace_path(workspace_id)?;
        let git = Git::for_client(env.clone());
        let plan =
            worktree::plan(&git, &workspace, branch, compare, &self.worktrees_dir, &id).await?;
        worktree::create_branch(&git, &plan).await?;
        let mut worktree = Some((git, plan));
        let added = self.add_session(id, workspace_id, command, env, &mut worktree);
        if let Some((git, plan)) = worktree
            && let Err(error) = worktree::roll_back(&git, &plan, false).await
        {
            eprintln!("oxd: undoing a session that was not saved: {error}");
        }
        added
    }

    /// Saves a session, with its worktree if it has one, and spawns it,
    /// taking the worktree to add. A session that cannot be saved leaves the
    /// worktree where it is.
    fn add_session(
        &self,
        id: String,
        workspace_id: &str,
        command: Vec<String>,
        env: BTreeMap<String, String>,
        worktree: &mut Option<(Git, WorktreePlan)>,
    ) -> Result<SessionInfo, ErrorBody> {
        self.update(|registry| {
            let workspace = registry
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .ok_or_else(|| not_found("workspace", workspace_id))?;
            let (cwd, info) = match worktree {
                Some((_, plan)) => (plan.cwd.clone(), Some(plan.saved())),
                None => (workspace.path.clone(), None),
            };
            registry.refuse_moving(&cwd)?;
            let number = self
                .store
                .add_session(&id, workspace_id, &command, &cwd, info.as_ref())
                .map_err(|error| storage(&error))?;
            let saved = SavedSession {
                number,
                id,
                workspace_id: workspace_id.into(),
                command,
                cwd,
                title: None,
                agent_session_id: None,
                worktree: info,
                last_seq: 0,
                archived: false,
            };
            let session = Session::spawn(
                self.store.clone(),
                self.changed.clone(),
                self.park_after,
                self.ox_server.clone(),
                saved,
                env,
                worktree.take(),
            );
            let info = session.info();
            registry.sessions.insert(session.id.clone(), session);
            Ok(info)
        })
    }

    fn workspace_path(&self, id: &str) -> Result<PathBuf, ErrorBody> {
        self.lock()
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.path.clone())
            .ok_or_else(|| not_found("workspace", id))
    }

    /// The branches of the workspace's repository, listed after a fetch
    /// when `fetch` is set.
    async fn branches(
        &self,
        workspace_id: &str,
        fetch: bool,
        env: BTreeMap<String, String>,
    ) -> Result<Branches, ErrorBody> {
        let workspace = self.workspace_path(workspace_id)?;
        let git = Git::for_client(env);
        let common_dir = branches::common_dir(&git, &workspace).await?;
        let fetch_error = if fetch {
            self.fetch(&git, common_dir, &workspace).await.err()
        } else {
            None
        };
        Ok(Branches {
            fetch_error,
            ..branches::list(&git, &workspace).await?
        })
    }

    /// Fetches the repository whose common Git directory is `common_dir`,
    /// from `workspace` with `git`, or waits for the fetch of it already
    /// running, then asks the sessions in the repository to compare again.
    /// The fetch runs in its own task, so it finishes even when every request
    /// waiting for it is dropped.
    async fn fetch(&self, git: &Git, common_dir: PathBuf, workspace: &Path) -> Result<(), String> {
        let outcome = self.fetched(git, common_dir.clone(), workspace).await;
        self.recompare(&common_dir);
        outcome
    }

    async fn fetched(
        &self,
        git: &Git,
        common_dir: PathBuf,
        workspace: &Path,
    ) -> Result<(), String> {
        let mut outcome = match self.fetches.lock().unwrap().entry(common_dir) {
            Entry::Occupied(running) => running.get().clone(),
            Entry::Vacant(entry) => {
                let (sender, outcome) = watch::channel(None);
                let common_dir = entry.key().clone();
                entry.insert(outcome.clone());
                let fetches = self.fetches.clone();
                let git = git.clone();
                let workspace = workspace.to_owned();
                tokio::spawn(async move {
                    sender.send_replace(Some(branches::fetch(&git, &workspace).await));
                    fetches.lock().unwrap().remove(&common_dir);
                });
                outcome
            }
        };
        match outcome.wait_for(Option::is_some).await {
            Ok(outcome) => outcome.clone().expect("waited for an outcome"),
            Err(_) => Err("the fetch stopped without an outcome".into()),
        }
    }

    /// Waits until the session is not starting, loading or being deleted,
    /// and returns the guard that keeps it so. A session deleted meanwhile
    /// is not found.
    async fn lifecycle<'a>(
        &self,
        session: &'a Session,
    ) -> Result<tokio::sync::MutexGuard<'a, ()>, ErrorBody> {
        let guard = session.lifecycle().await;
        if !self.lock().sessions.contains_key(&session.id) {
            return Err(not_found("session", &session.id));
        }
        Ok(guard)
    }

    /// Deletes a session once its startup has finished. A session in a
    /// worktree has its agent stopped, then its worktree removed and the
    /// branch rule applied, with git run with `env`. A worktree that would
    /// lose work refuses the delete unless `discard` is set, leaving the
    /// session as it was, or `exited` when the work appeared after its agent
    /// stopped.
    async fn delete_session(
        &self,
        id: &str,
        discard: bool,
        env: BTreeMap<String, String>,
    ) -> Result<Deleted, ErrorBody> {
        let session = self.session(id)?;
        let _lifecycle = self.lifecycle(&session).await?;
        let mut kept_branch = None;
        if let Some(worktree) = session.worktree() {
            let git = Git::for_client(env);
            if !discard {
                let unsaved = worktree::unsaved(&git, &worktree).await?;
                if !unsaved.is_empty() {
                    return Err(worktree::unsaved_work(&unsaved));
                }
            }
            session.stop(Change::Exited(None)).await;
            worktree::remove_worktree(&git, &worktree, discard).await?;
            kept_branch = worktree::apply_branch_rule(&git, &worktree).await?;
            self.recompare(&worktree.common_dir);
        }
        self.forget(id)?;
        session.close();
        Ok(Deleted { kept_branch })
    }

    /// Archives a session that is not starting: its agent is
    /// stopped and its worktree removed, with git run with `env`, and the
    /// branch is kept. A worktree that would lose work refuses the archive
    /// unless `discard` is set, leaving the session as it was, or `exited`
    /// when the work appeared after its agent stopped.
    async fn archive_session(
        &self,
        id: &str,
        discard: bool,
        env: BTreeMap<String, String>,
    ) -> Result<SessionInfo, ErrorBody> {
        let session = self.session(id)?;
        refuse_starting(&session)?;
        let _lifecycle = self.lifecycle(&session).await?;
        refuse_starting(&session)?;
        if session.archived() {
            return Err(invalid_state("the session is already archived"));
        }
        let git = Git::for_client(env);
        let worktree = session.worktree();
        if let Some(worktree) = &worktree
            && !discard
        {
            let unsaved = worktree::unsaved(&git, worktree).await?;
            if !unsaved.is_empty() {
                return Err(worktree::unsaved_work(&unsaved));
            }
        }
        session.stop(Change::Parked).await;
        if let Some(worktree) = &worktree {
            worktree::remove_worktree(&git, worktree, discard).await?;
        }
        self.store
            .set_archived(id, true)
            .map_err(|error| storage(&error))?;
        session.set_archived(true);
        Ok(session.info())
    }

    /// Adds an archived session's worktree again, with git run with `env`,
    /// and unarchives it.
    async fn unarchive_session(
        &self,
        id: &str,
        env: BTreeMap<String, String>,
    ) -> Result<SessionInfo, ErrorBody> {
        let session = self.session(id)?;
        let _lifecycle = self.lifecycle(&session).await?;
        if !session.archived() {
            return Err(invalid_state("the session is not archived"));
        }
        if let Some(worktree) = session.worktree() {
            worktree::restore(&Git::for_client(env), &worktree).await?;
        }
        self.store
            .set_archived(id, false)
            .map_err(|error| storage(&error))?;
        session.set_archived(false);
        Ok(session.info())
    }

    /// Moves local branch `branch` of the workspace's repository from
    /// `expected` to `to`, with git run with `env`, unless a session's agent
    /// in the checkout that has the branch is busy, and asks the sessions in
    /// the repository to compare again.
    async fn move_branch(
        &self,
        workspace_id: &str,
        movement: branches::Move<'_>,
        env: BTreeMap<String, String>,
    ) -> Result<(), ErrorBody> {
        let workspace = self.workspace_path(workspace_id)?;
        let git = Git::for_client(env);
        let common_dir = branches::common_dir(&git, &workspace).await?;
        let refresh_comparison = movement.source_branch.is_some();
        let result = branches::move_branch(&git, &common_dir, movement, |checkout| {
            self.reserve_checkout(checkout)
        })
        .await;
        if result.is_ok() || refresh_comparison {
            self.recompare(&common_dir);
        }
        result
    }

    /// Keeps Ox from starting or prompting agents in the checkout during
    /// a branch move. The registry lock makes the busy check and reservation
    /// one operation with respect to those requests.
    fn reserve_checkout(&self, checkout: &Path) -> Result<CheckoutMove<'_>, ErrorBody> {
        let checkout = resolved(checkout);
        let mut registry = self.lock();
        registry.refuse_moving(&checkout)?;
        for session in registry.sessions.values() {
            let state = session.state();
            if matches!(state, SessionState::Idle | SessionState::Exited) {
                continue;
            }
            if resolved(session.cwd()).starts_with(&checkout) {
                return Err(ErrorBody {
                    code: code::INVALID_STATE,
                    message: format!(
                        "session {} is {} in {}",
                        session.id,
                        state.as_str(),
                        checkout.display()
                    ),
                });
            }
        }
        registry.moving_checkouts.insert(checkout.clone());
        Ok(CheckoutMove {
            daemon: self,
            checkout,
        })
    }

    /// Asks every session whose worktree is in the repository with common
    /// Git directory `common_dir` to compare its branch again.
    fn recompare(&self, common_dir: &Path) {
        for session in self.lock().sessions.values() {
            if session
                .worktree()
                .is_some_and(|worktree| worktree.common_dir == common_dir)
            {
                session.recompare();
            }
        }
    }

    /// Sets or clears the session's comparison ref, which must name a
    /// commit, and returns the session with its comparison recomputed.
    async fn set_compare(
        &self,
        id: &str,
        compare: Option<String>,
    ) -> Result<SessionInfo, ErrorBody> {
        let session = self.session(id)?;
        let _lifecycle = self.lifecycle(&session).await?;
        let worktree = session
            .worktree()
            .ok_or_else(|| invalid_request("the session has no worktree".into()))?;
        if let Some(compare) = &compare
            && worktree::Repository::new(&Git::own(), &worktree.common_dir)
                .commit(compare)
                .await
                .is_none()
        {
            return Err(invalid_request(format!("{compare} is not a commit")));
        }
        self.store
            .set_compare(id, compare.as_deref())
            .map_err(|error| storage(&error))?;
        session.set_compare(compare).await;
        Ok(session.info())
    }

    /// Deletes the session's rows and removes it from the registry.
    fn forget(&self, id: &str) -> Result<(), ErrorBody> {
        self.update(|registry| {
            self.store
                .remove_session(id)
                .map_err(|error| storage(&error))?;
            registry.sessions.remove(id);
            Ok(())
        })
    }
}

/// Removes the worktree and branch of a session that never started, with
/// the daemon's own environment, as a delete without `discard` would.
async fn remove_unstarted(worktree: &SavedWorktree) -> Result<(), ErrorBody> {
    let git = Git::own();
    worktree::remove_worktree(&git, worktree, false).await?;
    worktree::apply_branch_rule(&git, worktree).await?;
    Ok(())
}

/// `path` with symbolic links resolved, or as it is when it cannot be.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn refuse_starting(session: &Session) -> Result<(), ErrorBody> {
    if session.state() == SessionState::Starting {
        return Err(invalid_state("the session is still starting"));
    }
    Ok(())
}

/// The workspace as clients see it.
fn workspace_info(workspace: SavedWorkspace) -> WorkspaceInfo {
    WorkspaceInfo {
        git_working_tree: branches::git_working_tree(&workspace.path),
        id: workspace.id,
        path: workspace.path,
    }
}

pub(crate) fn invalid_request(message: String) -> ErrorBody {
    ErrorBody {
        code: code::INVALID_REQUEST,
        message,
    }
}

fn not_found(kind: &str, id: &str) -> ErrorBody {
    ErrorBody {
        code: code::NOT_FOUND,
        message: format!("no {kind} {id}"),
    }
}

fn invalid_state(message: &str) -> ErrorBody {
    ErrorBody {
        code: code::INVALID_STATE,
        message: message.into(),
    }
}

fn storage(error: &rusqlite::Error) -> ErrorBody {
    ErrorBody {
        code: code::STORAGE,
        message: format!("the database failed: {error}"),
    }
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("protocol types serialize")
}
