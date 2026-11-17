use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v2::schema::{
    CancelSessionNotification, CancelledPermissionOutcome, ClientCapabilities, ContentBlock,
    CreateElicitationRequest, CreateElicitationResponse, ElicitationAction,
    ElicitationCapabilities, ElicitationFormCapabilities, Implementation, InitializeRequest,
    NewSessionRequest, PromptRequest, ReplayFrom, ReplayFromStart, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionRequest, SessionConfigOption, SessionConfigOptionValue,
    SetSessionConfigOptionRequest, SystemPrompt, TextContent, UpdateSessionNotification,
};
use agent_client_protocol::v2::{ConnectionTo, InitializeOutcome, Responder};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use ox_protocol::{
    ConfigValue, ErrorBody, Event, EventKind, SessionInfo, SessionState, Unseen, code,
};
use tokio::sync::{OwnedMutexGuard, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::instructions::{self, StepRole};
use crate::mcp::OxServer;
use crate::state::{self, Change, Request, Resume, State, invalid_state};
use crate::store::{SavedSession, Store};
use crate::worktree::Worktree;

type ConfigReply = oneshot::Sender<Result<Vec<SessionConfigOption>, String>>;

/// How an agent task opens its ACP session: a new one, or a resumed one.
enum Launch {
    New(Opening),
    Resume(Resume),
}

/// What a new session's agent is given once it opens its ACP session.
#[derive(Default)]
pub struct Opening {
    /// The workflow step a run's child session runs.
    pub step: Option<StepRole>,
    /// Option values the agent is set to where it reports others.
    pub options: Vec<(String, SessionConfigOptionValue)>,
    /// The first prompt, with the seq of the `PromptStarted` event that
    /// `queue_prompt` recorded for it, sent after the options.
    pub prompt: Option<(Vec<ContentBlock>, u64)>,
}

enum Command {
    /// A prompt, with the seq of the `PromptStarted` event that records it.
    Prompt {
        prompt: Vec<ContentBlock>,
        seq: u64,
    },
    Cancel,
    SetConfigOption {
        config_id: String,
        value: SessionConfigOptionValue,
        reply: ConfigReply,
    },
}

/// One ACP agent process and its event history. The database keeps the
/// session's workspace, command, directory, title, agent session ID, and
/// the ID of its worktree.
pub struct Session {
    pub id: String,
    pub number: u64,
    workspace_id: String,
    /// When the session was created, in milliseconds since the Unix epoch.
    created_at: u64,
    command: Vec<String>,
    cwd: PathBuf,
    /// The worktree the session runs in. Unset when it could not be added.
    worktree: Mutex<Option<Arc<Worktree>>>,
    /// Whether the session is archived.
    archived: Mutex<bool>,
    /// Held while the session adds its worktree and launches its agent,
    /// while it loads, and while it is deleted.
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    store: Arc<Store>,
    /// The `ox` MCP server the agent is given.
    ox_server: Arc<OxServer>,
    state: Mutex<State>,
    /// Set once the stored history is in the state.
    history: tokio::sync::OnceCell<()>,
    /// The seq of the session's last event. Updated under the state lock.
    last_seq: watch::Sender<u64>,
    /// The daemon's signal that the lists or the sessions' states changed,
    /// sent when this session's `SessionInfo` changes.
    changed: watch::Sender<()>,
    /// Counts the changes to the session's `SessionInfo`. Changed under the
    /// state lock.
    info_version: AtomicU64,
    /// Sends to the running agent task. A load replaces it.
    commands: Mutex<mpsc::UnboundedSender<Command>>,
    task: Mutex<Option<JoinHandle<()>>>,
    /// The subscriptions to the session's events. Locked after the state.
    viewers: Mutex<usize>,
    /// Whether the session may be parked.
    parkable: Parkability,
    /// Stops the agent of a session that stays parkable for the park delay.
    parker: Mutex<Option<JoinHandle<()>>>,
    /// How the session's turns ended while it was out of focus. Changed
    /// under the state lock.
    unseen: Mutex<Option<Unseen>>,
    /// The connections that focus the session.
    focused: AtomicUsize,
    /// The focus count of the run whose child the session is.
    run_focused: Option<Arc<AtomicUsize>>,
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
        self.0.update(|_| *self.0.viewers.lock().unwrap() -= 1);
    }
}

/// A connection's focus on a session, counted while it lives.
pub struct SessionFocus(Arc<Session>);

impl Drop for SessionFocus {
    fn drop(&mut self) {
        self.0.focused.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Session {
    /// The session `saved` in `state`, in `worktree` if it runs in one: a
    /// session just saved is `starting`, and `start` launches its agent; a
    /// restored one is `exited`, and its stored history is read by
    /// `history`. A run's child session shares the run's focus count,
    /// `run_focused`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the daemon's shared parts and the session's saved parts"
    )]
    pub fn new(
        store: Arc<Store>,
        changed: watch::Sender<()>,
        park_after: Duration,
        ox_server: Arc<OxServer>,
        saved: SavedSession,
        worktree: Option<Arc<Worktree>>,
        state: State,
        run_focused: Option<Arc<AtomicUsize>>,
    ) -> Arc<Self> {
        let session = Arc::new(Self {
            last_seq: watch::Sender::new(state.last_seq()),
            state: Mutex::new(state),
            history: tokio::sync::OnceCell::new(),
            id: saved.id,
            number: saved.number,
            workspace_id: saved.workspace_id,
            created_at: saved.created_at,
            command: saved.command,
            cwd: saved.cwd,
            worktree: Mutex::new(worktree),
            archived: Mutex::new(saved.archived),
            lifecycle: Arc::default(),
            store,
            ox_server,
            changed,
            info_version: AtomicU64::new(0),
            commands: Mutex::new(mpsc::unbounded_channel().0),
            task: Mutex::new(None),
            viewers: Mutex::new(0),
            parkable: Parkability::default(),
            parker: Mutex::new(None),
            unseen: Mutex::new(saved.unseen),
            focused: AtomicUsize::new(0),
            run_focused,
        });
        *session.parker.lock().unwrap() =
            Some(tokio::spawn(session.clone().park_when_idle(park_after)));
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
                let _lifecycle = session.lifecycle().await;
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
        // Holding the task slot keeps a concurrent `close` from missing the
        // agent's task.
        let mut task = self.task.lock().unwrap();
        if self.state() == SessionState::Starting {
            self.launch(&mut task, env, Launch::New(opening));
        }
    }

    /// Records the first prompt of a session that is still `starting` and
    /// returns the seq of its `PromptStarted` event. `start` sends it.
    pub fn queue_prompt(&self, prompt: Vec<ContentBlock>) -> u64 {
        self.update(|state| state.queue_prompt(prompt))
    }

    /// Records that the session's worktree could not be added: the session
    /// has no worktree and exits with `message`.
    pub fn fail_start(&self, message: String) {
        self.update(|state| {
            *self.worktree.lock().unwrap() = None;
            state.record(Change::Exited(Some(message)));
        });
    }

    /// Starts the agent task in `task`, which opens or resumes an ACP
    /// session. A new session's first prompt is its task's first command.
    fn launch(
        self: &Arc<Self>,
        task: &mut Option<JoinHandle<()>>,
        env: BTreeMap<String, String>,
        mut launch: Launch,
    ) {
        let (commands, commands_rx) = mpsc::unbounded_channel();
        if let Launch::New(opening) = &mut launch
            && let Some((prompt, seq)) = opening.prompt.take()
        {
            let _ = commands.send(Command::Prompt { prompt, seq });
        }
        *self.commands.lock().unwrap() = commands;
        *task = Some(tokio::spawn(self.clone().run(commands_rx, env, launch)));
    }

    /// Starts a new agent on an exited session that has an agent session ID,
    /// resuming it with replay if the history has no recorded agent start.
    /// Records `AgentStarted` on success or `AgentExited` on failure.
    pub fn load(self: &Arc<Self>, env: BTreeMap<String, String>) -> Result<(), ErrorBody> {
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
        // Holding the task slot keeps a concurrent `close` from missing the
        // new task.
        let mut task = self.task.lock().unwrap();
        let resume = self.update(State::load)?;
        self.launch(&mut task, env, Launch::Resume(resume));
        Ok(())
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
        *self.archived.lock().unwrap()
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

    /// Marks the session archived, or not, and signals the change.
    pub fn set_archived(&self, archived: bool) {
        self.update(|_| *self.archived.lock().unwrap() = archived);
    }

    /// The directory the session's agent runs in.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn worktree(&self) -> Option<Arc<Worktree>> {
        self.worktree.lock().unwrap().clone()
    }

    /// The lifecycle guard of a session no one else has seen yet, such as one
    /// just created or restored.
    pub fn hold_lifecycle(&self) -> OwnedMutexGuard<()> {
        self.lifecycle
            .clone()
            .try_lock_owned()
            .expect("nothing else holds a new session's lifecycle lock")
    }

    /// Waits until the session is not starting in its worktree, loading, or
    /// being deleted, and keeps others from doing so while the guard lives.
    pub async fn lifecycle(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.lifecycle.lock().await
    }

    pub fn state(&self) -> SessionState {
        self.lock().state()
    }

    fn info_of(&self, state: &State) -> SessionInfo {
        let agent = state.agent();
        let worktree_id = self
            .worktree
            .lock()
            .unwrap()
            .as_ref()
            .map(|worktree| worktree.id.clone());
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
            config_options: agent
                .map(|agent| agent.config_options.clone())
                .unwrap_or_default(),
            available_commands: agent
                .map(|agent| agent.available_commands.clone())
                .unwrap_or_default(),
            worktree_id,
            archived: *self.archived.lock().unwrap(),
            created_at: self.created_at,
            unseen: *self.unseen.lock().unwrap(),
        }
    }

    /// Counts a connection's focus on the session while the guard lives, and
    /// clears its unseen outcome.
    pub fn focus(self: &Arc<Self>) -> SessionFocus {
        self.update(|_| {
            self.focused.fetch_add(1, Ordering::SeqCst);
            *self.unseen.lock().unwrap() = None;
        });
        SessionFocus(self.clone())
    }

    /// Clears the unseen outcome of a child session whose run was focused.
    pub fn clear_unseen(&self) {
        self.update(|_| *self.unseen.lock().unwrap() = None);
    }

    /// Whether a connection focuses the session or its run.
    fn focused(&self) -> bool {
        self.focused.load(Ordering::SeqCst) > 0
            || self
                .run_focused
                .as_ref()
                .is_some_and(|run| run.load(Ordering::SeqCst) > 0)
    }

    /// Records the unseen outcome `events` give a session out of focus. An
    /// error replaces a finish but not the reverse, and a run's child
    /// session takes no finish.
    fn note_unseen(&self, events: &[Event]) {
        if self.focused() {
            return;
        }
        let mut unseen = self.unseen.lock().unwrap();
        match state::unseen(events) {
            Some(Unseen::Error) => *unseen = Some(Unseen::Error),
            Some(Unseen::Finished) if unseen.is_none() && self.run_focused.is_none() => {
                *unseen = Some(Unseen::Finished);
            }
            _ => {}
        }
    }

    /// Counts a subscription to the session's events while the guard lives.
    pub fn view(self: &Arc<Self>) -> Viewer {
        self.update(|_| *self.viewers.lock().unwrap() += 1);
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

    /// Sends a prompt and returns the seq of the `PromptStarted` event that
    /// records it, which the turn's events follow.
    pub fn prompt(&self, prompt: Vec<ContentBlock>) -> Result<u64, ErrorBody> {
        self.update(|state| {
            state.request(Request::Prompt(prompt.clone()))?;
            let seq = state.last_seq();
            self.send(Command::Prompt { prompt, seq });
            Ok(seq)
        })
    }

    /// Asks the agent to change a config option. The request goes to the
    /// agent ahead of every command sent after this returns, such as a
    /// prompt. The returned future resolves to the agent's resulting options.
    pub fn set_config_option(
        &self,
        config_id: String,
        value: ConfigValue,
    ) -> Result<impl Future<Output = Result<Vec<SessionConfigOption>, ErrorBody>> + use<>, ErrorBody>
    {
        let state = self.lock().state();
        if matches!(state, SessionState::Starting | SessionState::Exited) {
            return Err(invalid_state(state));
        }
        let value = option_value(value);
        let (reply, result) = oneshot::channel();
        self.send(Command::SetConfigOption {
            config_id,
            value,
            reply,
        });
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
        })
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
        })
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
        let task = self.task.lock().unwrap().take();
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
        self.record(Change::Closed);
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
        }
        let parker = self.parker.lock().unwrap().take();
        if let Some(parker) = parker {
            parker.abort();
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Sends a command to the running agent task. A session whose agent has
    /// exited has none, and its commands are dropped.
    fn send(&self, command: Command) {
        let _ = self.commands.lock().unwrap().send(command);
    }

    /// Changes the state, publishes the seq of its last event, records the
    /// unseen outcome its new events give, queues them for the database,
    /// saves a changed title, agent session ID or unseen outcome, signals
    /// the daemon if the session's `SessionInfo` changed, and asks the
    /// worktree for its comparison again when a turn ended.
    fn update<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.lock();
        let before = self.info_of(&state);
        let result = change(&mut state);
        self.last_seq.send_replace(state.last_seq());
        let events = state.take_unlogged();
        self.note_unseen(&events);
        if events
            .iter()
            .any(|event| matches!(event.kind, EventKind::TurnEnded { .. }))
            && let Some(worktree) = self.worktree()
        {
            worktree.recompare();
        }
        if !events.is_empty() {
            self.store.log(events);
        }
        self.parkable
            .set(state.quiet() && *self.viewers.lock().unwrap() == 0);
        let after = self.info_of(&state);
        if after != before {
            self.save(&before, &after);
            self.info_version.fetch_add(1, Ordering::Release);
            self.changed.send_replace(());
        }
        result
    }

    /// Writes what differs between two infos to the session's row. A failed
    /// write goes to oxd's log and the session carries on.
    fn save(&self, before: &SessionInfo, after: &SessionInfo) {
        if after.title != before.title
            && let Err(error) = self.store.set_title(&self.id, after.title.as_deref())
        {
            eprintln!("oxd: saving the title of session {}: {error}", self.id);
        }
        if after.agent_session_id != before.agent_session_id
            && let Some(agent_session_id) = &after.agent_session_id
            && let Err(error) = self.store.set_agent_session_id(&self.id, agent_session_id)
        {
            eprintln!(
                "oxd: saving the agent session of session {}: {error}",
                self.id
            );
        }
        if after.unseen != before.unseen
            && let Err(error) = self.store.set_session_unseen(&self.id, after.unseen)
        {
            eprintln!(
                "oxd: saving the unseen outcome of session {}: {error}",
                self.id
            );
        }
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
                        session.record(Change::PermissionRequested { request, answer });
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
                    let step = match &launch {
                        Launch::New(opening) => opening.step.as_ref(),
                        Launch::Resume(_) => None,
                    };
                    let instructions = instructions::text(step, !mcp_servers.is_empty());
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
                    let options = match launch {
                        Launch::Resume(Resume {
                            session_id,
                            replay,
                            options,
                        }) => {
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
                            options
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
                            opening.options
                        }
                    };
                    let acp_session = open.await.map_err(|_| {
                        agent_client_protocol::Error::internal_error()
                            .data("the agent closed the connection before the session opened")
                    })??;
                    // The agent reports its own option values, and is set to
                    // the resumed session's, or those a new session was
                    // given, where they differ.
                    let reported = state::option_values(&session.info().config_options);
                    for (config_id, value) in options {
                        if !reported
                            .iter()
                            .any(|(id, current)| *id == config_id && *current != value)
                        {
                            continue;
                        }
                        let session = session.clone();
                        connection
                            .send_request(SetSessionConfigOptionRequest::new(
                                acp_session.clone(),
                                config_id,
                                value,
                            ))
                            .on_receiving_result(async move |result| {
                                match result {
                                    Ok(response) => session
                                        .record(Change::ConfigOptionsSet(response.config_options)),
                                    Err(error) => eprintln!(
                                        "oxd: setting a config option of session {}: {error}",
                                        session.id
                                    ),
                                }
                                Ok(())
                            })?;
                    }
                    loop {
                        let command = tokio::select! {
                            () = connection.incoming_closed() => break,
                            command = commands.recv() => command,
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
                                        // The turn runs and ends on the
                                        // agent's `state_update`s. An error
                                        // answer means the prompt was not
                                        // inserted.
                                        if let Err(error) = result {
                                            session.record(Change::PromptRejected {
                                                prompt: seq,
                                                error: error.to_string(),
                                            });
                                        }
                                        Ok(())
                                    })?;
                            }
                            Some(Command::Cancel) => {
                                connection.send_notification(CancelSessionNotification::new(
                                    acp_session.clone(),
                                ))?;
                                session.record(Change::Cancelled);
                            }
                            Some(Command::SetConfigOption {
                                config_id,
                                value,
                                reply,
                            }) => {
                                let session = session.clone();
                                connection
                                    .send_request(SetSessionConfigOptionRequest::new(
                                        acp_session.clone(),
                                        config_id,
                                        value,
                                    ))
                                    .on_receiving_result(async move |result| {
                                        let result = result
                                            .map(|response| response.config_options)
                                            .map_err(|error| error.to_string());
                                        if let Ok(options) = &result {
                                            session
                                                .record(Change::ConfigOptionsSet(options.clone()));
                                        }
                                        let _ = reply.send(result);
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

/// The ACP value of an option value from a client.
pub fn option_value(value: ConfigValue) -> SessionConfigOptionValue {
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
