use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v2::schema::{
    CancelSessionNotification, CancelledPermissionOutcome, ClientCapabilities, ContentBlock,
    CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationCapabilities, ElicitationFormCapabilities, Implementation, InitializeRequest,
    NewSessionRequest, PermissionOption, PromptRequest, ReplayFrom, ReplayFromStart,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SessionConfigOptionValue, SetSessionConfigOptionRequest, SystemPrompt,
    TextContent, UpdateSessionNotification,
};
use agent_client_protocol::v2::{ConnectionTo, InitializeOutcome, Responder};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use ox_protocol::{
    ConfigSetting, ConfigValue, ErrorBody, Event, EventKind, QueuedPrompt, Sender, SessionInfo,
    SessionRequest, SessionState, TranscriptPage, code,
};
use tokio::sync::{OwnedMutexGuard, mpsc, oneshot, watch};
use tokio::task::{AbortHandle, JoinHandle};

use crate::mcp::OxServer;
use crate::state::{self, Change, Request, Resume, State, invalid_state};
use crate::store::{SavedSession, Store};
use crate::transcript;
use crate::worktree::{Git, Worktree};

type ConfigReply = oneshot::Sender<Result<(), String>>;

/// Ox's instructions to an agent that has the `ox` tools.
const OX_INSTRUCTIONS: &str = "You are running in Ox, a desktop app for coding agents.\n\n\
    Use the `ox` tool `set_title` to give this session a short title once its task is clear, \
    and again if the task changes.";

/// How an agent task opens its ACP session: a new one, or a resumed one.
enum Launch {
    New(Opening),
    Resume(Resume),
}

/// What a new session's agent is given once it opens its ACP session.
pub struct Opening {
    /// The first prompt, with the seq of the `PromptStarted` event that
    /// `record_first_prompt` recorded for it, sent once the agent has
    /// accepted the session's config, and rejected when it has not.
    pub prompt: Option<(Vec<ContentBlock>, u64)>,
}

enum Command {
    /// A prompt, with the seq of the `PromptStarted` event that records it.
    Prompt {
        prompt: Vec<ContentBlock>,
        seq: u64,
    },
    Cancel,
    /// A new config, whose settings the agent is sent in order.
    SetConfig {
        config: Vec<ConfigSetting>,
        reply: ConfigReply,
    },
}

/// One ACP agent process and its event history. The database keeps the
/// session's workspace, command, directory, title, agent session ID, config,
/// and the ID of its worktree.
///
/// Everything about the session that changes is in `state`, and every change
/// goes through `update`. Lock order is the lifecycle guard, then the
/// daemon's registry, then `state`, then `agent`.
pub struct Session {
    pub id: String,
    pub number: u64,
    workspace_id: String,
    /// When the session was created, in milliseconds since the Unix epoch.
    created_at: u64,
    command: Vec<String>,
    cwd: PathBuf,
    store: Arc<Store>,
    /// The `ox` MCP server the agent is given.
    ox_server: Arc<OxServer>,
    /// The daemon's signal that the lists or the sessions' states changed,
    /// sent when this session's `SessionInfo` changes.
    changed: watch::Sender<()>,
    state: Mutex<State>,
    agent: Mutex<AgentTask>,
    /// Held while the session adds its worktree and launches its agent, and
    /// while it loads, is parked, archived, unarchived or deleted, or its
    /// workspace is removed.
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    /// Set once the stored history is in the state.
    history: tokio::sync::OnceCell<()>,
    /// The seq of the session's last event.
    last_seq: watch::Sender<u64>,
    /// Counts the changes to the session's `SessionInfo`.
    info_version: AtomicU64,
    /// Whether the session may be parked.
    parkable: Parkability,
    /// Stops the agent of a session that stays parkable for the park delay.
    parker: OnceLock<AbortHandle>,
}

/// The running agent: the environment it was launched with, over the
/// daemon's, the channel to its task, and the task. A load replaces them.
#[derive(Default)]
struct AgentTask {
    env: BTreeMap<String, String>,
    commands: Option<mpsc::UnboundedSender<Command>>,
    task: Option<JoinHandle<()>>,
}

/// Whether a session is quiet and unviewed. `lapses` counts the times it
/// stopped being so, which a change that reverts before the park task looks
/// still counts.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Parkable {
    parkable: bool,
    lapses: u64,
}

/// Whether a session may be parked, published to its park task.
#[derive(Default)]
struct Parkability(watch::Sender<Parkable>);

impl Parkability {
    /// Records whether the session is parkable now. Stopping being parkable
    /// is a lapse.
    fn set(&self, parkable: bool) {
        self.0.send_if_modified(|p| {
            if p.parkable == parkable {
                return false;
            }
            p.parkable = parkable;
            if !parkable {
                p.lapses += 1;
            }
            true
        });
    }

    /// Whether the session is parkable with no lapse since it had `lapses`.
    fn holds(&self, lapses: u64) -> bool {
        let p = *self.0.borrow();
        p.parkable && p.lapses == lapses
    }

    fn subscribe(&self) -> watch::Receiver<Parkable> {
        self.0.subscribe()
    }
}

/// Calls `park` each time `parkable` stays parkable for `delay`, with the
/// lapse count the delay started from. A lapse, even one that reverted
/// before this task looked, restarts the delay. Returns once the sender is
/// gone.
#[expect(clippy::disallowed_methods, reason = "the park delay is the behavior")]
async fn park_after<F: Future<Output = ()>>(
    mut parkable: watch::Receiver<Parkable>,
    delay: Duration,
    mut park: impl FnMut(u64) -> F,
) {
    loop {
        let Ok(seen) = parkable.wait_for(|p| p.parkable).await.map(|p| p.lapses) else {
            return;
        };
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            lapsed = parkable.wait_for(|p| !p.parkable || p.lapses != seen) => {
                if lapsed.is_err() {
                    return;
                }
                continue;
            }
        }
        park(seen).await;
    }
}

/// A subscription to a session's events. A session with one is not parked.
pub struct Viewer(Arc<Session>);

impl Drop for Viewer {
    fn drop(&mut self) {
        self.0.update(State::unview);
    }
}

/// A connection's focus on a session, counted while it lives.
pub struct SessionFocus(Arc<Session>);

impl Drop for SessionFocus {
    fn drop(&mut self) {
        self.0.update(State::unfocus);
    }
}

impl Session {
    /// The session `saved` in `state`: a session just saved is `starting`,
    /// and `start` launches its agent; a restored one is `exited`, and its
    /// stored history is read by `history`.
    pub fn new(
        store: Arc<Store>,
        changed: watch::Sender<()>,
        park_after: Duration,
        ox_server: Arc<OxServer>,
        saved: SavedSession,
        state: State,
    ) -> Arc<Self> {
        let session = Arc::new(Self {
            last_seq: watch::Sender::new(state.last_seq()),
            state: Mutex::new(state),
            agent: Mutex::default(),
            history: tokio::sync::OnceCell::new(),
            id: saved.id,
            number: saved.number,
            workspace_id: saved.workspace_id,
            created_at: saved.created_at,
            command: saved.command,
            cwd: saved.cwd,
            lifecycle: Arc::default(),
            store,
            ox_server,
            changed,
            info_version: AtomicU64::new(0),
            parkable: Parkability::default(),
            parker: OnceLock::new(),
        });
        let parker = tokio::spawn(session.clone().park_when_idle(park_after));
        session
            .parker
            .set(parker.abort_handle())
            .expect("the park task is set once");
        session
    }

    /// Parks the session each time it stays parkable for `delay`. Any change
    /// that makes it not parkable restarts the delay.
    async fn park_when_idle(self: Arc<Self>, delay: Duration) {
        park_after(self.parkable.subscribe(), delay, |seen| {
            let session = self.clone();
            async move {
                // A load, startup or delete in progress finishes first, and
                // the session is checked again after.
                let Ok(_lifecycle) = session.lifecycle().await else {
                    return;
                };
                if session.parkable.holds(seen) {
                    session.stop(Change::Parked).await;
                }
            }
        })
        .await;
    }

    /// Launches the agent of a session that is still `starting`, with what
    /// `opening` gives it. The session records `AgentStarted` once the ACP
    /// session is open, or `AgentExited` if it cannot be opened. A session
    /// closed before it started stays closed.
    pub fn start(self: &Arc<Self>, env: BTreeMap<String, String>, opening: Opening) {
        let state = self.lock();
        if state.state() == SessionState::Starting {
            self.launch(env, Launch::New(opening));
        }
    }

    /// Records the first prompt of a session that is still `starting` and
    /// returns the seq of its `PromptStarted` event. `start` sends it.
    pub fn record_first_prompt(&self, prompt: Vec<ContentBlock>) -> u64 {
        self.update(|state| state.record_first_prompt(prompt))
    }

    /// Records that the session's worktree could not be added: the session
    /// forgets its worktree and exits with `message`.
    pub fn fail_start(&self, message: String) {
        self.update(|state| state.fail_start(message));
    }

    /// Starts the agent task, which opens or resumes an ACP session. A new
    /// session's first prompt goes before its task's commands. Called under
    /// the state lock, so a `close` cannot miss the task.
    fn launch(self: &Arc<Self>, env: BTreeMap<String, String>, launch: Launch) {
        let (commands, commands_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(self.clone().run(commands_rx, env.clone(), launch));
        *self.agent.lock().unwrap() = AgentTask {
            env,
            commands: Some(commands),
            task: Some(task),
        };
    }

    /// Loads an exited session that has an agent session ID, once no other
    /// lifecycle operation runs: its history is read, and a new agent
    /// resumes it, with replay if the history has no recorded agent start.
    /// Records `AgentStarted` on success or `AgentExited` on failure.
    pub async fn load(self: &Arc<Self>, env: BTreeMap<String, String>) -> Result<(), ErrorBody> {
        let _lifecycle = self.lifecycle().await?;
        self.history().await?;
        self.resume(env)
    }

    /// Loads the session as `load` does if it is `exited` once no other
    /// lifecycle operation runs, and otherwise leaves it as it is.
    pub async fn load_if_exited(
        self: &Arc<Self>,
        env: BTreeMap<String, String>,
    ) -> Result<(), ErrorBody> {
        if self.state() != SessionState::Exited {
            return Ok(());
        }
        let _lifecycle = self.lifecycle().await?;
        self.history().await?;
        if self.state() != SessionState::Exited {
            return Ok(());
        }
        self.resume(env)
    }

    /// Starts a new agent on the session, which `State::load` checks.
    fn resume(self: &Arc<Self>, env: BTreeMap<String, String>) -> Result<(), ErrorBody> {
        if self.archived() {
            return Err(ErrorBody {
                code: code::INVALID_STATE,
                message: "the session is archived".into(),
            });
        }
        if !self.cwd.is_dir() {
            return Err(ErrorBody {
                code: code::INVALID_STATE,
                message: format!(
                    "the session's directory {} no longer exists",
                    self.cwd.display()
                ),
            });
        }
        self.update(|state| {
            let resume = state.load()?;
            self.launch(env, Launch::Resume(resume));
            Ok(())
        })
    }

    /// Archives a session that is not starting, once no other lifecycle
    /// operation runs: its agent is stopped and its worktree removed, with
    /// `git`, and the branch is kept. A worktree that would lose work
    /// refuses the archive unless `discard` is set, leaving the session as it
    /// was, or `exited` when the work appeared after its agent stopped.
    pub async fn archive(&self, git: &Git, discard: bool) -> Result<SessionInfo, ErrorBody> {
        self.refuse_starting()?;
        let _lifecycle = self.lifecycle().await?;
        self.refuse_starting()?;
        if self.archived() {
            return Err(crate::daemon::invalid_state(
                "the session is already archived",
            ));
        }
        match self.worktree() {
            Some(worktree) => {
                worktree
                    .remove(git, discard, self.stop(Change::Parked))
                    .await?;
            }
            None => self.stop(Change::Parked).await,
        }
        self.commit(|state| state.set_archived(true))?;
        Ok(self.info())
    }

    /// Adds an archived session's worktree again, with `git`, and unarchives
    /// it, once no other lifecycle operation runs.
    pub async fn unarchive(&self, git: &Git) -> Result<SessionInfo, ErrorBody> {
        let _lifecycle = self.lifecycle().await?;
        if !self.archived() {
            return Err(crate::daemon::invalid_state("the session is not archived"));
        }
        if let Some(worktree) = self.worktree() {
            worktree.restore(git).await?;
        }
        self.commit(|state| state.set_archived(false))?;
        Ok(self.info())
    }

    fn refuse_starting(&self) -> Result<(), ErrorBody> {
        if self.state() == SessionState::Starting {
            return Err(crate::daemon::invalid_state(
                "the session is still starting",
            ));
        }
        Ok(())
    }

    /// Marks a branch move under way in the session's checkout, which an
    /// `idle` or `exited` session allows; another gives its state.
    pub fn begin_move(&self) -> Result<(), SessionState> {
        self.update(State::begin_move)
    }

    /// Ends the branch move, and starts the queue's first prompt if the
    /// session can take it.
    pub fn end_move(&self) {
        self.update(State::end_move);
    }

    pub fn info(&self) -> SessionInfo {
        self.info_of(&self.lock())
    }

    /// The number of changes to the session's `SessionInfo` so far, which
    /// grows with each change.
    pub fn info_version(&self) -> u64 {
        self.info_version.load(Ordering::Acquire)
    }

    /// The session's `SessionInfo` and its version.
    pub fn versioned_info(&self) -> (SessionInfo, u64) {
        let state = self.lock();
        (self.info_of(&state), self.info_version())
    }

    pub fn archived(&self) -> bool {
        self.lock().archived()
    }

    /// Sets the title, with runs of whitespace collapsed to one space, and
    /// saves and publishes it. A title left empty is refused.
    pub fn set_title(&self, title: &str) -> Result<(), ErrorBody> {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if title.is_empty() {
            return Err(ErrorBody {
                code: code::INVALID_REQUEST,
                message: "the title is empty".into(),
            });
        }
        self.update(|state| state.set_title(title));
        Ok(())
    }

    /// The directory the session's agent runs in.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// The environment the agent was last launched with, over the daemon's:
    /// empty for a session restored from the database that has not loaded.
    pub fn env(&self) -> BTreeMap<String, String> {
        self.agent.lock().unwrap().env.clone()
    }

    pub fn title(&self) -> Option<String> {
        self.lock().title().map(String::from)
    }

    /// The ID of the agent's session, once the agent has opened one.
    pub fn agent_session_id(&self) -> Option<String> {
        self.lock().agent().map(|agent| agent.session_id.clone())
    }

    pub fn worktree(&self) -> Option<Arc<Worktree>> {
        self.lock().worktree().cloned()
    }

    /// The lifecycle guard of a session no one else has seen yet, such as one
    /// just created or restored.
    pub fn hold_lifecycle(&self) -> OwnedMutexGuard<()> {
        self.lifecycle
            .clone()
            .try_lock_owned()
            .expect("nothing else holds a new session's lifecycle lock")
    }

    /// Waits until no other lifecycle operation runs on the session, and
    /// keeps others from running while the guard lives. A session closed
    /// meanwhile, as deleting it or removing its workspace does, is not
    /// found.
    pub async fn lifecycle(&self) -> Result<OwnedMutexGuard<()>, ErrorBody> {
        let guard = self.lifecycle.clone().lock_owned().await;
        if self.lock().closed() {
            return Err(ErrorBody {
                code: code::NOT_FOUND,
                message: format!("no session {}", self.id),
            });
        }
        Ok(guard)
    }

    pub fn state(&self) -> SessionState {
        self.lock().state()
    }

    fn info_of(&self, state: &State) -> SessionInfo {
        let agent = state.agent();
        SessionInfo {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            state: state.state(),
            title: state.title().map(String::from),
            agent_session_id: agent.map(|agent| agent.session_id.clone()),
            prompt_capabilities: agent
                .map(|agent| agent.prompt_capabilities.clone())
                .unwrap_or_default(),
            config: state.config().to_vec(),
            available_commands: agent
                .map(|agent| agent.available_commands.clone())
                .unwrap_or_default(),
            worktree_id: state.worktree().map(|worktree| worktree.id.clone()),
            archived: state.archived(),
            created_at: self.created_at,
            unseen: state.unseen(),
            queue: state.queued().to_vec(),
        }
    }

    /// Counts a connection's focus on the session while the guard lives, and
    /// clears its unseen outcome.
    pub fn focus(self: &Arc<Self>) -> SessionFocus {
        self.update(State::focus);
        SessionFocus(self.clone())
    }

    /// Counts a subscription to the session's events while the guard lives.
    pub fn view(self: &Arc<Self>) -> Viewer {
        self.update(State::view);
        Viewer(self.clone())
    }

    /// Reads the session's stored history into its state, once. Callers wait
    /// for it before they subscribe to the session or load it.
    pub async fn history(&self) -> Result<(), ErrorBody> {
        self.history
            .get_or_try_init(|| async {
                if self.lock().loaded() {
                    return Ok(());
                }
                let store = self.store.clone();
                let id = self.id.clone();
                let history = tokio::task::spawn_blocking(move || store.events(&id))
                    .await
                    .expect("the history read does not panic")
                    .map_err(|error| ErrorBody {
                        code: code::STORAGE,
                        message: error.to_string(),
                    })?;
                self.update(|state| state.fill(history));
                Ok(())
            })
            .await
            .copied()
    }

    /// Checks that events follow `after_seq`, now or later.
    pub fn subscribe(&self, after_seq: u64) -> Result<(), ErrorBody> {
        self.lock().subscribe(after_seq)
    }

    /// Waits for events with `seq > after_seq` and returns the first of
    /// them, in order.
    pub async fn events_after(&self, after_seq: u64) -> Vec<Event> {
        self.last_seq
            .subscribe()
            .wait_for(|last_seq| *last_seq > after_seq)
            .await
            .expect("the session holds the sender");
        self.lock().events_after(after_seq)
    }

    /// Sends a prompt now, starting a turn or steering the running one.
    pub fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<(), ErrorBody> {
        self.send_prompt(Request::Prompt(prompt))
    }

    /// Sends a peer message from `from` now, starting a turn or steering the
    /// running one, and returns whether it steered.
    pub fn message(&self, text: String, from: Sender) -> Result<bool, ErrorBody> {
        self.update(|state| {
            let steered = state.in_turn();
            let (prompt, seq) = state
                .request(Request::Message { text, from })?
                .expect("a message sends a prompt");
            self.send(Command::Prompt { prompt, seq });
            Ok(steered)
        })
    }

    /// Waits until the session is not `starting`. A session whose agent
    /// exits instead gives the agent's error.
    pub async fn started(&self) -> Result<(), ErrorBody> {
        // `update` sends on `last_seq` under the state lock, so the state is
        // read here, after each change, not in a watch callback.
        let mut changes = self.last_seq.subscribe();
        loop {
            match self.state() {
                SessionState::Starting => {}
                SessionState::Exited => {
                    let state = self.lock();
                    let error = state
                        .events()
                        .iter()
                        .rev()
                        .find_map(|event| match &event.kind {
                            EventKind::AgentExited { error } => Some(error.clone()),
                            _ => None,
                        });
                    return Err(ErrorBody {
                        code: code::AGENT,
                        message: match error.flatten() {
                            Some(error) => format!("the agent did not start: {error}"),
                            None => "the agent exited while starting".into(),
                        },
                    });
                }
                _ => return Ok(()),
            }
            changes
                .changed()
                .await
                .expect("the session holds the sender");
        }
    }

    /// Asks the session's user, in a permission request of the daemon's own
    /// with `title` and `options`, to start the session `request` describes,
    /// and waits for the answer. A request the session cannot take, or that
    /// is cancelled, as stopping the turn does, is answered `cancelled`.
    pub async fn ask(
        &self,
        title: String,
        request: SessionRequest,
        options: Vec<PermissionOption>,
    ) -> RequestPermissionOutcome {
        let (answer, outcome) = oneshot::channel();
        self.record(Change::PermissionRequested {
            request: RequestPermissionRequest::new(self.id.clone(), title, options),
            session_request: Some(request),
            answer,
        });
        outcome
            .await
            .unwrap_or_else(|_| CancelledPermissionOutcome::new().into())
    }

    /// A page of the session's compact transcript, before seq `before`.
    pub async fn transcript(&self, before: Option<u64>) -> Result<TranscriptPage, ErrorBody> {
        self.history().await?;
        Ok(transcript::page(self.lock().events(), before))
    }

    /// Appends a prompt to the queue and returns its ID.
    pub fn queue_add(&self, prompt: Vec<ContentBlock>) -> Result<String, ErrorBody> {
        let id = uuid::Uuid::new_v4().to_string();
        self.update(|state| {
            state.request(Request::QueueAdd(QueuedPrompt {
                id: id.clone(),
                prompt,
            }))
        })?;
        Ok(id)
    }

    /// Replaces a queued prompt in place.
    pub fn queue_edit(&self, id: String, prompt: Vec<ContentBlock>) -> Result<(), ErrorBody> {
        self.update(|state| state.request(Request::QueueEdit(QueuedPrompt { id, prompt })))?;
        Ok(())
    }

    pub fn queue_remove(&self, id: String) -> Result<(), ErrorBody> {
        self.update(|state| state.request(Request::QueueRemove { id }))?;
        Ok(())
    }

    /// Takes a queued prompt out of the queue and sends it now.
    pub fn queue_send(&self, id: String) -> Result<(), ErrorBody> {
        self.send_prompt(Request::QueueSend { id })
    }

    /// Applies a request that sends a prompt, and sends it to the agent.
    fn send_prompt(&self, request: Request) -> Result<(), ErrorBody> {
        self.update(|state| {
            let (prompt, seq) = state
                .request(request)?
                .expect("a prompt request sends a prompt");
            self.send(Command::Prompt { prompt, seq });
            Ok(())
        })
    }

    /// Replaces the session's config: the agent is sent each setting, in
    /// order, ahead of every command sent after this returns, such as a
    /// prompt. The returned future resolves once the agent has answered them
    /// all, to the error naming each setting it refused.
    pub fn set_config(
        &self,
        config: Vec<ConfigSetting>,
    ) -> Result<impl Future<Output = Result<(), ErrorBody>> + use<>, ErrorBody> {
        let state = self.lock().state();
        if matches!(state, SessionState::Starting | SessionState::Exited) {
            return Err(invalid_state(state));
        }
        let (reply, result) = oneshot::channel();
        self.send(Command::SetConfig { config, reply });
        Ok(async move {
            result
                .await
                .unwrap_or_else(|_| Err("the session ended".into()))
                .map_err(|message| ErrorBody {
                    code: code::AGENT,
                    message,
                })
        })
    }

    /// Cancels the running turn and every pending permission request.
    pub fn cancel(&self) {
        self.send(Command::Cancel);
    }

    pub fn permit(&self, request_id: &str, option_id: &str) -> Result<(), ErrorBody> {
        self.update(|state| {
            state.request(Request::Permit {
                request_id: request_id.into(),
                option_id: option_id.into(),
            })
        })?;
        Ok(())
    }

    pub fn respond_elicitation(
        &self,
        request_id: &str,
        action: ElicitationAction,
    ) -> Result<(), ErrorBody> {
        self.update(|state| {
            state.request(Request::Respond {
                request_id: request_id.into(),
                action,
            })
        })?;
        Ok(())
    }

    /// The agent withdrew the elicitation whose answer `answer` receives.
    /// Closing the receiver under the state lock settles a race with a
    /// client's answer: an answer already sent wins and is returned, and
    /// otherwise the elicitation ends as `cancel`.
    fn withdraw_elicitation(
        &self,
        mut answer: oneshot::Receiver<ElicitationAction>,
    ) -> ElicitationAction {
        self.update(|state| {
            answer.close();
            answer.try_recv().unwrap_or_else(|_| {
                state.record(Change::ElicitationWithdrawn);
                ElicitationAction::Cancel
            })
        })
    }

    /// Stops the agent, killing its process group, and returns once the
    /// agent's task has ended. The session is `exited`, and keeps its
    /// subscriptions. `change` is what the session records: `Exited(None)`,
    /// or `Parked`.
    pub async fn stop(&self, change: Change) {
        let task = self.agent.lock().unwrap().task.take();
        if let Some(task) = task {
            task.abort();
            // The task ends cancelled, having dropped the agent.
            let _ = task.await;
        }
        self.record(change);
    }

    /// Stops the agent, killing its process group, and ends every
    /// subscription with `SessionClosed`.
    pub fn close(&self) {
        let task = self.update(|state| {
            state.record(Change::Closed);
            self.agent.lock().unwrap().task.take()
        });
        if let Some(task) = task {
            task.abort();
        }
        if let Some(parker) = self.parker.get() {
            parker.abort();
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Sends a command to the running agent task. A session whose agent has
    /// exited has none, and its commands are dropped.
    fn send(&self, command: Command) {
        if let Some(commands) = &self.agent.lock().unwrap().commands {
            let _ = commands.send(command);
        }
    }

    /// The values Ox set the agent to.
    fn config(&self) -> Vec<ConfigSetting> {
        self.lock().config().to_vec()
    }

    /// Changes the state, then, under the state lock, starts the queue's
    /// next prompt if the session can take it, records the unseen outcome
    /// the new events give, saves the changes to the session's row, queues
    /// the events for the database, asks the worktree for its comparison
    /// again when a turn ended, and publishes the seq of the last event,
    /// whether the session is parkable and, if its `SessionInfo` changed,
    /// the change. A failed save goes to oxd's log and the session carries
    /// on.
    fn update<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        let (result, saved) = self.apply(change);
        if let Err(error) = saved {
            eprintln!("oxd: saving session {}: {error}", self.id);
        }
        result
    }

    /// Changes the state as `update` does, and gives the error of a failed
    /// save.
    fn commit<R>(&self, change: impl FnOnce(&mut State) -> R) -> Result<R, ErrorBody> {
        let (result, saved) = self.apply(change);
        saved.map_err(|error| crate::daemon::storage(&error))?;
        Ok(result)
    }

    fn apply<R>(&self, change: impl FnOnce(&mut State) -> R) -> (R, rusqlite::Result<()>) {
        let mut state = self.lock();
        let before = self.info_of(&state);
        let result = change(&mut state);
        if let Some((prompt, seq)) = state.start_queued() {
            self.send(Command::Prompt { prompt, seq });
        }
        let events = state.take_unlogged();
        state.note_unseen(&events);
        let saved = self.store.save_session(&self.id, &state.take_unsaved());
        let turn_ended = events
            .iter()
            .any(|event| matches!(event.kind, EventKind::TurnEnded { .. }));
        if !events.is_empty() {
            self.store.log(events);
        }
        if turn_ended && let Some(worktree) = state.worktree() {
            worktree.recompare();
        }
        self.last_seq.send_replace(state.last_seq());
        self.parkable.set(state.parkable());
        if self.info_of(&state) != before {
            self.info_version.fetch_add(1, Ordering::Release);
            self.changed.send_replace(());
        }
        (result, saved)
    }

    fn record(&self, change: Change) {
        self.update(|state| state.record(change));
    }

    async fn run(
        self: Arc<Self>,
        mut commands: mpsc::UnboundedReceiver<Command>,
        env: BTreeMap<String, String>,
        launch: Launch,
    ) {
        let config = AcpAgentConfig::new(&self.command[0])
            .args(self.command[1..].iter().cloned())
            .envs(env);
        let session = self.clone();
        let result = Client
            .v2()
            .name("oxd")
            .on_receive_notification(
                {
                    let session = self.clone();
                    async move |notification: UpdateSessionNotification,
                                _connection: ConnectionTo<Agent>| {
                        session.record(Change::Update(notification.update));
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                {
                    let session = self.clone();
                    async move |request: RequestPermissionRequest,
                                responder: Responder<RequestPermissionResponse>,
                                connection: ConnectionTo<Agent>| {
                        let (answer, outcome) = oneshot::channel();
                        session.record(Change::PermissionRequested {
                            request,
                            session_request: None,
                            answer,
                        });
                        connection.spawn(async move {
                            let outcome = outcome
                                .await
                                .unwrap_or_else(|_| CancelledPermissionOutcome::new().into());
                            responder.respond(RequestPermissionResponse::new(outcome))
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .on_receive_request(
                {
                    let session = self.clone();
                    async move |request: CreateElicitationRequest,
                                responder: Responder<CreateElicitationResponse>,
                                connection: ConnectionTo<Agent>| {
                        let (message, tool_call_id, schema) = match state::check_form(request) {
                            Ok(form) => form,
                            Err(error) => return responder.respond_with_error(error),
                        };
                        let (answer, mut action) = oneshot::channel();
                        session.record(Change::ElicitationRequested {
                            message,
                            tool_call_id,
                            schema,
                            answer,
                        });
                        // `session/cancel` leaves an elicitation open; the
                        // agent withdraws it with `$/cancel_request`.
                        let withdrawn = responder.cancellation();
                        let session = session.clone();
                        connection.spawn(async move {
                            let action = tokio::select! {
                                biased;
                                action = &mut action => {
                                    action.unwrap_or(ElicitationAction::Cancel)
                                }
                                () = withdrawn.cancelled() => {
                                    session.withdraw_elicitation(action)
                                }
                            };
                            responder.respond(CreateElicitationResponse::new(action))
                        })
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(
                AcpAgent::new(config),
                async move |connection: ConnectionTo<Agent>| {
                    let InitializeOutcome::Initialized(initialized) = connection
                        .send_initialize(
                            InitializeRequest::new(
                                ProtocolVersion::V2,
                                Implementation::new("oxd", env!("CARGO_PKG_VERSION")),
                            )
                            .capabilities(
                                ClientCapabilities::new().elicitation(
                                    ElicitationCapabilities::new()
                                        .form(ElicitationFormCapabilities::new()),
                                ),
                            ),
                        )
                        .block_task()
                        .await?
                    else {
                        return Err(agent_client_protocol::Error::invalid_request()
                            .data("the agent does not speak ACP v2"));
                    };
                    let Some(capabilities) = initialized.capabilities.session else {
                        return Err(agent_client_protocol::Error::invalid_request()
                            .data("the agent does not support sessions"));
                    };
                    // A resuming agent sends any requested replay and reports
                    // its state as notifications before it answers. The
                    // session starts in an ordered callback on the answer,
                    // which the dispatch loop finishes before it reads what
                    // the agent sends next (the SDK's `concepts::ordering`
                    // chapter), so every later update finds it live.
                    let prompt_capabilities = capabilities.prompt.unwrap_or_default();
                    let mcp_servers = if capabilities.mcp.is_some_and(|mcp| mcp.stdio.is_some()) {
                        vec![session.ox_server.for_session(&session.id)]
                    } else {
                        Vec::new()
                    };
                    // An agent that takes a system prompt gets Ox's
                    // instructions in it; another gets them before a new
                    // session's first prompt.
                    // TODO: A new session whose agent exits before its first
                    // prompt is resumed without them.
                    let instructions =
                        (!mcp_servers.is_empty()).then(|| OX_INSTRUCTIONS.to_owned());
                    let takes_system_prompt = capabilities.system_prompt.is_some();
                    let system_prompt =
                        instructions
                            .clone()
                            .filter(|_| takes_system_prompt)
                            .map(|text| {
                                SystemPrompt::new(vec![ContentBlock::Text(TextContent::new(text))])
                            });
                    let mut preamble = instructions
                        .filter(|_| !takes_system_prompt && matches!(launch, Launch::New(_)));
                    // The config as the session was launched with it, before
                    // a session without one takes the agent's.
                    let config = session.config();
                    let (opened, open) = oneshot::channel();
                    let start = {
                        let session = session.clone();
                        move |result: Result<state::Agent, _>| {
                            let result = result.map(|agent| {
                                let id = agent.session_id.clone();
                                session.record(Change::Started(agent));
                                id
                            });
                            let _ = opened.send(result);
                        }
                    };
                    let opening = match launch {
                        Launch::Resume(Resume { session_id, replay }) => {
                            connection
                                .prepare_request(
                                    ResumeSessionRequest::new(
                                        session_id.clone(),
                                        session.cwd.clone(),
                                    )
                                    .replay_from(
                                        replay.then(|| ReplayFrom::Start(ReplayFromStart::new())),
                                    )
                                    .mcp_servers(mcp_servers)
                                    .system_prompt(system_prompt),
                                )
                                .on_receiving_result(async move |result| {
                                    start(result.map(|resumed| state::Agent {
                                        session_id,
                                        prompt_capabilities,
                                        config_options: resumed.config_options,
                                        available_commands: resumed.available_commands,
                                    }));
                                    Ok(())
                                })?;
                            None
                        }
                        Launch::New(opening) => {
                            connection
                                .prepare_request(
                                    NewSessionRequest::new(session.cwd.clone())
                                        .mcp_servers(mcp_servers)
                                        .system_prompt(system_prompt),
                                )
                                .on_receiving_result(async move |result| {
                                    start(result.map(|created| state::Agent {
                                        session_id: created.session_id.to_string(),
                                        prompt_capabilities,
                                        config_options: created.config_options,
                                        available_commands: created.available_commands,
                                    }));
                                    Ok(())
                                })?;
                            Some(opening.prompt)
                        }
                    };
                    let acp_session = open.await.map_err(|_| {
                        agent_client_protocol::Error::internal_error()
                            .data("the agent closed the connection before the session opened")
                    })??;
                    // The agent is set to the session's config. A new
                    // session's first prompt waits for the answers, and is
                    // not sent when the agent refuses a setting. A resumed
                    // agent is set to values it accepted before, so its
                    // answers are not waited for.
                    let answers = send_config(&connection, &acp_session, config);
                    let mut first = None;
                    match opening {
                        Some(prompt) => {
                            let refused = state::refusals(&answers.await);
                            match prompt {
                                Some((prompt, seq)) if refused.is_empty() => {
                                    first = Some(Command::Prompt { prompt, seq });
                                }
                                Some((_, seq)) => session.record(Change::PromptRejected {
                                    prompt: seq,
                                    error: refused.join("\n"),
                                }),
                                None => {
                                    for refusal in refused {
                                        eprintln!("oxd: session {}: {refusal}", session.id);
                                    }
                                }
                            }
                        }
                        None => {
                            let id = session.id.clone();
                            connection.spawn(async move {
                                for refusal in state::refusals(&answers.await) {
                                    eprintln!("oxd: session {id}: {refusal}");
                                }
                                Ok(())
                            })?;
                        }
                    }
                    loop {
                        let command = match first.take() {
                            Some(command) => Some(command),
                            None => tokio::select! {
                                () = connection.incoming_closed() => break,
                                command = commands.recv() => command,
                            },
                        };
                        match command {
                            None => break,
                            Some(Command::Prompt { prompt, seq }) => {
                                let prompt = match preamble.take() {
                                    Some(text) => {
                                        std::iter::once(ContentBlock::Text(TextContent::new(text)))
                                            .chain(prompt)
                                            .collect()
                                    }
                                    None => prompt,
                                };
                                let session = session.clone();
                                connection
                                    .prepare_request(PromptRequest::new(
                                        acp_session.clone(),
                                        prompt,
                                    ))
                                    .on_receiving_result(async move |result| {
                                        // The answer says whether the agent
                                        // inserted the prompt; the turn runs
                                        // and ends on its `state_update`s.
                                        session.record(match result {
                                            Ok(_) => Change::PromptAccepted { prompt: seq },
                                            Err(error) => Change::PromptRejected {
                                                prompt: seq,
                                                error: error.to_string(),
                                            },
                                        });
                                        Ok(())
                                    })?;
                            }
                            Some(Command::Cancel) => {
                                connection.send_notification(CancelSessionNotification::new(
                                    acp_session.clone(),
                                ))?;
                                session.record(Change::Cancelled);
                            }
                            Some(Command::SetConfig { config, reply }) => {
                                let answers = send_config(&connection, &acp_session, config);
                                let session = session.clone();
                                connection.spawn(async move {
                                    let answers = answers.await;
                                    let _ = reply
                                        .send(session.update(|state| state.settle_config(answers)));
                                    Ok(())
                                })?;
                            }
                        }
                    }
                    Ok(())
                },
            )
            .await;
        self.record(Change::Exited(result.err().map(|error| error.to_string())));
    }
}

/// Sends the agent of ACP session `session_id` each setting of `config`, in
/// order, and returns the future of the settings with the error of each one
/// the agent refused.
fn send_config(
    connection: &ConnectionTo<Agent>,
    session_id: &str,
    config: Vec<ConfigSetting>,
) -> impl Future<Output = Vec<(ConfigSetting, Option<String>)>> + use<> {
    let sent: Vec<_> = config
        .into_iter()
        .map(|setting| {
            let request = connection.send_request(SetSessionConfigOptionRequest::new(
                session_id.to_owned(),
                setting.config_id.clone(),
                option_value(setting.value.clone()),
            ));
            (setting, request)
        })
        .collect();
    async move {
        let mut answers = Vec::with_capacity(sent.len());
        for (setting, request) in sent {
            let refusal = request.block_task().await.err();
            answers.push((setting, refusal.map(|error| error.to_string())));
        }
        answers
    }
}

/// The ACP value of an option value from a client.
fn option_value(value: ConfigValue) -> SessionConfigOptionValue {
    match value {
        ConfigValue::Boolean(value) => SessionConfigOptionValue::boolean(value),
        ConfigValue::Id(id) => SessionConfigOptionValue::id(id),
    }
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "the tests move the paused clock by sleeping"
)]
mod tests {
    use tokio::sync::{Notify, mpsc};
    use tokio::time::Instant;

    use super::*;

    const DELAY: Duration = Duration::from_mins(1);
    const TICK: Duration = Duration::from_millis(1);

    /// A park task over `parkability` that parks as a session does: once
    /// `lifecycle` lets it, if no lapse came since its delay started, after
    /// which the session is not parkable. It sends the time of each park.
    fn parker(
        parkability: &Arc<Parkability>,
        lifecycle: Option<Arc<Notify>>,
    ) -> mpsc::UnboundedReceiver<Instant> {
        let (parked, parks) = mpsc::unbounded_channel();
        let parkability = parkability.clone();
        tokio::spawn(async move {
            park_after(parkability.subscribe(), DELAY, |seen| {
                let (parkability, lifecycle, parked) =
                    (parkability.clone(), lifecycle.clone(), parked.clone());
                async move {
                    if let Some(lifecycle) = lifecycle {
                        lifecycle.notified().await;
                    }
                    if parkability.holds(seen) {
                        parkability.set(false);
                        let _ = parked.send(Instant::now());
                    }
                }
            })
            .await;
        });
        parks
    }

    // These tests run on a paused clock: it moves only when every task waits
    // on a timer, and then straight to the earliest one.

    #[tokio::test(start_paused = true)]
    async fn a_parkable_session_is_parked_once_the_delay_passes() {
        let parkability = Arc::new(Parkability::default());
        let mut parks = parker(&parkability, None);
        let start = Instant::now();
        parkability.set(true);
        tokio::time::sleep(DELAY.saturating_sub(TICK)).await;
        assert!(parks.try_recv().is_err());
        assert_eq!(parks.recv().await.unwrap() - start, DELAY);
    }

    #[tokio::test(start_paused = true)]
    async fn a_lapse_that_reverts_before_the_park_task_looks_restarts_the_delay() {
        let parkability = Arc::new(Parkability::default());
        let mut parks = parker(&parkability, None);
        parkability.set(true);
        tokio::time::sleep(DELAY / 2).await;
        parkability.set(false);
        parkability.set(true);
        let restarted = Instant::now();
        tokio::time::sleep(DELAY / 2 + TICK).await;
        assert!(parks.try_recv().is_err(), "parked by the first delay");
        assert_eq!(parks.recv().await.unwrap() - restarted, DELAY);
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_is_parked_only_while_it_stays_parkable() {
        let parkability = Arc::new(Parkability::default());
        let mut parks = parker(&parkability, None);
        parkability.set(true);
        tokio::time::sleep(DELAY / 2).await;
        parkability.set(false);
        tokio::time::sleep(DELAY * 10).await;
        assert!(parks.try_recv().is_err());

        parkability.set(true);
        let parkable = Instant::now();
        assert_eq!(parks.recv().await.unwrap() - parkable, DELAY);
        // Parked again each time it becomes parkable.
        parkability.set(true);
        let again = Instant::now();
        assert_eq!(parks.recv().await.unwrap() - again, DELAY);
    }

    #[tokio::test(start_paused = true)]
    async fn a_lapse_while_the_park_waits_for_the_lifecycle_restarts_the_delay() {
        let parkability = Arc::new(Parkability::default());
        let lifecycle = Arc::new(Notify::new());
        let mut parks = parker(&parkability, Some(lifecycle.clone()));
        parkability.set(true);
        // The delay passes and the park waits for the lifecycle.
        tokio::time::sleep(DELAY + TICK).await;
        parkability.set(false);
        parkability.set(true);
        let restarted = Instant::now();
        lifecycle.notify_one();
        tokio::time::sleep(DELAY.saturating_sub(TICK)).await;
        assert!(parks.try_recv().is_err(), "parked after a lapse");
        lifecycle.notify_one();
        assert_eq!(parks.recv().await.unwrap() - restarted, DELAY);
    }

    #[test]
    fn parkability_holds_until_a_lapse() {
        let parkability = Parkability::default();
        assert!(!parkability.holds(0));
        parkability.set(true);
        assert!(parkability.holds(0));
        parkability.set(true);
        assert!(parkability.holds(0), "setting it again is not a lapse");
        parkability.set(false);
        parkability.set(true);
        assert!(!parkability.holds(0));
        assert!(parkability.holds(1));
    }
}
