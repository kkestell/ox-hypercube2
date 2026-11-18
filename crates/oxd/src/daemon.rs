use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ox_protocol::config::{Choice, HarnessConfig};
use ox_protocol::schema::{
    ContentBlock, PermissionOption, PermissionOptionKind, PromptCapabilities,
    RequestPermissionOutcome, TextContent,
};
use ox_protocol::{
    Branches, Checkout, ConfigSetting, ConfigValue, Deleted, ErrorBody, EventKind, Harness,
    ImportedSessions, ListingChange, MessageSent, Method, QueueAdded, RequestedCheckout, Sender,
    SessionInfo, SessionRequest, SessionState, WorkspaceInfo, code,
};
use serde_json::Value;
use tokio::sync::{Notify, OwnedMutexGuard, watch};

use crate::branches;
use crate::directories::Directories;
use crate::import::{self, ClaudeSession, Transcript};
use crate::mcp::OxServer;
use crate::session::{Opening, Session};
use crate::state::{self, Change, Queue, State};
use crate::store::{SavedSession, SavedWorkspace, Store};
use crate::worktree::{self, Git, Worktree, WorktreePlan};

struct Registry {
    workspaces: Vec<SavedWorkspace>,
    worktrees: HashMap<String, Arc<Worktree>>,
    sessions: HashMap<String, Arc<Session>>,
    clients: usize,
    moving_checkouts: HashSet<PathBuf>,
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
    /// Sent when a workspace, worktree or session is added or removed, a
    /// client connects or disconnects, or a session's `SessionInfo` or a
    /// worktree's `WorktreeInfo` changes.
    changed: watch::Sender<()>,
    /// The directories that directory watches follow.
    directories: Arc<Directories>,
    /// The `ox` MCP server each session's agent is given.
    ox_server: Arc<OxServer>,
    /// Notified when a client asks the daemon to stop.
    stop: Notify,
}

/// What a watch has sent: the workspaces, as of a version of the registry,
/// and the version of each worktree's and session's info.
#[derive(Default)]
pub struct Sent {
    registry_version: Option<u64>,
    workspaces: Vec<WorkspaceInfo>,
    worktrees: HashMap<String, u64>,
    sessions: HashMap<String, u64>,
}

/// Counts one connected client for as long as it lives.
pub struct ClientGuard(Arc<Daemon>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.update(|registry| registry.clients -= 1);
    }
}

/// A branch move under way in a checkout, whose sessions refuse turns and
/// loads until it ends.
struct CheckoutMove<'a> {
    daemon: &'a Daemon,
    checkout: PathBuf,
}

impl Drop for CheckoutMove<'_> {
    fn drop(&mut self) {
        let mut registry = self.daemon.lock();
        registry.moving_checkouts.remove(&self.checkout);
        for session in registry.sessions.values() {
            if resolved(session.cwd()).starts_with(&self.checkout) {
                session.end_move();
            }
        }
    }
}

impl Registry {
    /// Whether a branch is moving in the checkout that holds `cwd`.
    fn moving(&self, cwd: &Path) -> bool {
        if self.moving_checkouts.is_empty() {
            return false;
        }
        let cwd = resolved(cwd);
        self.moving_checkouts
            .iter()
            .any(|checkout| cwd.starts_with(checkout))
    }

    fn refuse_moving(&self, cwd: &Path) -> Result<(), ErrorBody> {
        if self.moving(cwd) {
            return Err(invalid_state(
                "a branch move is in progress in this checkout",
            ));
        }
        Ok(())
    }
}

impl Daemon {
    /// A daemon with the workspaces, worktrees and sessions `store` holds,
    /// which adds worktrees in `worktrees_dir` and parks the agent of a
    /// session that has been idle, with no background work, and unviewed for
    /// `park_after`, whose directory watches follow `directories`, and whose
    /// sessions' agents are given `ox_server`. Each saved session is
    /// `exited`, with no agent, until it loads, and what a stop left open in
    /// its events is closed. A saved session whose agent never started keeps
    /// its history and has nothing to load.
    pub fn new(
        store: Store,
        worktrees_dir: PathBuf,
        park_after: Duration,
        directories: Arc<Directories>,
        ox_server: Arc<OxServer>,
    ) -> rusqlite::Result<Self> {
        let store = Arc::new(store);
        let changed = watch::Sender::new(());
        let worktrees: HashMap<String, Arc<Worktree>> = store
            .worktrees()?
            .into_iter()
            .map(|saved| (saved.id.clone(), Worktree::new(saved, changed.clone())))
            .collect();
        let mut left_open = store.left_open()?;
        let mut sessions = HashMap::new();
        for saved in store.sessions()? {
            let worktree = saved.worktree_id.as_ref().map(|id| {
                worktrees
                    .get(id)
                    .expect("a session's worktree is saved")
                    .clone()
            });
            let state = State::restored(&saved, worktree);
            let session = Session::new(
                store.clone(),
                changed.clone(),
                park_after,
                ox_server.clone(),
                saved,
                state,
            );
            if let Some(open) = left_open.remove(&session.id) {
                session.recover(open);
            }
            sessions.insert(session.id.clone(), session);
        }
        let registry = Registry {
            workspaces: store.workspaces()?,
            worktrees,
            sessions,
            clients: 0,
            moving_checkouts: HashSet::new(),
            version: 0,
        };
        Ok(Self {
            registry: Mutex::new(registry),
            store,
            worktrees_dir,
            park_after,
            changed,
            directories,
            ox_server,
            stop: Notify::new(),
        })
    }

    pub fn client(self: &Arc<Self>) -> ClientGuard {
        self.update(|registry| registry.clients += 1);
        ClientGuard(self.clone())
    }

    /// Resolves once the daemon has had no clients and no running agents for
    /// `idle_exit`. A session whose agent is not running,
    /// `exited`, is saved and does not keep the daemon alive.
    pub async fn idle(&self, idle_exit: Duration) {
        idle_for(self.changed.subscribe(), idle_exit, || self.is_idle()).await;
    }

    /// Asks the daemon to stop, as `daemon.stop` does. Refused, listing
    /// each busy session as `<title or id> (<state>)`, while a session is in
    /// a state other than `idle` or `exited`.
    pub fn stop(&self) -> Result<(), ErrorBody> {
        let sessions: Vec<_> = self.lock().sessions.values().cloned().collect();
        let mut busy: Vec<String> = sessions
            .iter()
            .filter_map(|session| {
                let state = session.state();
                (!matches!(state, SessionState::Idle | SessionState::Exited)).then(|| {
                    let name = session.title().unwrap_or_else(|| session.id.clone());
                    format!("{name} ({})", state.as_str())
                })
            })
            .collect();
        if !busy.is_empty() {
            busy.sort();
            return Err(invalid_state(&format!(
                "oxd has busy sessions: {}",
                busy.join(", ")
            )));
        }
        self.stop.notify_one();
        Ok(())
    }

    /// Resolves once a client has asked the daemon to stop.
    pub async fn stop_requested(&self) {
        self.stop.notified().await;
    }

    fn is_idle(&self) -> bool {
        let registry = self.lock();
        nothing_running(
            registry.clients,
            registry.sessions.values().map(|session| session.state()),
        )
    }

    /// Stops every agent and every comparison. The sessions stay saved.
    pub fn close_all(&self) {
        let mut sessions = Vec::new();
        let mut worktrees = Vec::new();
        self.update(|registry| {
            sessions.extend(registry.sessions.drain().map(|(_, s)| s));
            worktrees.extend(registry.worktrees.drain().map(|(_, w)| w));
        });
        for session in sessions {
            session.close();
        }
        for worktree in worktrees {
            worktree.close();
        }
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

    fn worktree(&self, id: &str) -> Result<Arc<Worktree>, ErrorBody> {
        self.lock()
            .worktrees
            .get(id)
            .cloned()
            .ok_or_else(|| not_found("worktree", id))
    }

    /// Sets the settings in `config` on session `session_id` and leaves the
    /// others as they are, sending its agent each setting ahead of every
    /// later command to the session. The returned future resolves once the
    /// agent has answered.
    pub fn set_config(
        &self,
        session_id: &str,
        config: Vec<ConfigSetting>,
    ) -> Result<impl Future<Output = Result<Value, ErrorBody>> + use<>, ErrorBody> {
        let answered = self.session(session_id)?.set_config(config)?;
        Ok(async move {
            answered.await?;
            Ok(Value::Null)
        })
    }

    /// Handles every method except `hello`, `session.subscribe`, `watch`, and
    /// `unsubscribe`, which belong to the connection.
    pub async fn handle(self: &Arc<Self>, method: Method) -> Result<Value, ErrorBody> {
        match method {
            Method::WorkspaceAdd { path } => Ok(to_value(self.add_workspace(path)?)),
            Method::WorkspaceRemove { workspace_id } => {
                self.remove_workspace(&workspace_id).await?;
                Ok(Value::Null)
            }
            Method::WorkspaceBranches { workspace_id, env } => {
                Ok(to_value(self.branches(&workspace_id, env).await?))
            }
            Method::WorkspaceImportClaudeSessions { workspace_id, env } => Ok(to_value(
                self.import_claude_sessions(&workspace_id, env).await?,
            )),
            Method::SessionNew {
                workspace_id,
                command,
                env,
                checkout,
                prompt,
                config,
            } => Ok(to_value(
                self.new_session(&workspace_id, command, env, checkout, prompt, config)
                    .await?,
            )),
            Method::SessionPrompt { session_id, prompt } => {
                self.session(&session_id)?.prompt(prompt)?;
                Ok(Value::Null)
            }
            Method::SessionQueueAdd { session_id, prompt } => {
                let prompt_id = self.session(&session_id)?.queue_add(prompt)?;
                Ok(to_value(QueueAdded { prompt_id }))
            }
            Method::SessionQueueEdit {
                session_id,
                prompt_id,
                prompt,
            } => {
                self.session(&session_id)?.queue_edit(prompt_id, prompt)?;
                Ok(Value::Null)
            }
            Method::SessionQueueRemove {
                session_id,
                prompt_id,
            } => {
                self.session(&session_id)?.queue_remove(prompt_id)?;
                Ok(Value::Null)
            }
            Method::SessionQueueSend {
                session_id,
                prompt_id,
            } => {
                self.session(&session_id)?.queue_send(prompt_id)?;
                Ok(Value::Null)
            }
            Method::SessionSetConfig { session_id, config } => {
                self.set_config(&session_id, config)?.await
            }
            Method::SessionCancel { session_id } => {
                self.session(&session_id)?.cancel();
                Ok(Value::Null)
            }
            Method::SessionSendMessage {
                session_id,
                to,
                text,
            } => Ok(to_value(self.send_message(&session_id, &to, text).await?)),
            Method::SessionTranscript { session_id, before } => Ok(to_value(
                self.session(&session_id)?.transcript(before).await?,
            )),
            Method::SessionRequest {
                session_id,
                prompt,
                workspace,
                worktree,
                branch,
                harness,
                model,
                effort,
                mode,
            } => Ok(to_value(
                self.request_session(
                    &session_id,
                    RequestParams {
                        prompt,
                        workspace,
                        worktree,
                        branch,
                        harness,
                        model,
                        effort,
                        mode,
                    },
                )
                .await?,
            )),
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
                env,
            } => {
                self.move_branch(
                    &workspace_id,
                    branches::Move {
                        branch: &branch,
                        to: &to,
                        expected: &expected,
                        source_branch: &source_branch,
                    },
                    env,
                )
                .await?;
                Ok(Value::Null)
            }
            Method::WorktreeRefresh { worktree_id } => {
                let worktree = self.worktree(&worktree_id)?;
                worktree.compare().await;
                Ok(to_value(worktree.info()))
            }
            Method::SessionSetTitle { session_id, title } => {
                self.session(&session_id)?.set_title(&title)?;
                Ok(Value::Null)
            }
            Method::Hello { .. }
            | Method::DaemonStop
            | Method::SessionSubscribe { .. }
            | Method::Watch
            | Method::DirectoryWatch { .. }
            | Method::FileWatch { .. }
            | Method::Unsubscribe { .. }
            | Method::Focus { .. } => Err(ErrorBody {
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

    /// The changes to the workspaces, worktrees and sessions since `sent`,
    /// which then records them: removals of sessions and worktrees, then
    /// additions and changes of workspaces, worktrees and sessions, in that
    /// order, so that a session's worktree comes before the session. Only
    /// worktrees and sessions whose info changed are read,
    /// and the workspaces only when the registry changed, such as when a
    /// client connects.
    pub fn listing_changes(&self, sent: &mut Sent) -> Vec<ListingChange> {
        let registry = self.lock();
        let workspaces =
            (sent.registry_version != Some(registry.version)).then(|| registry.workspaces.clone());
        sent.registry_version = Some(registry.version);
        let (outdated_worktrees, removed_worktrees) =
            stale(&registry.worktrees, &sent.worktrees, Worktree::version);
        let (mut outdated, removed) =
            stale(&registry.sessions, &sent.sessions, Session::info_version);
        drop(registry);

        let mut changes = Vec::new();
        for id in removed {
            sent.sessions.remove(&id);
            changes.push(ListingChange::SessionRemoved { session_id: id });
        }
        for id in removed_worktrees {
            sent.worktrees.remove(&id);
            changes.push(ListingChange::WorktreeRemoved { worktree_id: id });
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
        for worktree in outdated_worktrees {
            let (info, version) = worktree.versioned_info();
            sent.worktrees.insert(worktree.id.clone(), version);
            changes.push(ListingChange::Worktree { worktree: info });
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

    /// Removes a workspace whose sessions are all archived, deleting them
    /// with their worktrees' rows. Their branches are kept. An unarchive or
    /// delete of one of its sessions already under way finishes first.
    async fn remove_workspace(&self, id: &str) -> Result<(), ErrorBody> {
        let not_archived = || ErrorBody {
            code: code::INVALID_STATE,
            message: "the workspace has sessions that are not archived".into(),
        };
        loop {
            let mut sessions = self.workspace_sessions(id)?;
            if !sessions.iter().all(|session| session.archived()) {
                return Err(not_archived());
            }
            sessions.sort_by_key(|session| session.number);
            let mut guards = Vec::with_capacity(sessions.len());
            for session in &sessions {
                match session.lifecycle().await {
                    Ok(guard) => guards.push(guard),
                    // Deleted meanwhile.
                    Err(_) => break,
                }
            }
            if guards.len() < sessions.len() {
                continue;
            }
            let removed = self.update(|registry| {
                let index = registry
                    .workspaces
                    .iter()
                    .position(|workspace| workspace.id == id)
                    .ok_or_else(|| not_found("workspace", id))?;
                let current = registry
                    .sessions
                    .values()
                    .filter(|session| session.workspace_id() == id)
                    .count();
                // A session added meanwhile, such as by an import.
                if current != sessions.len()
                    || !sessions
                        .iter()
                        .all(|session| registry.sessions.contains_key(&session.id))
                {
                    return Ok(false);
                }
                if !sessions.iter().all(|session| session.archived()) {
                    return Err(not_archived());
                }
                self.store
                    .remove_workspace(id)
                    .map_err(|error| storage(&error))?;
                registry.workspaces.remove(index);
                for session in &sessions {
                    registry.sessions.remove(&session.id);
                    if let Some(worktree) = session.worktree() {
                        registry.worktrees.remove(&worktree.id);
                    }
                }
                Ok(true)
            })?;
            if !removed {
                continue;
            }
            for session in &sessions {
                if let Some(worktree) = session.worktree() {
                    worktree.close();
                }
                session.close();
            }
            return Ok(());
        }
    }

    /// The sessions of workspace `id`.
    fn workspace_sessions(&self, id: &str) -> Result<Vec<Arc<Session>>, ErrorBody> {
        let registry = self.lock();
        if !registry
            .workspaces
            .iter()
            .any(|workspace| workspace.id == id)
        {
            return Err(not_found("workspace", id));
        }
        Ok(registry
            .sessions
            .values()
            .filter(|session| session.workspace_id() == id)
            .cloned()
            .collect())
    }

    /// Creates a session in the workspace's directory or in a new worktree,
    /// with its first prompt and its config.
    /// For a worktree, the branch is resolved and created here, and the
    /// worktree is added before the session's agent starts.
    async fn new_session(
        self: &Arc<Self>,
        workspace_id: &str,
        command: Vec<String>,
        env: BTreeMap<String, String>,
        checkout: Checkout,
        prompt: Option<Vec<ContentBlock>>,
        config: Vec<ConfigSetting>,
    ) -> Result<SessionInfo, ErrorBody> {
        if command.is_empty() {
            return Err(invalid_request("command is empty".into()));
        }
        // The agent's capabilities are unknown until it starts, so a first
        // prompt holds what every agent accepts.
        if let Some(prompt) = &prompt {
            state::check_prompt(prompt, &PromptCapabilities::default())?;
        }
        let opening = Opening { prompt: None };
        let id = uuid::Uuid::new_v4().to_string();
        let Checkout::Worktree { compare, branch } = checkout else {
            return self.add_session(
                id,
                workspace_id,
                command,
                config,
                env,
                &mut None,
                opening,
                prompt,
            );
        };
        let git = Git::for_client(env.clone());
        let workspace = self.workspace_path(workspace_id)?;
        let worktree_id = uuid::Uuid::new_v4().to_string();
        let plan = worktree::plan(
            &git,
            &workspace,
            compare,
            branch,
            &self.worktrees_dir,
            &worktree_id,
        )
        .await?;
        if plan.created_branch {
            worktree::create_branch(&git, &plan).await?;
        }
        // A branch created for a session that is not saved is deleted again.
        let mut pending = Some((git, plan));
        let added = self.add_session(
            id,
            workspace_id,
            command,
            config,
            env,
            &mut pending,
            opening,
            prompt,
        );
        if let Some((git, plan)) = pending
            && let Err(error) = worktree::roll_back(&git, &plan, false).await
        {
            eprintln!("oxd: undoing a session that was not saved: {error}");
        }
        added
    }

    /// Saves and registers a session, with its config and its worktree if it
    /// has one, and starts it with `opening`, taking the worktree to add. A first
    /// `prompt` is recorded at once and titles the session. A session that
    /// cannot be saved leaves the worktree where it is.
    #[expect(
        clippy::too_many_arguments,
        reason = "the session's saved parts and what its agent is given"
    )]
    fn add_session(
        self: &Arc<Self>,
        id: String,
        workspace_id: &str,
        command: Vec<String>,
        config: Vec<ConfigSetting>,
        env: BTreeMap<String, String>,
        worktree: &mut Option<(Git, WorktreePlan)>,
        mut opening: Opening,
        prompt: Option<Vec<ContentBlock>>,
    ) -> Result<SessionInfo, ErrorBody> {
        self.update(|registry| {
            let workspace = registry
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .ok_or_else(|| not_found("workspace", workspace_id))?;
            let (cwd, saved_worktree) = match worktree {
                Some((_, plan)) => (plan.cwd.clone(), Some(plan.saved())),
                None => (workspace.path.clone(), None),
            };
            registry.refuse_moving(&cwd)?;
            let created_at = now();
            let number = self
                .store
                .add_session(
                    &id,
                    workspace_id,
                    &command,
                    &cwd,
                    saved_worktree.as_ref(),
                    created_at,
                    &config,
                )
                .map_err(|error| storage(&error))?;
            let saved = SavedSession {
                number,
                id: id.clone(),
                workspace_id: workspace_id.into(),
                command,
                cwd,
                title: None,
                agent_session_id: None,
                worktree_id: saved_worktree.as_ref().map(|saved| saved.id.clone()),
                last_seq: 0,
                archived: false,
                created_at,
                unseen: None,
                queue: Queue::default(),
                config,
                needs_replay: true,
            };
            let state = State::created(
                &saved,
                saved_worktree.map(|saved| Worktree::new(saved, self.changed.clone())),
            );
            let session = Session::new(
                self.store.clone(),
                self.changed.clone(),
                self.park_after,
                self.ox_server.clone(),
                saved,
                state,
            );
            registry.sessions.insert(id, session.clone());
            if let Some(prompt) = prompt {
                if let Some(title) = acp_bridge::title(&prompt_text(&prompt)) {
                    session
                        .set_title(&title)
                        .expect("a title from a prompt is not empty");
                }
                let seq = session.record_first_prompt(prompt.clone());
                opening.prompt = Some((prompt, seq));
            }
            match (session.worktree(), worktree.take()) {
                (Some(live_worktree), Some((git, plan))) => {
                    registry
                        .worktrees
                        .insert(live_worktree.id.clone(), live_worktree.clone());
                    let lifecycle = session.hold_lifecycle();
                    tokio::spawn(self.clone().start_in_worktree(
                        session.clone(),
                        live_worktree,
                        lifecycle,
                        git,
                        plan,
                        env,
                        opening,
                    ));
                }
                _ => session.start(env, opening),
            }
            Ok(session.info())
        })
    }

    /// Adds the session's worktree, then launches its agent in it with
    /// `opening`. If the worktree cannot be added, undoes the branch and the
    /// worktree, forgets the worktree, and the session exits with git's
    /// error and no worktree.
    #[expect(
        clippy::too_many_arguments,
        reason = "the session, its worktree, and what its agent is given"
    )]
    async fn start_in_worktree(
        self: Arc<Self>,
        session: Arc<Session>,
        live_worktree: Arc<Worktree>,
        _lifecycle: OwnedMutexGuard<()>,
        git: Git,
        plan: WorktreePlan,
        env: BTreeMap<String, String>,
        opening: Opening,
    ) {
        let Err(message) = worktree::add(&git, &plan).await else {
            session.start(env, opening);
            return;
        };
        self.update(|registry| {
            registry.worktrees.remove(&live_worktree.id);
            session.fail_start(message);
        });
        live_worktree.close();
    }

    /// Sends a peer message from session `sender_id` to session `to`. An
    /// exited recipient is loaded first, with the sender's environment, and
    /// the message waits for its agent to start.
    async fn send_message(
        &self,
        sender_id: &str,
        to: &str,
        text: String,
    ) -> Result<MessageSent, ErrorBody> {
        let sender = self.session(sender_id)?;
        if to == sender_id {
            return Err(invalid_request("a session cannot message itself".into()));
        }
        if text.trim().is_empty() {
            return Err(invalid_request("the message is empty".into()));
        }
        let recipient = self.session(to)?;
        if recipient.archived() {
            return Err(invalid_state(&format!("session {to} is archived")));
        }
        recipient.load_if_exited(sender.env()).await?;
        recipient.started().await?;
        let from = Sender {
            session_id: sender.id.clone(),
            title: sender.title(),
        };
        let steered = recipient.message(text, from)?;
        Ok(MessageSent { steered })
    }

    /// Asks the user, through a permission request on session
    /// `requester_id`, to start the session `params` describes, once it is
    /// checked against the workspace and Ox's config. Starting it, the new
    /// session gets the requester's environment. Resolves to `None` when the
    /// user does not start it.
    async fn request_session(
        self: &Arc<Self>,
        requester_id: &str,
        params: RequestParams,
    ) -> Result<Option<SessionInfo>, ErrorBody> {
        let requester = self.session(requester_id)?;
        if params.prompt.trim().is_empty() {
            return Err(invalid_request("the prompt is empty".into()));
        }
        let (workspace_id, workspace) = match &params.workspace {
            Some(path) => {
                let wanted = resolved(path);
                self.lock()
                    .workspaces
                    .iter()
                    .find(|workspace| resolved(&workspace.path) == wanted)
                    .map(|workspace| (workspace.id.clone(), workspace.path.clone()))
                    .ok_or_else(|| {
                        invalid_request(format!("{} is not an Ox workspace", path.display()))
                    })?
            }
            None => {
                let id = requester.workspace_id().to_owned();
                let path = self.workspace_path(&id)?;
                (id, path)
            }
        };
        let harnesses = ox_protocol::config::load()
            .map_err(|error| invalid_state(&error))?
            .harnesses;
        let chosen = chosen(&harnesses, &params)?;
        let bridge = bridge(chosen.harness)?;
        let env = requester.env();
        let git = Git::for_client(env.clone());
        let (checkout, approved) = if params.worktree {
            let worktree_id = uuid::Uuid::new_v4().to_string();
            worktree::plan(
                &git,
                &workspace,
                None,
                params.branch.clone(),
                &self.worktrees_dir,
                &worktree_id,
            )
            .await?;
            let head = branches::list(&git, &workspace).await?.head;
            let shown = RequestedCheckout::Worktree {
                branch: params.branch.clone(),
                base: head.clone().unwrap_or_else(|| "HEAD".into()),
            };
            let checkout = Checkout::Worktree {
                compare: head,
                branch: params.branch.clone(),
            };
            (checkout, shown)
        } else {
            if let Some(branch) = &params.branch {
                let head = branches::list(&git, &workspace).await?.head;
                if head.as_ref() != Some(branch) {
                    return Err(invalid_request(format!(
                        "the workspace's checkout is on {}, not {branch}",
                        head.as_deref().unwrap_or("no branch")
                    )));
                }
            }
            let shown = RequestedCheckout::Workspace {
                path: workspace.clone(),
            };
            (Checkout::Workspace, shown)
        };

        let name = workspace.file_name().map_or_else(
            || workspace.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let request = SessionRequest {
            harness: chosen.harness,
            model: chosen.model,
            provider: chosen.provider,
            effort: chosen.effort,
            mode: chosen.mode,
            checkout: approved,
            prompt: params.prompt.clone(),
        };
        let options = vec![
            PermissionOption::new("start", "Start Session", PermissionOptionKind::AllowOnce),
            PermissionOption::new("decline", "Decline", PermissionOptionKind::RejectOnce),
        ];
        let outcome = requester
            .ask(format!("Start a session in {name}?"), request, options)
            .await;
        let RequestPermissionOutcome::Selected(selected) = outcome else {
            return Ok(None);
        };
        if *selected.option_id.0 != *"start" {
            return Ok(None);
        }
        let command = vec![bridge.to_string_lossy().into_owned()];
        let prompt = vec![ContentBlock::Text(TextContent::new(params.prompt))];
        self.new_session(
            &workspace_id,
            command,
            env,
            checkout,
            Some(prompt),
            chosen.config,
        )
        .await
        .map(Some)
    }

    fn workspace_path(&self, id: &str) -> Result<PathBuf, ErrorBody> {
        self.lock()
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .map(|workspace| workspace.path.clone())
            .ok_or_else(|| not_found("workspace", id))
    }

    /// The branches of the workspace's repository.
    async fn branches(
        &self,
        workspace_id: &str,
        env: BTreeMap<String, String>,
    ) -> Result<Branches, ErrorBody> {
        let workspace = self.workspace_path(workspace_id)?;
        let git = Git::for_client(env);
        branches::common_dir(&git, &workspace).await?;
        branches::list(&git, &workspace).await
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
        let _lifecycle = session.lifecycle().await?;
        let mut kept_branch = None;
        if let Some(worktree) = session.worktree() {
            let git = Git::for_client(env);
            worktree
                .remove(&git, discard, session.stop(Change::Exited(None)))
                .await?;
            kept_branch = worktree.apply_branch_rule(&git).await?;
            self.recompare(&worktree.saved().common_dir);
        }
        self.forget(id)?;
        session.close();
        Ok(Deleted { kept_branch })
    }

    /// Archives a session, as `Session::archive` describes, with git run
    /// with `env`.
    async fn archive_session(
        &self,
        id: &str,
        discard: bool,
        env: BTreeMap<String, String>,
    ) -> Result<SessionInfo, ErrorBody> {
        self.session(id)?
            .archive(&Git::for_client(env), discard)
            .await
    }

    /// Unarchives a session, as `Session::unarchive` describes, with git run
    /// with `env`.
    async fn unarchive_session(
        &self,
        id: &str,
        env: BTreeMap<String, String>,
    ) -> Result<SessionInfo, ErrorBody> {
        self.session(id)?.unarchive(&Git::for_client(env)).await
    }

    /// Moves local branch `branch` of the workspace's repository from
    /// `expected` to `to`, with git run with `env`, unless a session's agent
    /// in the checkout that has the branch is busy, and asks the worktrees of
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
        let result = branches::move_branch(&git, &common_dir, movement, |checkout| {
            self.reserve_checkout(checkout)
        })
        .await;
        self.recompare(&common_dir);
        result
    }

    /// Keeps Ox from starting or prompting agents in the checkout during
    /// a branch move: each session in it, which must be `idle` or `exited`,
    /// refuses turns and loads until the move ends, and no session is
    /// created in it. Under the registry lock, no session is added or begins
    /// another move meanwhile.
    fn reserve_checkout(&self, checkout: &Path) -> Result<CheckoutMove<'_>, ErrorBody> {
        let checkout = resolved(checkout);
        let mut registry = self.lock();
        registry.refuse_moving(&checkout)?;
        let mut sessions: Vec<Arc<Session>> = Vec::new();
        for session in registry.sessions.values() {
            if !resolved(session.cwd()).starts_with(&checkout) {
                continue;
            }
            if let Err(state) = session.begin_move() {
                for marked in &sessions {
                    marked.end_move();
                }
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
            sessions.push(session.clone());
        }
        registry.moving_checkouts.insert(checkout.clone());
        Ok(CheckoutMove {
            daemon: self,
            checkout,
        })
    }

    /// Asks every worktree of the repository with common Git directory
    /// `common_dir` to compare its branch again.
    fn recompare(&self, common_dir: &Path) {
        for worktree in self.lock().worktrees.values() {
            if worktree.saved().common_dir == common_dir {
                worktree.recompare();
            }
        }
    }

    /// Imports the interactive Claude sessions started in the workspace's
    /// directory that no session has and no deleted session had, newest
    /// first, each saved and registered as soon as its transcript is read.
    /// Returns how many it imported.
    async fn import_claude_sessions(
        self: &Arc<Self>,
        workspace_id: &str,
        env: BTreeMap<String, String>,
    ) -> Result<ImportedSessions, ErrorBody> {
        let workspace = self.workspace_path(workspace_id)?;
        let bridge = bridge(Harness::Claude)?;
        let listed = import::sessions(&bridge, &workspace, &env)
            .await
            .map_err(|error| ErrorBody {
                code: code::AGENT,
                message: format!("listing Claude Code sessions: {error}"),
            })?;
        let mut imported = 0;
        for claude in listed {
            if self.knows_claude_session(&self.lock(), &claude.session_id)? {
                continue;
            }
            let transcript =
                match import::transcript(&bridge, &workspace, &claude.session_id, &env).await {
                    Ok(transcript) => transcript,
                    Err(error) => {
                        eprintln!(
                            "oxd: importing Claude Code session {}: {error}",
                            claude.session_id
                        );
                        continue;
                    }
                };
            if self.add_imported(workspace_id, &bridge, &claude, transcript)? {
                imported += 1;
            }
        }
        Ok(ImportedSessions { imported })
    }

    /// Whether a session has Claude session `agent_session_id`, or a deleted
    /// session had it.
    fn knows_claude_session(
        &self,
        registry: &Registry,
        agent_session_id: &str,
    ) -> Result<bool, ErrorBody> {
        let registered = registry
            .sessions
            .values()
            .any(|session| session.agent_session_id().as_deref() == Some(agent_session_id));
        Ok(registered
            || self
                .store
                .was_deleted(agent_session_id)
                .map_err(|error| storage(&error))?)
    }

    /// Saves and registers an imported Claude session as archived and
    /// exited, as a session restored at startup is, unless a session has it
    /// by now. Returns whether it was added.
    fn add_imported(
        &self,
        workspace_id: &str,
        bridge: &Path,
        claude: &ClaudeSession,
        transcript: Transcript,
    ) -> Result<bool, ErrorBody> {
        self.update(|registry| {
            if self.knows_claude_session(registry, &claude.session_id)? {
                return Ok(false);
            }
            let workspace = registry
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .ok_or_else(|| not_found("workspace", workspace_id))?;
            let id = uuid::Uuid::new_v4().to_string();
            let events = import::events(&id, transcript.updates);
            let mut saved = SavedSession {
                number: 0,
                id: id.clone(),
                workspace_id: workspace_id.into(),
                command: vec![bridge.to_string_lossy().into_owned()],
                cwd: workspace.path.clone(),
                title: transcript.title,
                agent_session_id: Some(claude.session_id.clone()),
                worktree_id: None,
                last_seq: events.last().map_or(0, |event| event.seq),
                archived: true,
                created_at: claude.started_at,
                unseen: None,
                queue: Queue::default(),
                config: Vec::new(),
                needs_replay: !events.iter().any(|event| {
                    matches!(
                        event.kind,
                        EventKind::AgentStarted { .. } | EventKind::PromptStarted { .. }
                    )
                }),
            };
            saved.number = self
                .store
                .import_session(&saved, &events)
                .map_err(|error| storage(&error))?;
            let mut state = State::restored(&saved, None);
            if registry.moving(&saved.cwd) {
                state
                    .begin_move()
                    .expect("a restored session is exited, which a move allows");
            }
            let session = Session::new(
                self.store.clone(),
                self.changed.clone(),
                self.park_after,
                self.ox_server.clone(),
                saved,
                state,
            );
            registry.sessions.insert(id, session);
            Ok(true)
        })
    }

    /// Deletes the session's rows, with its worktree's, and removes both from
    /// the registry.
    fn forget(&self, id: &str) -> Result<(), ErrorBody> {
        let worktree = self.update(|registry| {
            self.store
                .remove_session(id)
                .map_err(|error| storage(&error))?;
            let session = registry.sessions.remove(id);
            let worktree = session.and_then(|session| session.worktree());
            if let Some(worktree) = &worktree {
                registry.worktrees.remove(&worktree.id);
            }
            Ok::<_, ErrorBody>(worktree)
        })?;
        if let Some(worktree) = worktree {
            worktree.close();
        }
        Ok(())
    }
}

/// A session that one asks the user to start, as `session.request` gives it.
struct RequestParams {
    prompt: String,
    workspace: Option<PathBuf>,
    worktree: bool,
    branch: Option<String>,
    harness: String,
    model: String,
    effort: Option<String>,
    mode: Option<String>,
}

/// A requested harness, model, effort and mode, checked against Ox's
/// config.
struct Chosen {
    harness: Harness,
    /// The names of the model, its provider, effort and mode, for the user
    /// to approve.
    model: String,
    provider: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    /// The settings that start the session on them.
    config: Vec<ConfigSetting>,
}

/// The harness, model, effort and mode `params` asks for, when `harnesses`
/// lists them. An effort is required for a model with effort levels and
/// refused for one without, and a mode likewise for a harness with modes.
fn chosen(harnesses: &[HarnessConfig], params: &RequestParams) -> Result<Chosen, ErrorBody> {
    let ids = |choices: &[Choice]| -> String {
        let ids: Vec<&str> = choices.iter().map(|choice| choice.id.as_str()).collect();
        ids.join(", ")
    };
    let wanted = &params.harness;
    let config = harnesses
        .iter()
        .find(|config| config.harness.executable() == wanted)
        .ok_or_else(|| {
            let names: Vec<&str> = harnesses
                .iter()
                .map(|config| config.harness.executable())
                .collect();
            invalid_request(format!(
                "{wanted} is not a harness Ox offers; it offers {}",
                names.join(", ")
            ))
        })?;
    let harness = config.harness.executable();
    let model = config
        .models
        .iter()
        .find(|model| model.id == params.model)
        .ok_or_else(|| {
            let models: Vec<&str> = config
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect();
            invalid_request(format!(
                "{} is not a {harness} model Ox offers; it offers {}",
                params.model,
                models.join(", ")
            ))
        })?;
    let effort = match (&params.effort, model.efforts.as_slice()) {
        (None, []) => None,
        (Some(effort), []) => {
            return Err(invalid_request(format!(
                "{effort} is not an effort Ox offers for {}; it has no effort levels",
                model.id
            )));
        }
        (None, efforts) => {
            return Err(invalid_request(format!(
                "{} needs an effort; Ox offers {}",
                model.id,
                ids(efforts)
            )));
        }
        (Some(wanted), efforts) => Some(
            efforts
                .iter()
                .find(|effort| effort.id == *wanted)
                .ok_or_else(|| {
                    invalid_request(format!(
                        "{wanted} is not an effort Ox offers for {}; it offers {}",
                        model.id,
                        ids(efforts)
                    ))
                })?,
        ),
    };
    let mode = match (&params.mode, config.modes.as_slice()) {
        (None, []) => None,
        (Some(mode), []) => {
            return Err(invalid_request(format!(
                "{mode} is not a {harness} mode Ox offers; it has no modes"
            )));
        }
        (None, listed) => {
            return Err(invalid_request(format!(
                "{harness} needs a mode; Ox offers {}",
                ids(listed)
            )));
        }
        (Some(wanted), listed) => Some(listed.iter().find(|mode| mode.id == *wanted).ok_or_else(
            || {
                invalid_request(format!(
                    "{wanted} is not a {harness} mode Ox offers; it offers {}",
                    ids(listed)
                ))
            },
        )?),
    };
    let setting = |config_id: &str, value: &str| ConfigSetting {
        config_id: config_id.into(),
        value: ConfigValue::Id(value.into()),
    };
    let mut settings = vec![setting("model", &model.id)];
    settings.extend(effort.map(|effort| setting("effort", &effort.id)));
    settings.extend(mode.map(|mode| setting("mode", &mode.id)));
    Ok(Chosen {
        harness: config.harness,
        model: model.name.clone(),
        provider: model.provider.clone(),
        effort: effort.map(|effort| effort.name.clone()),
        mode: mode.map(|mode| mode.name.clone()),
        config: settings,
    })
}

/// The path of the ACP bridge of `harness`, which sits beside the daemon's
/// executable.
fn bridge(harness: Harness) -> Result<PathBuf, ErrorBody> {
    let bridge = std::env::current_exe()
        .map_err(|error| ErrorBody {
            code: code::INVALID_STATE,
            message: format!("finding the daemon's executable: {error}"),
        })?
        .with_file_name(harness.bridge());
    if !bridge.is_file() {
        return Err(invalid_state(&format!(
            "the {} bridge is not installed at {}",
            harness.executable(),
            bridge.display()
        )));
    }
    Ok(bridge)
}

/// The text blocks of a prompt, joined by newlines.
fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The current time in milliseconds since the Unix epoch.
pub fn now() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch");
    u64::try_from(since_epoch.as_millis()).expect("the time fits a u64")
}

/// `path` with symbolic links resolved, or as it is when it cannot be.
fn resolved(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// The workspace as clients see it.
/// The items whose version differs from the one in `sent`, and the IDs in
/// `sent` with no item.
fn stale<T>(
    items: &HashMap<String, Arc<T>>,
    sent: &HashMap<String, u64>,
    version: impl Fn(&T) -> u64,
) -> (Vec<Arc<T>>, Vec<String>) {
    let outdated = items
        .iter()
        .filter(|(id, item)| sent.get(*id) != Some(&version(item)))
        .map(|(_, item)| item.clone())
        .collect();
    let removed = sent
        .keys()
        .filter(|id| !items.contains_key(*id))
        .cloned()
        .collect();
    (outdated, removed)
}

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

pub(crate) fn invalid_state(message: &str) -> ErrorBody {
    ErrorBody {
        code: code::INVALID_STATE,
        message: message.into(),
    }
}

pub(crate) fn storage(error: &rusqlite::Error) -> ErrorBody {
    ErrorBody {
        code: code::STORAGE,
        message: format!("the database failed: {error}"),
    }
}

fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).expect("protocol types serialize")
}

/// Whether nothing keeps the daemon running: no client and no session whose
/// agent is running.
fn nothing_running(clients: usize, mut sessions: impl Iterator<Item = SessionState>) -> bool {
    clients == 0 && sessions.all(|state| state == SessionState::Exited)
}

/// Resolves once `is_idle` has held for `delay` with no change signalled on
/// `changed`. Each change restarts the delay.
#[expect(
    clippy::disallowed_methods,
    reason = "the idle exit delay is the behavior"
)]
async fn idle_for(mut changed: watch::Receiver<()>, delay: Duration, is_idle: impl Fn() -> bool) {
    loop {
        // Marking the change seen before reading the state means a change
        // made while it is read wakes this again.
        changed.borrow_and_update();
        if is_idle() {
            tokio::select! {
                () = tokio::time::sleep(delay) => return,
                _ = changed.changed() => {}
            }
        } else {
            let _ = changed.changed().await;
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "the tests move the paused clock by sleeping"
)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::time::Instant;

    use super::*;

    #[test]
    fn only_clients_and_live_sessions_keep_the_daemon_running() {
        use SessionState::*;
        for (clients, sessions, expected) in [
            (0, vec![], true),
            (0, vec![Exited, Exited], true),
            (1, vec![], false),
            (0, vec![Exited, Starting], false),
            (0, vec![Idle], false),
            (0, vec![Running], false),
            (0, vec![AwaitingPermission], false),
            (0, vec![AwaitingInput], false),
        ] {
            assert_eq!(
                nothing_running(clients, sessions.iter().copied()),
                expected,
                "{clients} clients, sessions {sessions:?}"
            );
        }
    }

    const DELAY: Duration = Duration::from_secs(5);
    const TICK: Duration = Duration::from_millis(1);

    /// Waits on `idle_for` in a task, which sends the time it resolved.
    fn idle_task(
        changed: &watch::Sender<()>,
        idle: &Arc<AtomicBool>,
    ) -> tokio::sync::oneshot::Receiver<Instant> {
        let (resolved, resolution) = tokio::sync::oneshot::channel();
        let changed = changed.subscribe();
        let idle = idle.clone();
        tokio::spawn(async move {
            idle_for(changed, DELAY, || idle.load(Ordering::SeqCst)).await;
            let _ = resolved.send(Instant::now());
        });
        resolution
    }

    // These tests run on a paused clock: it moves only when every task waits
    // on a timer, and then straight to the earliest one.

    #[tokio::test(start_paused = true)]
    async fn a_change_restarts_the_delay_and_busy_time_does_not_count() {
        let changed = watch::Sender::new(());
        let idle = Arc::new(AtomicBool::new(true));
        let mut resolution = idle_task(&changed, &idle);
        tokio::time::sleep(DELAY / 2).await;
        idle.store(false, Ordering::SeqCst);
        changed.send_replace(());
        tokio::time::sleep(DELAY * 10).await;
        assert!(resolution.try_recv().is_err(), "resolved while busy");

        idle.store(true, Ordering::SeqCst);
        changed.send_replace(());
        let idle_since = Instant::now();
        tokio::time::sleep(DELAY / 2).await;
        // A change that leaves it idle restarts the delay too.
        changed.send_replace(());
        let restarted = Instant::now();
        tokio::time::sleep(DELAY / 2 + TICK).await;
        assert!(Instant::now() - idle_since > DELAY);
        assert!(
            resolution.try_recv().is_err(),
            "resolved by the first delay"
        );
        assert_eq!(resolution.await.unwrap() - restarted, DELAY);
    }
}
