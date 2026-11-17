use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ox_protocol::schema::{
    ContentBlock, PermissionOption, PermissionOptionKind, PromptCapabilities,
    RequestPermissionOutcome, SessionConfigKind, SessionConfigOptionCategory,
    SessionConfigSelectOptions, TextContent,
};
use ox_protocol::{
    Branches, Checkout, ConfigSetting, ConfigValue, Deleted, ErrorBody, Harness, ListingChange,
    MessageSent, Method, QueueAdded, Sender, SessionInfo, SessionState, WorkspaceInfo, code,
};
use serde_json::Value;
use tokio::sync::{OwnedMutexGuard, watch};

use crate::branches;
use crate::directories::Directories;
use crate::mcp::OxServer;
use crate::session::{self, Opening, Session};
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
    /// A daemon with the workspaces, worktrees and sessions `store` holds,
    /// which adds worktrees in `worktrees_dir` and parks the agent of a
    /// session that has been idle, with no background work, and unviewed for
    /// `park_after`, whose directory watches follow `directories`, and whose
    /// sessions' agents are given `ox_server`. Each saved session is
    /// `exited`, with no agent, until it loads. A saved session that never
    /// started has nothing to load. One without a worktree is deleted; one
    /// with a worktree stays `exited`.
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
        let mut sessions = HashMap::new();
        let saved_sessions = store.sessions()?;
        for saved in saved_sessions {
            let worktree = saved.worktree_id.as_ref().map(|id| {
                worktrees
                    .get(id)
                    .expect("a session's worktree is saved")
                    .clone()
            });
            // TODO: A never-started session's first prompt is not shown
            // after a restart.
            if saved.agent_session_id.is_none() && worktree.is_none() {
                store.remove_session(&saved.id)?;
                continue;
            }
            let state = match saved.agent_session_id.clone() {
                Some(agent_session_id) => State::restored(
                    saved.id.clone(),
                    saved.title.clone(),
                    agent_session_id,
                    saved.last_seq,
                    saved.queue.clone(),
                ),
                None => {
                    State::unstarted(saved.id.clone(), saved.title.clone(), saved.queue.clone())
                }
            };
            let session = Session::new(
                store.clone(),
                changed.clone(),
                park_after,
                ox_server.clone(),
                saved,
                worktree,
                state,
            );
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

    /// Calls `send` on a session, which may start a turn, unless its
    /// checkout's branch is moving. The registry stays locked throughout, so
    /// no move starts in between.
    fn send_unmoved<R>(
        &self,
        id: &str,
        send: impl FnOnce(&Session) -> Result<R, ErrorBody>,
    ) -> Result<R, ErrorBody> {
        let registry = self.lock();
        let session = registry
            .sessions
            .get(id)
            .ok_or_else(|| not_found("session", id))?;
        registry.refuse_moving(session.cwd())?;
        send(session)
    }

    fn worktree(&self, id: &str) -> Result<Arc<Worktree>, ErrorBody> {
        self.lock()
            .worktrees
            .get(id)
            .cloned()
            .ok_or_else(|| not_found("worktree", id))
    }

    /// Sends session `session_id`'s agent a config option change, ahead of
    /// every later command to the session. The returned future resolves to
    /// the agent's resulting options.
    pub fn set_config_option(
        &self,
        session_id: &str,
        config_id: String,
        value: ConfigValue,
    ) -> Result<impl Future<Output = Result<Value, ErrorBody>> + use<>, ErrorBody> {
        let options = self
            .session(session_id)?
            .set_config_option(config_id, value)?;
        Ok(async move { Ok(to_value(options.await?)) })
    }

    /// Handles every method except `hello`, `session.subscribe`, `watch`, and
    /// `unsubscribe`, which belong to the connection.
    pub async fn handle(self: &Arc<Self>, method: Method) -> Result<Value, ErrorBody> {
        match method {
            Method::WorkspaceAdd { path } => Ok(to_value(self.add_workspace(path)?)),
            Method::WorkspaceRemove { workspace_id } => {
                self.remove_workspace(&workspace_id)?;
                Ok(Value::Null)
            }
            Method::WorkspaceBranches { workspace_id, env } => {
                Ok(to_value(self.branches(&workspace_id, env).await?))
            }
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
                self.send_unmoved(&session_id, |session| session.prompt(prompt))?;
                Ok(Value::Null)
            }
            Method::SessionQueueAdd { session_id, prompt } => {
                let prompt_id =
                    self.send_unmoved(&session_id, |session| session.queue_add(prompt))?;
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
                self.send_unmoved(&session_id, |session| session.queue_send(prompt_id))?;
                Ok(Value::Null)
            }
            Method::SessionSetConfigOption {
                session_id,
                config_id,
                value,
            } => self.set_config_option(&session_id, config_id, value)?.await,
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
            } => Ok(to_value(
                self.request_session(
                    &session_id,
                    SessionRequest {
                        prompt,
                        workspace,
                        worktree,
                        branch,
                        harness,
                        model,
                        effort,
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

    /// Creates a session in the workspace's directory or in a new worktree,
    /// with its first prompt and the option values its agent is set to.
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
        let opening = Opening {
            options: config
                .into_iter()
                .map(|setting| (setting.config_id, session::option_value(setting.value)))
                .collect(),
            prompt: None,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let Checkout::Worktree { compare, branch } = checkout else {
            return self.add_session(id, workspace_id, command, env, &mut None, opening, prompt);
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

    /// Saves and registers a session, with its worktree if it has one, and
    /// starts it with `opening`, taking the worktree to add. A first
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
            };
            let session = Session::new(
                self.store.clone(),
                self.changed.clone(),
                self.park_after,
                self.ox_server.clone(),
                saved,
                saved_worktree.map(|saved| Worktree::new(saved, self.changed.clone())),
                State::new(id.clone()),
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
        let Err(mut message) = worktree::add(&git, &plan).await else {
            session.start(env, opening);
            return;
        };
        if let Err(error) = self.store.remove_worktree(&live_worktree.id) {
            eprintln!(
                "oxd: forgetting the worktree of session {}: {error}",
                session.id
            );
            message = format!("{message}; forgetting the worktree failed: {error}");
        }
        self.update(|registry| registry.worktrees.remove(&live_worktree.id));
        live_worktree.close();
        session.fail_start(message);
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
        if recipient.state() == SessionState::Exited {
            let _lifecycle = self.lifecycle(&recipient).await?;
            recipient.history().await?;
            if recipient.state() == SessionState::Exited {
                let registry = self.lock();
                registry.refuse_moving(recipient.cwd())?;
                recipient.load(sender.env())?;
            }
        }
        recipient.started().await?;
        let from = Sender {
            session_id: sender.id.clone(),
            title: sender.title(),
        };
        let steered = self.send_unmoved(to, |session| session.message(text, from))?;
        Ok(MessageSent { steered })
    }

    /// Asks the user, through a permission request on session
    /// `requester_id`, to start the session `request` describes, once it is
    /// checked. Starting it, the new session gets the requester's
    /// environment. Resolves to `None` when the user does not start it.
    async fn request_session(
        self: &Arc<Self>,
        requester_id: &str,
        request: SessionRequest,
    ) -> Result<Option<SessionInfo>, ErrorBody> {
        let requester = self.session(requester_id)?;
        if request.prompt.trim().is_empty() {
            return Err(invalid_request("the prompt is empty".into()));
        }
        let (workspace_id, workspace) = match &request.workspace {
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
        let harness = match request.harness {
            Some(harness) => harness,
            None => Harness::of_command(requester.command()).ok_or_else(|| {
                invalid_request("this session's harness is not known; name one".into())
            })?,
        };
        // The harness's ACP bridge sits beside the daemon's executable.
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
        let env = requester.env();
        let git = Git::for_client(env.clone());
        let (checkout, branch_line) = if request.worktree {
            let worktree_id = uuid::Uuid::new_v4().to_string();
            worktree::plan(
                &git,
                &workspace,
                None,
                request.branch.clone(),
                &self.worktrees_dir,
                &worktree_id,
            )
            .await?;
            let head = branches::list(&git, &workspace).await?.head;
            let branch_line = match &request.branch {
                Some(branch) => branch.clone(),
                None => format!("New branch from {}", head.as_deref().unwrap_or("HEAD")),
            };
            let checkout = Checkout::Worktree {
                compare: head,
                branch: request.branch.clone(),
            };
            (checkout, Some(branch_line))
        } else {
            let head = match &request.branch {
                Some(branch) => {
                    let head = branches::list(&git, &workspace).await?.head;
                    if head.as_ref() != Some(branch) {
                        return Err(invalid_request(format!(
                            "the workspace's checkout is on {}, not {branch}",
                            head.as_deref().unwrap_or("no branch")
                        )));
                    }
                    head
                }
                None => branches::list(&git, &workspace)
                    .await
                    .ok()
                    .and_then(|b| b.head),
            };
            (Checkout::Workspace, head)
        };
        let config = settings(&bridge, harness, &workspace, &env, &request).await?;

        let name = workspace.file_name().map_or_else(
            || workspace.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let default = || "Default".to_owned();
        let mut description = format!(
            "Harness: {}\nModel: {}\nEffort: {}\nCheckout: {}\n",
            harness.executable(),
            request.model.clone().unwrap_or_else(default),
            request.effort.clone().unwrap_or_else(default),
            if request.worktree {
                "New worktree"
            } else {
                "Workspace checkout"
            },
        );
        if let Some(branch) = branch_line {
            let _ = writeln!(description, "Branch: {branch}");
        }
        let _ = write!(description, "\n{}", request.prompt);
        let options = vec![
            PermissionOption::new("start", "Start Session", PermissionOptionKind::AllowOnce),
            PermissionOption::new("decline", "Decline", PermissionOptionKind::RejectOnce),
        ];
        let outcome = requester
            .ask(format!("Start a session in {name}?"), description, options)
            .await;
        let RequestPermissionOutcome::Selected(selected) = outcome else {
            return Ok(None);
        };
        if *selected.option_id.0 != *"start" {
            return Ok(None);
        }
        let command = vec![bridge.to_string_lossy().into_owned()];
        let prompt = vec![ContentBlock::Text(TextContent::new(request.prompt))];
        self.new_session(&workspace_id, command, env, checkout, Some(prompt), config)
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

    /// Archives a session that is not starting: its agent is stopped and its
    /// worktree removed, with git run with `env`, and the branch is kept. A
    /// worktree that would lose work refuses the archive unless `discard` is
    /// set, leaving the session as it was, or `exited`
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
        match session.worktree() {
            Some(worktree) => {
                let git = Git::for_client(env);
                worktree
                    .remove(&git, discard, session.stop(Change::Parked))
                    .await?;
            }
            None => session.stop(Change::Parked).await,
        }
        self.store
            .set_archived(id, true)
            .map_err(|error| storage(&error))?;
        session.set_archived(true);
        Ok(session.info())
    }

    /// Adds an archived session's worktree again, with git run with
    /// `env`, and unarchives it.
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
            worktree.restore(&Git::for_client(env)).await?;
        }
        self.store
            .set_archived(id, false)
            .map_err(|error| storage(&error))?;
        session.set_archived(false);
        Ok(session.info())
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

    /// Asks every worktree of the repository with common Git directory
    /// `common_dir` to compare its branch again.
    fn recompare(&self, common_dir: &Path) {
        for worktree in self.lock().worktrees.values() {
            if worktree.saved().common_dir == common_dir {
                worktree.recompare();
            }
        }
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
struct SessionRequest {
    prompt: String,
    workspace: Option<PathBuf>,
    worktree: bool,
    branch: Option<String>,
    harness: Option<Harness>,
    model: Option<String>,
    effort: Option<String>,
}

/// The settings of the requested model and effort, each checked against
/// the values of the option of its category that the bridge's `--describe`
/// reports in `workspace`.
async fn settings(
    bridge: &Path,
    harness: Harness,
    workspace: &Path,
    env: &BTreeMap<String, String>,
    request: &SessionRequest,
) -> Result<Vec<ConfigSetting>, ErrorBody> {
    let wanted = [
        (SessionConfigOptionCategory::Model, "model", &request.model),
        (
            SessionConfigOptionCategory::ThoughtLevel,
            "effort",
            &request.effort,
        ),
    ];
    if wanted.iter().all(|(_, _, value)| value.is_none()) {
        return Ok(Vec::new());
    }
    let harness = harness.executable();
    let controls = ox_protocol::describe(bridge, workspace, env)
        .await
        .map_err(|error| ErrorBody {
            code: code::AGENT,
            message: format!("asking {harness} for its options: {error}"),
        })?;
    let mut settings = Vec::new();
    for (category, label, value) in wanted {
        let Some(value) = value else {
            continue;
        };
        let option = controls
            .config_options
            .iter()
            .find(|option| option.category.as_ref() == Some(&category))
            .ok_or_else(|| invalid_request(format!("{harness} has no {label} option")))?;
        let values: Vec<String> = match &option.kind {
            SessionConfigKind::Select(select) => match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => options
                    .iter()
                    .map(|option| option.value.to_string())
                    .collect(),
                SessionConfigSelectOptions::Grouped(groups) => groups
                    .iter()
                    .flat_map(|group| &group.options)
                    .map(|option| option.value.to_string())
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        if !values.contains(value) {
            return Err(invalid_request(format!(
                "{value} is not a {label} {harness} offers; it offers {}",
                values.join(", ")
            )));
        }
        settings.push(ConfigSetting {
            config_id: option.config_id.to_string(),
            value: ConfigValue::Id(value.clone()),
        });
    }
    Ok(settings)
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

fn refuse_starting(session: &Session) -> Result<(), ErrorBody> {
    if session.state() == SessionState::Starting {
        return Err(invalid_state("the session is still starting"));
    }
    Ok(())
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
    async fn idling_for_the_delay_resolves() {
        let changed = watch::Sender::new(());
        let idle = Arc::new(AtomicBool::new(true));
        let start = Instant::now();
        let mut resolution = idle_task(&changed, &idle);
        tokio::time::sleep(DELAY.saturating_sub(TICK)).await;
        assert!(resolution.try_recv().is_err());
        assert_eq!(resolution.await.unwrap() - start, DELAY);
    }

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
