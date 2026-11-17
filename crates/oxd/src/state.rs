//! A session's state machine and the event history it writes.

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v2::schema::StopReason;
use agent_client_protocol::v2::schema::{
    AvailableCommand, BackgroundWork, CancelledPermissionOutcome, ConfigOptionUpdate, ContentBlock,
    CreateElicitationRequest, ElicitationAction, ElicitationMode, ElicitationPropertySchema,
    ElicitationSchema, ElicitationScope, EmbeddedResourceResource, PermissionOption,
    PromptCapabilities, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionValue,
    SessionUpdate, StateUpdate, TextContent, ToolCallId,
};
use ox_protocol::{
    ErrorBody, Event, EventKind, QueuedPrompt, Sender, SessionRequest, SessionState, Unseen, code,
};
use tokio::sync::oneshot;

/// Events a subscriber is handed at a time.
const BATCH: usize = 256;

/// What an agent reports when its ACP session opens.
pub struct Agent {
    pub session_id: String,
    pub prompt_capabilities: PromptCapabilities,
    pub config_options: Vec<SessionConfigOption>,
    pub available_commands: Vec<AvailableCommand>,
}

impl Agent {
    /// The agent session of a session whose agent is not running: its ID, and
    /// nothing the agent reported.
    fn saved(session_id: String) -> Self {
        Self {
            session_id,
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }
    }
}

/// What a client asks of a session. The session refuses a request its state
/// does not allow.
pub enum Request {
    /// A prompt to send now: it starts a turn on an idle session and steers
    /// a running one.
    Prompt(Vec<ContentBlock>),
    /// A peer message from another session, sent now as a prompt is, but
    /// leaving a held queue held. The agent gets `text` after a header that
    /// names the sender.
    Message {
        text: String,
        from: Sender,
    },
    QueueAdd(QueuedPrompt),
    /// Replaces the queued prompt with the same ID.
    QueueEdit(QueuedPrompt),
    QueueRemove {
        id: String,
    },
    /// Takes a queued prompt out of the queue and sends it now.
    QueueSend {
        id: String,
    },
    Permit {
        request_id: String,
        option_id: String,
    },
    /// An answer to an elicitation.
    Respond {
        request_id: String,
        action: ElicitationAction,
    },
}

/// Something that happened to a session. A change that does not fit the
/// session's state, such as an agent's answer that arrives after the session
/// closed, is ignored.
#[allow(
    clippy::large_enum_variant,
    reason = "a change is built and consumed at once, never stored"
)]
pub enum Change {
    Started(Agent),
    Update(SessionUpdate),
    /// The options an agent answered `session/set_config_option` with. The
    /// connection's dispatch loop waits for each handler to finish before it
    /// reads the next message (the SDK's `concepts::ordering` chapter), so a
    /// `config_option_update` the agent sent before its response is already
    /// recorded, and options that differ from the session's have not been
    /// reported.
    ConfigOptionsSet(Vec<SessionConfigOption>),
    /// A permission request, the agent's or, with `session_request`, the
    /// daemon's own for `session.request`.
    PermissionRequested {
        request: RequestPermissionRequest,
        session_request: Option<SessionRequest>,
        answer: oneshot::Sender<RequestPermissionOutcome>,
    },
    /// A form elicitation that `check_form` accepted.
    ElicitationRequested {
        message: String,
        tool_call_id: Option<ToolCallId>,
        schema: ElicitationSchema,
        answer: oneshot::Sender<ElicitationAction>,
    },
    /// The agent withdrew an elicitation with `$/cancel_request`. Its handler
    /// closed the answer's receiver, so every pending elicitation whose
    /// receiver is closed is the one withdrawn.
    ElicitationWithdrawn,
    /// The agent was told to cancel the turn. Pending permission requests
    /// are cancelled; elicitations stay open until the agent withdraws them.
    Cancelled,
    /// The agent answered `session/prompt` successfully: it inserted the
    /// prompt that the `PromptStarted` or `PromptSteered` event at seq
    /// `prompt` records. The turn runs and ends on the agent's
    /// `state_update`s, so only a steered prompt's acceptance is recorded.
    PromptAccepted {
        prompt: u64,
    },
    /// The agent answered `session/prompt` with an error: it did not insert
    /// the prompt that the `PromptStarted` or `PromptSteered` event at seq
    /// `prompt` records.
    PromptRejected {
        prompt: u64,
        error: String,
    },
    Exited(Option<String>),
    /// The daemon stopped the agent of a session that was idle, with no
    /// background work, and unviewed, or that it archived.
    Parked,
    Closed,
}

/// A request the agent is waiting on a client to answer.
struct Pending {
    request_id: String,
    kind: PendingKind,
}

enum PendingKind {
    Permission {
        options: Vec<PermissionOption>,
        answer: oneshot::Sender<RequestPermissionOutcome>,
    },
    Elicitation {
        answer: oneshot::Sender<ElicitationAction>,
    },
}

impl Pending {
    fn is_permission(&self) -> bool {
        matches!(self.kind, PendingKind::Permission { .. })
    }

    /// Answers the agent that the request is cancelled and returns the
    /// event that records it.
    fn cancel(self) -> EventKind {
        match self.kind {
            PendingKind::Permission { answer, .. } => resolve_permission(
                self.request_id,
                answer,
                CancelledPermissionOutcome::new().into(),
            ),
            PendingKind::Elicitation { answer, .. } => {
                resolve_elicitation(self.request_id, answer, ElicitationAction::Cancel)
            }
        }
    }
}

/// Answers a permission request and returns the event that records it.
fn resolve_permission(
    request_id: String,
    answer: oneshot::Sender<RequestPermissionOutcome>,
    outcome: RequestPermissionOutcome,
) -> EventKind {
    let _ = answer.send(outcome.clone());
    EventKind::PermissionResolved {
        request_id,
        outcome,
    }
}

/// Answers an elicitation and returns the event that records it.
fn resolve_elicitation(
    request_id: String,
    answer: oneshot::Sender<ElicitationAction>,
    action: ElicitationAction,
) -> EventKind {
    let _ = answer.send(action.clone());
    EventKind::ElicitationResolved { request_id, action }
}

/// The running turn. A turn starts when a prompt is sent or when the agent
/// reports work, and ends when the agent reports `idle` after reporting work.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Turn {
    /// The prompt whose `PromptStarted` event has this seq was sent, and
    /// the agent has not reported work for it. An `idle` here repeats a
    /// snapshot from before the prompt.
    Prompted(u64),
    /// The agent reported work. `recorded` is false for work a resumed agent
    /// was already doing, whose start the session's history does not have.
    Working { recorded: bool },
}

/// An agent with an open ACP session.
struct Live {
    agent: Agent,
    turn: Option<Turn>,
    pending: Vec<Pending>,
    /// The seqs of the `PromptSteered` events whose prompts the agent has
    /// not answered. A steered prompt still pending when the turn ends
    /// starts a turn of the agent's own.
    steered: Vec<u64>,
    /// The background work the agent last reported. `None` when it does not
    /// report any.
    background: Option<Vec<BackgroundWork>>,
}

impl Live {
    /// No turn, no pending request or steered prompt, and the agent reports
    /// no background work.
    fn quiet(&self) -> bool {
        self.turn.is_none()
            && self.pending.is_empty()
            && self.steered.is_empty()
            && self.background.as_deref() == Some(&[])
    }

    fn state(&self) -> SessionState {
        if self.pending.iter().any(Pending::is_permission) {
            SessionState::AwaitingPermission
        } else if !self.pending.is_empty() {
            SessionState::AwaitingInput
        } else if self.turn.is_some() {
            SessionState::Running
        } else {
            SessionState::Idle
        }
    }

    /// Applies a `state_update` and returns the events of the turn it starts
    /// or ends. Any state but `idle`, including one this daemon does not
    /// know, is work in progress.
    fn follow(&mut self, state: &StateUpdate) -> Vec<EventKind> {
        match (state, self.turn) {
            (StateUpdate::Idle(idle), Some(Turn::Working { recorded })) => {
                self.turn = None;
                if recorded {
                    vec![EventKind::TurnEnded {
                        stop_reason: idle.stop_reason.clone(),
                        error: None,
                    }]
                } else {
                    Vec::new()
                }
            }
            (StateUpdate::Idle(_), _) | (_, Some(Turn::Working { .. })) => Vec::new(),
            (_, None) => {
                self.turn = Some(Turn::Working { recorded: true });
                vec![EventKind::TurnStarted]
            }
            (_, Some(Turn::Prompted(_))) => {
                self.turn = Some(Turn::Working { recorded: true });
                Vec::new()
            }
        }
    }

    /// Cancels the pending requests that `which` selects.
    fn cancel_pending(&mut self, which: impl Fn(&Pending) -> bool) -> Vec<EventKind> {
        self.pending
            .extract_if(.., |pending| which(pending))
            .map(Pending::cancel)
            .collect()
    }

    /// Ends what the agent left open: every pending request is cancelled,
    /// every steered prompt it had not taken in is dropped, and a running
    /// turn ends with `error`.
    fn end(mut self, error: &str) -> (Agent, Vec<EventKind>) {
        let mut events = self.cancel_pending(|_| true);
        events.extend(
            self.steered
                .drain(..)
                .map(|prompt| EventKind::SteeredPromptDropped { prompt }),
        );
        if let Some(Turn::Prompted(_) | Turn::Working { recorded: true }) = self.turn {
            events.push(EventKind::TurnEnded {
                stop_reason: None,
                error: Some(error.into()),
            });
        }
        (self.agent, events)
    }
}

enum Phase {
    /// The agent is starting. `replay` buffers updates until the agent starts
    /// when recovering missing history. On resume, `busy` and `background`
    /// track the state the agent reports before it starts. `previous` holds
    /// the saved agent session ID. `prompt` is the seq of the `PromptStarted`
    /// event of a new session's first prompt, which is sent once the agent
    /// starts.
    Starting {
        replay: Option<Vec<SessionUpdate>>,
        busy: bool,
        background: Option<Vec<BackgroundWork>>,
        previous: Option<Agent>,
        prompt: Option<u64>,
    },
    Live(Live),
    /// The agent is gone. It holds what the agent reported if it started,
    /// or the saved agent session ID of a session restored from the
    /// database.
    Exited(Option<Agent>),
    Closed,
}

impl Phase {
    fn state(&self) -> SessionState {
        match self {
            Self::Starting { .. } => SessionState::Starting,
            Self::Live(live) => live.state(),
            Self::Exited(_) | Self::Closed => SessionState::Exited,
        }
    }

    /// The phase after `change` and the events that record it.
    /// Permission requests take their IDs from `next_request`, which the
    /// session keeps across agents.
    fn after(self, change: Change, next_request: &mut u64) -> (Self, Vec<EventKind>) {
        match (self, change) {
            (
                Self::Starting {
                    replay,
                    busy,
                    background,
                    prompt,
                    ..
                },
                Change::Started(agent),
            ) => {
                let mut events: Vec<_> = replay
                    .into_iter()
                    .flatten()
                    .map(EventKind::Update)
                    .collect();
                events.push(EventKind::AgentStarted {
                    agent_session_id: agent.session_id.clone(),
                    prompt_capabilities: agent.prompt_capabilities.clone(),
                    config_options: agent.config_options.clone(),
                    available_commands: agent.available_commands.clone(),
                });
                // A resumed agent that is working has a turn the session did
                // not see start, and a new session's first prompt is sent
                // as it starts.
                let turn = match prompt {
                    Some(seq) => Some(Turn::Prompted(seq)),
                    None => busy.then_some(Turn::Working { recorded: false }),
                };
                let live = Live {
                    agent,
                    turn,
                    pending: Vec::new(),
                    steered: Vec::new(),
                    background,
                };
                (Self::Live(live), events)
            }
            (
                Self::Starting {
                    mut replay,
                    mut busy,
                    mut background,
                    previous,
                    prompt,
                },
                Change::Update(update),
            ) => {
                if let SessionUpdate::StateUpdate(state) = &update {
                    busy = !matches!(state, StateUpdate::Idle(_));
                    background = state.background().map(<[_]>::to_vec);
                }
                if let Some(replay) = &mut replay {
                    replay.push(update);
                }
                (
                    Self::Starting {
                        replay,
                        busy,
                        background,
                        previous,
                        prompt,
                    },
                    Vec::new(),
                )
            }
            (
                Self::Starting {
                    previous, prompt, ..
                },
                Change::Exited(error),
            ) => {
                let mut events = Vec::new();
                if prompt.is_some() {
                    events.push(EventKind::TurnEnded {
                        stop_reason: None,
                        error: Some(
                            error
                                .clone()
                                .unwrap_or_else(|| "the agent exited before the prompt".into()),
                        ),
                    });
                }
                events.push(EventKind::AgentExited { error });
                (Self::Exited(previous), events)
            }
            (Self::Starting { .. } | Self::Exited(_), Change::Closed) => {
                (Self::Closed, vec![EventKind::SessionClosed])
            }

            (Self::Live(mut live), Change::Update(update)) => {
                let mut turn = Vec::new();
                match &update {
                    SessionUpdate::ConfigOptionUpdate(options) => {
                        live.agent
                            .config_options
                            .clone_from(&options.config_options);
                    }
                    SessionUpdate::AvailableCommandsUpdate(commands) => {
                        live.agent
                            .available_commands
                            .clone_from(&commands.available_commands);
                    }
                    SessionUpdate::StateUpdate(state) => {
                        live.background = state.background().map(<[_]>::to_vec);
                        turn = live.follow(state);
                    }
                    _ => {}
                }
                let mut events = vec![EventKind::Update(update)];
                events.extend(turn);
                (Self::Live(live), events)
            }
            (Self::Live(mut live), Change::ConfigOptionsSet(options)) => {
                let mut events = Vec::new();
                if live.agent.config_options != options {
                    live.agent.config_options.clone_from(&options);
                    let update = ConfigOptionUpdate::new(options);
                    events.push(EventKind::Update(SessionUpdate::ConfigOptionUpdate(update)));
                }
                (Self::Live(live), events)
            }
            (
                Self::Live(mut live),
                Change::PermissionRequested {
                    request,
                    session_request,
                    answer,
                },
            ) => {
                let request_id = next_request.to_string();
                *next_request += 1;
                live.pending.push(Pending {
                    request_id: request_id.clone(),
                    kind: PendingKind::Permission {
                        options: request.options.clone(),
                        answer,
                    },
                });
                let requested = EventKind::PermissionRequested {
                    request_id,
                    title: request.title,
                    description: request.description,
                    subject: request.subject,
                    options: request.options,
                    session_request,
                };
                (Self::Live(live), vec![requested])
            }
            (
                Self::Live(mut live),
                Change::ElicitationRequested {
                    message,
                    tool_call_id,
                    schema,
                    answer,
                },
            ) => {
                let request_id = next_request.to_string();
                *next_request += 1;
                live.pending.push(Pending {
                    request_id: request_id.clone(),
                    kind: PendingKind::Elicitation { answer },
                });
                let requested = EventKind::ElicitationRequested {
                    request_id,
                    message,
                    tool_call_id,
                    requested_schema: schema,
                };
                (Self::Live(live), vec![requested])
            }
            (Self::Live(mut live), Change::ElicitationWithdrawn) => {
                let events = live.cancel_pending(|pending| {
                    matches!(&pending.kind, PendingKind::Elicitation { answer, .. } if answer.is_closed())
                });
                (Self::Live(live), events)
            }
            (Self::Live(mut live), Change::Cancelled) => {
                let events = live.cancel_pending(Pending::is_permission);
                (Self::Live(live), events)
            }
            (Self::Live(mut live), Change::PromptAccepted { prompt })
                if live.steered.contains(&prompt) =>
            {
                live.steered.retain(|steered| *steered != prompt);
                (
                    Self::Live(live),
                    vec![EventKind::SteeredPromptTaken { prompt }],
                )
            }
            (Self::Live(mut live), Change::PromptRejected { prompt, .. })
                if live.steered.contains(&prompt) =>
            {
                live.steered.retain(|steered| *steered != prompt);
                (
                    Self::Live(live),
                    vec![EventKind::SteeredPromptDropped { prompt }],
                )
            }
            // The agent rejected the prompt, so no turn began. After the
            // agent reported work for it, the turn ends at the agent's `idle`.
            (Self::Live(mut live), Change::PromptRejected { prompt, error })
                if live.turn == Some(Turn::Prompted(prompt)) =>
            {
                live.turn = None;
                let ended = EventKind::TurnEnded {
                    stop_reason: None,
                    error: Some(error),
                };
                (Self::Live(live), vec![ended])
            }
            (Self::Live(live), Change::Exited(error)) => {
                let (agent, mut events) = live.end("the agent exited");
                events.push(EventKind::AgentExited { error });
                (Self::Exited(Some(agent)), events)
            }
            // The daemon parks a quiet session, and archives any session. One
            // that stopped being quiet before the agent was stopped, or that
            // is archived while it works, ends what the agent left open.
            (Self::Live(live), Change::Parked) => {
                let (agent, mut events) = live.end("the agent was stopped");
                events.push(EventKind::AgentParked);
                (Self::Exited(Some(agent)), events)
            }
            (Self::Live(live), Change::Closed) => {
                let (_, mut events) = live.end("the session was closed");
                events.push(EventKind::SessionClosed);
                (Self::Closed, events)
            }

            (
                phase @ Self::Starting { .. },
                Change::ConfigOptionsSet(_)
                | Change::PermissionRequested { .. }
                | Change::ElicitationRequested { .. }
                | Change::ElicitationWithdrawn
                | Change::Cancelled
                | Change::PromptAccepted { .. }
                | Change::PromptRejected { .. },
            )
            | (
                phase @ Self::Live(_),
                Change::Started(_) | Change::PromptAccepted { .. } | Change::PromptRejected { .. },
            )
            | (
                phase @ Self::Exited(_),
                Change::Started(_)
                | Change::Update(_)
                | Change::ConfigOptionsSet(_)
                | Change::PermissionRequested { .. }
                | Change::ElicitationRequested { .. }
                | Change::ElicitationWithdrawn
                | Change::Cancelled
                | Change::PromptAccepted { .. }
                | Change::PromptRejected { .. }
                | Change::Exited(_)
                | Change::Parked,
            )
            | (phase @ Self::Starting { .. }, Change::Parked)
            | (phase @ Self::Closed, _) => (phase, Vec::new()),
        }
    }
}

/// A session's queue: the prompts waiting to start a turn each, in order.
/// `paused` holds them after a cancelled turn or one that did not end
/// normally, until a prompt is sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    pub prompts: Vec<QueuedPrompt>,
    pub paused: bool,
}

/// A session's phase, its queue, and every event it has recorded.
/// `SessionClosed` ends a session's subscriptions and is not stored. Each
/// `PromptStarted` and `TurnStarted` is followed by one `TurnEnded`, each
/// `PromptSteered` by one `SteeredPromptTaken` or `SteeredPromptDropped`,
/// each `PermissionRequested` by one `PermissionResolved`, and each
/// `ElicitationRequested` by one `ElicitationResolved`.
///
/// A session restored from the database knows its last seq but holds no
/// events until `fill` gives it the stored history, and records none before.
pub struct State {
    session_id: String,
    phase: Phase,
    /// The title from the last recorded `session_info_update` that set or
    /// cleared it, or from `set_title`, whichever came last.
    title: Option<String>,
    events: Option<Vec<Event>>,
    last_seq: u64,
    /// Events recorded since `take_unlogged`, which the database has not been
    /// sent. `SessionClosed` is not stored.
    unlogged: Vec<Event>,
    /// The ID of the next permission request or elicitation. Request IDs
    /// stay unique for the session's lifetime, across every agent it loads.
    next_request: u64,
    queue: Queue,
    /// Whether the queue changed since `take_changed_queue`.
    queue_changed: bool,
}

#[derive(Debug)]
pub struct Resume {
    pub session_id: String,
    pub replay: bool,
    /// The value of each option the agent last reported, which the resumed
    /// agent is set to again.
    pub options: Vec<(String, SessionConfigOptionValue)>,
}

/// The value of each select and boolean option in `options`, by option ID.
pub fn option_values(options: &[SessionConfigOption]) -> Vec<(String, SessionConfigOptionValue)> {
    options
        .iter()
        .filter_map(|option| {
            let value = match &option.kind {
                SessionConfigKind::Select(select) => {
                    SessionConfigOptionValue::id(select.current_value.clone())
                }
                SessionConfigKind::Boolean(boolean) => {
                    SessionConfigOptionValue::boolean(boolean.current_value)
                }
                _ => return None,
            };
            Some((option.config_id.to_string(), value))
        })
        .collect()
}

impl State {
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            phase: Phase::Starting {
                replay: Some(Vec::new()),
                busy: false,
                background: None,
                previous: None,
                prompt: None,
            },
            title: None,
            events: Some(Vec::new()),
            last_seq: 0,
            unlogged: Vec::new(),
            next_request: 1,
            queue: Queue::default(),
            queue_changed: false,
        }
    }

    /// A session saved in the database: exited, with no agent and its
    /// history, which ends at `last_seq`, not yet read, and its saved queue.
    pub fn restored(
        session_id: String,
        title: Option<String>,
        agent_session_id: String,
        last_seq: u64,
        queue: Queue,
    ) -> Self {
        Self {
            session_id,
            phase: Phase::Exited(Some(Agent::saved(agent_session_id))),
            title,
            events: None,
            last_seq,
            unlogged: Vec::new(),
            next_request: 1,
            queue,
            queue_changed: false,
        }
    }

    /// A saved session whose agent never started: exited, with no events and
    /// nothing to load, and its saved queue.
    pub fn unstarted(session_id: String, title: Option<String>, queue: Queue) -> Self {
        Self {
            session_id,
            phase: Phase::Exited(None),
            title,
            events: Some(Vec::new()),
            last_seq: 0,
            unlogged: Vec::new(),
            next_request: 1,
            queue,
            queue_changed: false,
        }
    }

    /// Whether the session holds its history.
    pub fn loaded(&self) -> bool {
        self.events.is_some()
    }

    /// Gives a restored session its stored history. What a crash left open is
    /// closed: a pending permission request or elicitation is cancelled, a
    /// steered prompt the agent had not taken in is dropped, and a running
    /// turn ends with an error, which holds the queue. Request IDs continue
    /// after the history's.
    pub fn fill(&mut self, history: Vec<Event>) {
        let mut turn_open = false;
        let mut steered = Vec::new();
        // Each open request's ID, and whether it is a permission request.
        let mut pending: Vec<(&str, bool)> = Vec::new();
        let mut last_request = 0;
        for event in &history {
            match &event.kind {
                EventKind::PromptStarted { .. } | EventKind::TurnStarted => turn_open = true,
                EventKind::TurnEnded { .. } => turn_open = false,
                EventKind::PromptSteered { .. } => steered.push(event.seq),
                EventKind::SteeredPromptTaken { prompt }
                | EventKind::SteeredPromptDropped { prompt } => {
                    steered.retain(|seq| seq != prompt);
                }
                EventKind::PermissionRequested { request_id, .. }
                | EventKind::ElicitationRequested { request_id, .. } => {
                    let permission = matches!(event.kind, EventKind::PermissionRequested { .. });
                    pending.push((request_id, permission));
                    last_request = last_request.max(request_id.parse().unwrap_or(0));
                }
                EventKind::PermissionResolved { request_id, .. }
                | EventKind::ElicitationResolved { request_id, .. } => {
                    pending.retain(|(id, _)| id != request_id);
                }
                _ => {}
            }
        }
        let cancelled: Vec<_> = pending
            .into_iter()
            .map(|(request_id, permission)| {
                let request_id = request_id.into();
                if permission {
                    EventKind::PermissionResolved {
                        request_id,
                        outcome: CancelledPermissionOutcome::new().into(),
                    }
                } else {
                    EventKind::ElicitationResolved {
                        request_id,
                        action: ElicitationAction::Cancel,
                    }
                }
            })
            .collect();
        self.next_request = last_request + 1;
        self.events = Some(history);
        for event in cancelled {
            self.append(event);
        }
        for prompt in steered {
            self.append(EventKind::SteeredPromptDropped { prompt });
        }
        // TODO: a turn a crash cut off gets its unseen error only here, when
        // the session's history is first read, not when oxd restarts.
        if turn_open {
            self.append(EventKind::TurnEnded {
                stop_reason: None,
                error: Some("oxd stopped during the turn".into()),
            });
            self.pause();
        }
    }

    /// The events recorded since the last call, in order, for the database.
    pub fn take_unlogged(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.unlogged)
    }

    /// The queue, if it changed since the last call, for the database.
    pub fn take_changed_queue(&mut self) -> Option<Queue> {
        std::mem::take(&mut self.queue_changed).then(|| self.queue.clone())
    }

    /// The prompts waiting to start a turn each, in order.
    pub fn queued(&self) -> &[QueuedPrompt] {
        &self.queue.prompts
    }

    fn pause(&mut self) {
        if !self.queue.paused {
            self.queue.paused = true;
            self.queue_changed = true;
        }
    }

    fn resume(&mut self) {
        if self.queue.paused {
            self.queue.paused = false;
            self.queue_changed = true;
        }
    }

    /// Starts a turn with the queue's first prompt when the agent is idle,
    /// has no steered prompt to take in, and the queue is not held. Returns
    /// the prompt to send and the seq of its `PromptStarted` event.
    // TODO: a queued prompt can start while the session's branch is moving,
    // which `session.prompt` refuses.
    pub fn start_queued(&mut self) -> Option<(Vec<ContentBlock>, u64)> {
        let Phase::Live(live) = &mut self.phase else {
            return None;
        };
        if live.turn.is_some()
            || !live.pending.is_empty()
            || !live.steered.is_empty()
            || self.queue.paused
            || self.queue.prompts.is_empty()
        {
            return None;
        }
        let seq = self.last_seq + 1;
        live.turn = Some(Turn::Prompted(seq));
        let prompt = self.queue.prompts.remove(0).prompt;
        self.queue_changed = true;
        self.append(EventKind::PromptStarted {
            prompt: prompt.clone(),
            from: None,
        });
        Some((prompt, seq))
    }

    /// Starts a load: the session goes from `exited` to `starting` and
    /// returns what to request from the agent. A session whose history holds
    /// neither an agent start nor a prompt requests and records replay to
    /// recover its conversation.
    pub fn load(&mut self) -> Result<Resume, ErrorBody> {
        let id = match &self.phase {
            Phase::Exited(Some(agent)) => agent.session_id.clone(),
            Phase::Exited(None) => {
                return Err(ErrorBody {
                    code: code::INVALID_STATE,
                    message: "the session never started, so it has nothing to load".into(),
                });
            }
            phase => return Err(invalid_state(phase.state())),
        };
        let events = self.events.iter().flatten();
        let replay = !events.clone().any(|event| {
            matches!(
                event.kind,
                EventKind::AgentStarted { .. } | EventKind::PromptStarted { .. }
            )
        });
        let options = events
            .rev()
            .find_map(|event| match &event.kind {
                EventKind::AgentStarted { config_options, .. } => Some(config_options),
                EventKind::Update(SessionUpdate::ConfigOptionUpdate(update)) => {
                    Some(&update.config_options)
                }
                _ => None,
            })
            .map(|options| option_values(options))
            .unwrap_or_default();
        self.phase = Phase::Starting {
            busy: false,
            background: None,
            replay: replay.then(Vec::new),
            previous: Some(Agent::saved(id.clone())),
            prompt: None,
        };
        Ok(Resume {
            session_id: id,
            replay,
            options,
        })
    }

    /// Whether the agent is running with nothing to do: no turn, no pending
    /// request, and no background work.
    pub fn quiet(&self) -> bool {
        matches!(&self.phase, Phase::Live(live) if live.quiet())
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Replaces the title, recording no event.
    pub fn set_title(&mut self, title: String) {
        self.title = Some(title);
    }

    pub fn state(&self) -> SessionState {
        self.phase.state()
    }

    /// What the agent reported when it started, with its current config
    /// options.
    pub fn agent(&self) -> Option<&Agent> {
        match &self.phase {
            Phase::Live(live) => Some(&live.agent),
            Phase::Exited(agent)
            | Phase::Starting {
                previous: agent, ..
            } => agent.as_ref(),
            Phase::Closed => None,
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Every event the session holds, in order. A session that has not
    /// loaded its history holds none.
    pub fn events(&self) -> &[Event] {
        self.events.as_deref().unwrap_or_default()
    }

    /// Whether a turn is running, which a prompt sent now steers.
    pub fn in_turn(&self) -> bool {
        matches!(&self.phase, Phase::Live(live) if live.turn.is_some())
    }

    /// Checks that a subscriber that has every event through `after_seq`
    /// has more to come, the last of them `SessionClosed`.
    pub fn subscribe(&self, after_seq: u64) -> Result<(), ErrorBody> {
        let last_seq = self.last_seq();
        if after_seq > last_seq {
            return Err(invalid_request(format!(
                "after_seq {after_seq} is past the session's last event, {last_seq}"
            )));
        }
        if after_seq == last_seq && matches!(self.phase, Phase::Closed) {
            return Err(invalid_request(format!(
                "the session is closed and has no event after {after_seq}"
            )));
        }
        Ok(())
    }

    /// The first events with `seq > after_seq`, in order. A session that has
    /// not loaded its history has none to give.
    pub fn events_after(&self, after_seq: u64) -> Vec<Event> {
        let events = self.events.as_deref().unwrap_or_default();
        let start = usize::try_from(after_seq)
            .unwrap_or(usize::MAX)
            .min(events.len());
        events[start..].iter().take(BATCH).cloned().collect()
    }

    /// Records a new session's first prompt while its agent starts and
    /// returns the seq of its `PromptStarted` event. The prompt's turn runs
    /// once the agent starts.
    pub fn record_first_prompt(&mut self, prompt: Vec<ContentBlock>) -> u64 {
        let seq = self.last_seq() + 1;
        let Phase::Starting { prompt: first, .. } = &mut self.phase else {
            unreachable!("a first prompt is recorded while the agent starts");
        };
        *first = Some(seq);
        self.append(EventKind::PromptStarted { prompt, from: None });
        seq
    }

    /// Applies a client's request. A prompt it sends now is returned as the
    /// agent is to get it, with the seq of the `PromptStarted` or
    /// `PromptSteered` event that records it.
    pub fn request(
        &mut self,
        request: Request,
    ) -> Result<Option<(Vec<ContentBlock>, u64)>, ErrorBody> {
        let event = match (&mut self.phase, request) {
            (_, Request::Prompt(prompt)) => return self.send_now(prompt, None, true).map(Some),
            (_, Request::Message { text, from }) => {
                let header = message_header(&from);
                let prompt = vec![ContentBlock::Text(TextContent::new(text.clone()))];
                let (_, seq) = self.send_now(prompt, Some(from), false)?;
                let agent_prompt = format!("{header}\n\n{text}");
                return Ok(Some((
                    vec![ContentBlock::Text(TextContent::new(agent_prompt))],
                    seq,
                )));
            }
            (Phase::Closed, Request::QueueAdd(_) | Request::QueueEdit(_)) => {
                return Err(invalid_state(SessionState::Exited));
            }
            (_, Request::QueueAdd(queued)) => {
                self.check_queued(&queued.prompt)?;
                self.queue.prompts.push(queued);
                self.queue_changed = true;
                return Ok(None);
            }
            (_, Request::QueueEdit(queued)) => {
                self.check_queued(&queued.prompt)?;
                let index = self.queued_index(&queued.id)?;
                self.queue.prompts[index] = queued;
                self.queue_changed = true;
                return Ok(None);
            }
            (_, Request::QueueRemove { id }) => {
                let index = self.queued_index(&id)?;
                self.queue.prompts.remove(index);
                self.queue_changed = true;
                return Ok(None);
            }
            (_, Request::QueueSend { id }) => {
                let index = self.queued_index(&id)?;
                let sent = self.send_now(self.queue.prompts[index].prompt.clone(), None, true)?;
                self.queue.prompts.remove(index);
                self.queue_changed = true;
                return Ok(Some(sent));
            }
            (
                Phase::Live(live),
                Request::Permit {
                    request_id,
                    option_id,
                },
            ) => {
                let Some(index) = live.pending.iter().position(|pending| {
                    pending.request_id == request_id && pending.is_permission()
                }) else {
                    return Err(not_pending(&request_id));
                };
                let PendingKind::Permission { options, .. } = &live.pending[index].kind else {
                    unreachable!("the request found is a permission request");
                };
                if !options
                    .iter()
                    .any(|option| *option.option_id.0 == *option_id)
                {
                    return Err(invalid_request(format!(
                        "{option_id} is not an option of request {request_id}"
                    )));
                }
                let PendingKind::Permission { answer, .. } = live.pending.remove(index).kind else {
                    unreachable!("the request found is a permission request");
                };
                resolve_permission(
                    request_id,
                    answer,
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(option_id)),
                )
            }
            (Phase::Live(live), Request::Respond { request_id, action }) => {
                // An elicitation the agent is withdrawing has a closed
                // receiver, and its handler records its end.
                let Some(index) = live.pending.iter().position(|pending| {
                    pending.request_id == request_id
                        && matches!(&pending.kind, PendingKind::Elicitation { answer, .. } if !answer.is_closed())
                }) else {
                    return Err(not_pending(&request_id));
                };
                if !matches!(
                    action,
                    ElicitationAction::Accept(_)
                        | ElicitationAction::Decline
                        | ElicitationAction::Cancel
                ) {
                    return Err(invalid_request("unknown elicitation action".into()));
                }
                let PendingKind::Elicitation { answer, .. } = live.pending.remove(index).kind
                else {
                    unreachable!("the request found is an elicitation");
                };
                resolve_elicitation(request_id, answer, action)
            }
            (
                Phase::Starting { .. } | Phase::Exited(_) | Phase::Closed,
                Request::Permit { request_id, .. } | Request::Respond { request_id, .. },
            ) => return Err(not_pending(&request_id)),
        };
        self.append(event);
        Ok(None)
    }

    /// Sends a prompt now, recorded with its sender `from`: on an idle
    /// session it starts a turn, and during a turn it steers it. Either
    /// resumes a held queue when `resume_queue` is set. Returns the prompt
    /// and the seq of the event that records it.
    fn send_now(
        &mut self,
        prompt: Vec<ContentBlock>,
        from: Option<Sender>,
        resume_queue: bool,
    ) -> Result<(Vec<ContentBlock>, u64), ErrorBody> {
        let seq = self.last_seq + 1;
        let Phase::Live(live) = &mut self.phase else {
            return Err(invalid_state(self.phase.state()));
        };
        check_prompt(&prompt, &live.agent.prompt_capabilities)?;
        let event = if live.turn.is_some() {
            live.steered.push(seq);
            EventKind::PromptSteered {
                prompt: prompt.clone(),
                from,
            }
        } else if live.state() == SessionState::Idle {
            live.turn = Some(Turn::Prompted(seq));
            EventKind::PromptStarted {
                prompt: prompt.clone(),
                from,
            }
        } else {
            return Err(invalid_state(live.state()));
        };
        if resume_queue {
            self.resume();
        }
        self.append(event);
        Ok((prompt, seq))
    }

    /// Checks a prompt for the queue, against the prompt capabilities of the
    /// agent when it is running.
    fn check_queued(&self, prompt: &[ContentBlock]) -> Result<(), ErrorBody> {
        match &self.phase {
            Phase::Live(live) => check_prompt(prompt, &live.agent.prompt_capabilities),
            _ if prompt.is_empty() => Err(invalid_request("the prompt is empty".into())),
            _ => Ok(()),
        }
    }

    fn queued_index(&self, id: &str) -> Result<usize, ErrorBody> {
        self.queue
            .prompts
            .iter()
            .position(|queued| queued.id == id)
            .ok_or_else(|| ErrorBody {
                code: code::NOT_FOUND,
                message: format!("prompt {id} is not queued"),
            })
    }

    /// Records a change. A cancelled turn, and a turn that ends other than
    /// normally, holds the queue.
    pub fn record(&mut self, change: Change) {
        let cancelled = matches!(change, Change::Cancelled);
        let phase = std::mem::replace(&mut self.phase, Phase::Closed);
        let (phase, events) = phase.after(change, &mut self.next_request);
        self.phase = phase;
        if cancelled || events.iter().any(ends_abnormally) {
            self.pause();
        }
        for event in events {
            self.append(event);
        }
    }

    fn append(&mut self, kind: EventKind) {
        if let EventKind::Update(SessionUpdate::SessionInfoUpdate(update)) = &kind {
            match &update.title {
                MaybeUndefined::Value(title) => self.title = Some(title.clone()),
                MaybeUndefined::Null => self.title = None,
                MaybeUndefined::Undefined => {}
            }
        }
        // A session that has not loaded its history records nothing.
        let Some(events) = &mut self.events else {
            return;
        };
        self.last_seq += 1;
        let event = Event {
            session_id: self.session_id.clone(),
            seq: self.last_seq,
            recorded_at: crate::daemon::now(),
            kind,
        };
        if !matches!(event.kind, EventKind::SessionClosed) {
            self.unlogged.push(event.clone());
        }
        events.push(event);
    }
}

/// The line before a peer message's text that tells the agent who sent it
/// and how to answer.
fn message_header(from: &Sender) -> String {
    let title = from
        .title
        .as_ref()
        .map(|title| format!(" (\"{title}\")"))
        .unwrap_or_default();
    format!(
        "Message from Ox session {}{title}. Your reply in this conversation does not reach it; \
         answer with the ox tool send_message.",
        from.session_id
    )
}

/// Whether an event is a turn's end other than a normal one: an error, or a
/// stop reason other than `end_turn`.
fn ends_abnormally(event: &EventKind) -> bool {
    matches!(
        event,
        EventKind::TurnEnded { stop_reason, error }
            if error.is_some() || !matches!(stop_reason, None | Some(StopReason::EndTurn))
    )
}

/// Checks a prompt against what the agent accepts. Every agent accepts text
/// and resource links.
pub fn check_prompt(
    prompt: &[ContentBlock],
    accepts: &PromptCapabilities,
) -> Result<(), ErrorBody> {
    if prompt.is_empty() {
        return Err(invalid_request("the prompt is empty".into()));
    }
    for block in prompt {
        let (kind, accepted) = match block {
            ContentBlock::Text(_) | ContentBlock::ResourceLink(_) => continue,
            ContentBlock::Image(_) => ("image", accepts.image.is_some()),
            ContentBlock::Audio(_) => ("audio", accepts.audio.is_some()),
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(_)
                | EmbeddedResourceResource::BlobResourceContents(_) => {
                    ("embedded resource", accepts.embedded_context.is_some())
                }
                _ => ("unknown resource", false),
            },
            _ => ("unknown", false),
        };
        if !accepted {
            return Err(invalid_request(format!(
                "the agent does not accept {kind} content"
            )));
        }
    }
    Ok(())
}

/// Checks a form elicitation that oxd can show to clients, and returns its
/// message, tool call, and schema. Other modes, a request scope, and forms a
/// client cannot render are refused with `-32602`. A client renders text,
/// `oneOf` choices and multi-selects, and leaves out an optional property of
/// unknown type.
pub fn check_form(
    request: CreateElicitationRequest,
) -> Result<(String, Option<ToolCallId>, ElicitationSchema), agent_client_protocol::Error> {
    let refuse = |reason: String| agent_client_protocol::Error::invalid_params().data(reason);
    let ElicitationMode::Form(form) = request.mode else {
        return Err(refuse("oxd supports form elicitation only".into()));
    };
    let ElicitationScope::Session(scope) = form.scope else {
        return Err(refuse(
            "oxd supports session-scoped elicitation only".into(),
        ));
    };
    let schema = form.requested_schema;
    for (name, property) in &schema.properties {
        match property {
            ElicitationPropertySchema::String(string) if string.enum_values.is_none() => {}
            ElicitationPropertySchema::Array(_) | ElicitationPropertySchema::Other(_) => {}
            _ => {
                return Err(refuse(format!(
                    "property {name} is of a kind oxd cannot show"
                )));
            }
        }
    }
    for name in schema.required.iter().flatten() {
        match schema.properties.get(name) {
            None => return Err(refuse(format!("required property {name} has no schema"))),
            Some(ElicitationPropertySchema::Other(other)) => {
                return Err(refuse(format!(
                    "required property {name} has the unknown type {}",
                    other.type_
                )));
            }
            Some(_) => {}
        }
    }
    Ok((request.message, scope.tool_call_id, schema))
}

/// The unseen outcome new `events` give a session that is out of focus:
/// `Error` for a turn that failed or an agent that exited with an error,
/// otherwise `Finished` for a turn that ended other than by cancellation.
/// Events that park the agent, as a park or an archive does, give nothing.
pub fn unseen(events: &[Event]) -> Option<Unseen> {
    if events
        .iter()
        .any(|event| matches!(event.kind, EventKind::AgentParked))
    {
        return None;
    }
    let mut unseen = None;
    for event in events {
        match &event.kind {
            EventKind::TurnEnded { error: Some(_), .. }
            | EventKind::TurnEnded {
                stop_reason: Some(StopReason::Error(_)),
                ..
            }
            | EventKind::AgentExited { error: Some(_) } => return Some(Unseen::Error),
            EventKind::TurnEnded {
                stop_reason: Some(StopReason::Cancelled),
                ..
            } => {}
            EventKind::TurnEnded { .. } => unseen = Some(Unseen::Finished),
            _ => {}
        }
    }
    unseen
}

fn invalid_request(message: String) -> ErrorBody {
    ErrorBody {
        code: code::INVALID_REQUEST,
        message,
    }
}

pub fn invalid_state(state: SessionState) -> ErrorBody {
    ErrorBody {
        code: code::INVALID_STATE,
        message: format!("session is {}", state.as_str()),
    }
}

fn not_pending(request_id: &str) -> ErrorBody {
    ErrorBody {
        code: code::INVALID_STATE,
        message: format!("request {request_id} is not pending"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use agent_client_protocol::v2::schema::{
        BackgroundToolCall, ContentChunk, ElicitationAcceptAction, ElicitationContentValue,
        ElicitationFormMode, ElicitationRequestScope, ElicitationSessionScope, ElicitationUrlMode,
        EnumOption, ErrorStopReason, IdleStateUpdate, MultiSelectPropertySchema,
        OtherElicitationPropertySchema, OtherStateUpdate, PermissionOptionKind,
        RequiresActionStateUpdate, RunningStateUpdate, SessionInfoUpdate, StopReason,
        StringPropertySchema, TextContent,
    };
    use serde_json::json;

    use super::*;

    fn started() -> State {
        let mut state = State::new("s".into());
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        state
    }

    #[test]
    fn commands_replace_the_catalog_including_on_resume_without_replay() {
        let commands = |name: &str| {
            vec![AvailableCommand::Command(
                agent_client_protocol::v2::schema::Command::new(name, "Run"),
            )]
        };
        let mut state = State::new("s".into());
        state.record(Change::Started(Agent {
            available_commands: commands("first"),
            ..Agent::saved("agent".into())
        }));
        assert_eq!(state.agent().unwrap().available_commands, commands("first"));
        state.record(Change::Update(SessionUpdate::AvailableCommandsUpdate(
            agent_client_protocol::v2::schema::AvailableCommandsUpdate::new(commands("second")),
        )));
        assert_eq!(
            state.agent().unwrap().available_commands,
            commands("second")
        );
        state.record(Change::Parked);
        state.load().unwrap();
        // The session keeps its old history, but the new setup catalog wins.
        state.record(Change::Started(Agent {
            available_commands: commands("resumed"),
            ..Agent::saved("agent".into())
        }));
        assert_eq!(
            state.agent().unwrap().available_commands,
            commands("resumed")
        );
        state.record(Change::Update(SessionUpdate::AvailableCommandsUpdate(
            agent_client_protocol::v2::schema::AvailableCommandsUpdate::new(Vec::new()),
        )));
        assert!(state.agent().unwrap().available_commands.is_empty());
        state.record(Change::Parked);
        state.load().unwrap();
        state.record(Change::Started(Agent::saved("agent".into())));
        assert!(state.agent().unwrap().available_commands.is_empty());
    }

    fn prompt(state: &mut State) -> Result<(), ErrorBody> {
        state.request(Request::Prompt(text("hello"))).map(|_| ())
    }

    fn text(text: &str) -> Vec<ContentBlock> {
        vec![ContentBlock::Text(TextContent::new(text))]
    }

    /// Records a permission request with the option `allow` and returns the
    /// receiver of its answer.
    fn request_permission(state: &mut State) -> oneshot::Receiver<RequestPermissionOutcome> {
        let (answer, outcome) = oneshot::channel();
        state.record(Change::PermissionRequested {
            request: RequestPermissionRequest::new(
                "agent",
                "Tool",
                vec![PermissionOption::new(
                    "allow",
                    "Allow",
                    PermissionOptionKind::AllowOnce,
                )],
            ),
            session_request: None,
            answer,
        });
        outcome
    }

    fn state_change(state: StateUpdate) -> Change {
        Change::Update(SessionUpdate::StateUpdate(state))
    }

    fn running() -> Change {
        state_change(StateUpdate::Running(RunningStateUpdate::new()))
    }

    fn idle(stop_reason: Option<StopReason>) -> Change {
        state_change(StateUpdate::Idle(
            IdleStateUpdate::new().stop_reason(stop_reason),
        ))
    }

    fn chunk() -> Change {
        Change::Update(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new("late")),
            "message",
        )))
    }

    /// The names of the events after the first `skip`.
    fn kinds(state: &State, skip: u64) -> Vec<&'static str> {
        state
            .events_after(skip)
            .iter()
            .map(|event| match event.kind {
                EventKind::AgentStarted { .. } => "agent_started",
                EventKind::PromptStarted { .. } => "prompt_started",
                EventKind::PromptSteered { .. } => "prompt_steered",
                EventKind::SteeredPromptTaken { .. } => "steered_prompt_taken",
                EventKind::SteeredPromptDropped { .. } => "steered_prompt_dropped",
                EventKind::TurnStarted => "turn_started",
                EventKind::Update(_) => "update",
                EventKind::PermissionRequested { .. } => "permission_requested",
                EventKind::PermissionResolved { .. } => "permission_resolved",
                EventKind::ElicitationRequested { .. } => "elicitation_requested",
                EventKind::ElicitationResolved { .. } => "elicitation_resolved",
                EventKind::TurnEnded { .. } => "turn_ended",
                EventKind::AgentExited { .. } => "agent_exited",
                EventKind::AgentParked => "agent_parked",
                EventKind::SessionClosed => "session_closed",
            })
            .collect()
    }

    #[test]
    fn state_follows_the_turn_and_its_permission_requests() {
        let mut state = State::new("s".into());
        assert_eq!(state.state(), SessionState::Starting);
        assert_eq!(prompt(&mut state).unwrap_err().code, code::INVALID_STATE);

        let mut state = started();
        assert_eq!(state.state(), SessionState::Idle);
        prompt(&mut state).unwrap();
        assert_eq!(state.state(), SessionState::Running);
        state.record(running());

        let mut outcome = request_permission(&mut state);
        assert_eq!(state.state(), SessionState::AwaitingPermission);
        let permit = |option_id: &str| Request::Permit {
            request_id: "1".into(),
            option_id: option_id.into(),
        };
        let error = state.request(permit("deny")).unwrap_err();
        assert_eq!(error.code, code::INVALID_REQUEST);
        state.request(permit("allow")).unwrap();
        assert_eq!(state.state(), SessionState::Running);
        assert!(matches!(
            outcome.try_recv(),
            Ok(RequestPermissionOutcome::Selected(selected)) if &*selected.option_id.0 == "allow"
        ));
        let error = state.request(permit("allow")).unwrap_err();
        assert_eq!(error.code, code::INVALID_STATE);

        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(
            kinds(&state, 1),
            [
                "prompt_started",
                "update",
                "permission_requested",
                "permission_resolved",
                "update",
                "turn_ended"
            ]
        );
        assert_eq!(
            state.events_after(6)[0].kind,
            EventKind::TurnEnded {
                stop_reason: Some(StopReason::EndTurn),
                error: None
            }
        );
    }

    #[test]
    fn a_first_prompt_is_recorded_while_starting_and_runs_once_started() {
        let mut state = State::new("s".into());
        let seq = state.record_first_prompt(vec![ContentBlock::Text(TextContent::new("hello"))]);
        assert_eq!(seq, 1);
        assert_eq!(state.state(), SessionState::Starting);
        assert_eq!(prompt(&mut state).unwrap_err().code, code::INVALID_STATE);
        state.record(Change::Started(Agent::saved("agent".into())));
        assert_eq!(state.state(), SessionState::Running);
        state.record(Change::PromptRejected {
            prompt: seq,
            error: "refused".into(),
        });
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(
            kinds(&state, 0),
            ["prompt_started", "agent_started", "turn_ended"]
        );
    }

    #[test]
    fn a_first_prompt_ends_when_the_agent_exits_while_starting() {
        for (error, ended) in [
            (Some("no agent".to_string()), "no agent"),
            (None, "the agent exited before the prompt"),
        ] {
            let mut state = State::new("s".into());
            state.record_first_prompt(vec![ContentBlock::Text(TextContent::new("hello"))]);
            state.record(Change::Exited(error.clone()));
            assert_eq!(state.state(), SessionState::Exited);
            let events = state.events_after(1);
            assert_eq!(
                events.iter().map(|event| &event.kind).collect::<Vec<_>>(),
                [
                    &EventKind::TurnEnded {
                        stop_reason: None,
                        error: Some(ended.into()),
                    },
                    &EventKind::AgentExited { error },
                ]
            );
        }
    }

    #[test]
    fn exit_and_close_end_the_turn_and_its_permission_requests() {
        for (change, last) in [
            (Change::Exited(None), "agent_exited"),
            (Change::Closed, "session_closed"),
        ] {
            let mut state = started();
            prompt(&mut state).unwrap();
            let mut outcome = request_permission(&mut state);
            state.record(change);
            assert_eq!(state.state(), SessionState::Exited);
            assert_eq!(
                kinds(&state, 3),
                ["permission_resolved", "turn_ended", last]
            );
            assert_eq!(
                outcome.try_recv(),
                Ok(CancelledPermissionOutcome::new().into()),
                "{last}"
            );
        }
    }

    #[test]
    fn the_agent_starts_and_ends_turns_of_its_own() {
        let mut state = started();
        state.record(idle(None));
        assert_eq!(kinds(&state, 1), ["update"]);
        assert_eq!(state.state(), SessionState::Idle);

        state.record(running());
        assert_eq!(state.state(), SessionState::Running);
        state.record(running());

        let mut outcome = request_permission(&mut state);
        assert_eq!(state.state(), SessionState::AwaitingPermission);
        state.record(Change::Cancelled);
        assert_eq!(
            outcome.try_recv(),
            Ok(CancelledPermissionOutcome::new().into())
        );
        assert_eq!(state.state(), SessionState::Running);

        state.record(idle(Some(StopReason::Cancelled)));
        assert_eq!(state.state(), SessionState::Idle);
        state.record(idle(None));
        assert_eq!(
            kinds(&state, 1),
            [
                "update",
                "update",
                "turn_started",
                "update",
                "permission_requested",
                "permission_resolved",
                "update",
                "turn_ended",
                "update"
            ]
        );
        assert_eq!(
            state.events_after(8)[0].kind,
            EventKind::TurnEnded {
                stop_reason: Some(StopReason::Cancelled),
                error: None
            }
        );
        prompt(&mut state).unwrap();
    }

    #[test]
    fn a_repeated_idle_snapshot_after_a_prompt_leaves_the_turn_running() {
        let mut state = started();
        prompt(&mut state).unwrap();
        // The agent repeats the snapshot it sent before the prompt, then
        // reports the work the prompt asked for, then stops.
        state.record(idle(None));
        assert_eq!(state.state(), SessionState::Running);
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(
            kinds(&state, 1),
            ["prompt_started", "update", "update", "update", "turn_ended"]
        );
    }

    /// The agent's rejection of the last prompt sent.
    fn rejected(state: &State, error: &str) -> Change {
        let prompt = state
            .events_after(0)
            .iter()
            .rev()
            .find(|event| matches!(event.kind, EventKind::PromptStarted { .. }))
            .map_or(0, |event| event.seq);
        Change::PromptRejected {
            prompt,
            error: error.into(),
        }
    }

    #[test]
    fn a_rejected_prompt_ends_the_turn_with_its_error() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(rejected(&state, "not inserted"));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(kinds(&state, 1), ["prompt_started", "turn_ended"]);
        assert_eq!(
            state.events_after(2)[0].kind,
            EventKind::TurnEnded {
                stop_reason: None,
                error: Some("not inserted".into())
            }
        );
        // The agent may report the state it was already in.
        state.record(idle(None));
        assert_eq!(kinds(&state, 3), ["update"]);
        prompt(&mut state).unwrap();
    }

    #[test]
    fn a_rejected_prompt_after_the_agent_reported_work_waits_for_idle() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(running());
        state.record(rejected(&state, "late"));
        assert_eq!(state.state(), SessionState::Running);
        state.record(idle(Some(StopReason::Cancelled)));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(
            kinds(&state, 1),
            ["prompt_started", "update", "update", "turn_ended"]
        );
        assert_eq!(
            state.events_after(4)[0].kind,
            EventKind::TurnEnded {
                stop_reason: Some(StopReason::Cancelled),
                error: None
            }
        );
    }

    #[test]
    fn an_answered_prompt_that_was_rejected_after_its_turn_ended_changes_nothing() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        state.record(rejected(&state, "late"));
        assert_eq!(
            kinds(&state, 1),
            ["prompt_started", "update", "update", "turn_ended"]
        );
    }

    #[test]
    fn a_late_rejection_leaves_the_next_prompt_running() {
        let mut state = started();
        prompt(&mut state).unwrap();
        let first = rejected(&state, "late");
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        prompt(&mut state).unwrap();
        state.record(first);
        assert_eq!(state.state(), SessionState::Running);
        state.record(running());
        assert_eq!(
            kinds(&state, 1),
            [
                "prompt_started",
                "update",
                "update",
                "turn_ended",
                "prompt_started",
                "update"
            ]
        );
    }

    /// Queues a prompt with the ID `id`.
    fn enqueue(state: &mut State, id: &str) {
        state
            .request(Request::QueueAdd(QueuedPrompt {
                id: id.into(),
                prompt: text(id),
            }))
            .unwrap();
    }

    fn queued_ids(state: &State) -> Vec<&str> {
        state
            .queued()
            .iter()
            .map(|queued| queued.id.as_str())
            .collect()
    }

    /// The text of the prompt `start_queued` starts, if any.
    fn start_queued(state: &mut State) -> Option<String> {
        state.start_queued().map(|(prompt, _)| match &prompt[0] {
            ContentBlock::Text(text) => text.text.clone(),
            _ => unreachable!("queued prompts are text"),
        })
    }

    /// The seq of the last `PromptSteered` event.
    fn last_steered(state: &State) -> u64 {
        state
            .events_after(0)
            .iter()
            .rev()
            .find(|event| matches!(event.kind, EventKind::PromptSteered { .. }))
            .map(|event| event.seq)
            .unwrap()
    }

    #[test]
    fn a_prompt_during_a_turn_is_steered_and_taken_when_the_agent_accepts_it() {
        let mut state = started();
        prompt(&mut state).unwrap();
        let (_, seq) = state
            .request(Request::Prompt(text("also")))
            .unwrap()
            .unwrap();
        assert_eq!(seq, last_steered(&state));
        state.record(running());
        state.record(Change::PromptAccepted { prompt: seq });
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(
            kinds(&state, 1),
            [
                "prompt_started",
                "prompt_steered",
                "update",
                "steered_prompt_taken",
                "update",
                "turn_ended"
            ]
        );
        assert_eq!(
            state.events_after(4)[0].kind,
            EventKind::SteeredPromptTaken { prompt: seq }
        );
        assert_eq!(state.state(), SessionState::Idle);
    }

    fn message(state: &mut State, text: &str) -> Result<(Vec<ContentBlock>, u64), ErrorBody> {
        state
            .request(Request::Message {
                text: text.into(),
                from: Sender {
                    session_id: "peer".into(),
                    title: Some("Plan the work".into()),
                },
            })
            .map(|sent| sent.expect("a message is sent"))
    }

    #[test]
    fn a_peer_message_starts_a_turn_or_steers_with_its_sender_and_a_header_for_the_agent() {
        let sender = Some(Sender {
            session_id: "peer".into(),
            title: Some("Plan the work".into()),
        });
        let mut state = started();
        let (agent_prompt, seq) = message(&mut state, "hello").unwrap();
        assert_eq!(
            state.events_after(seq - 1)[0].kind,
            EventKind::PromptStarted {
                prompt: text("hello"),
                from: sender.clone(),
            }
        );
        assert_eq!(
            agent_prompt,
            text(
                "Message from Ox session peer (\"Plan the work\"). Your reply in this \
                 conversation does not reach it; answer with the ox tool send_message.\n\nhello"
            )
        );
        assert!(state.in_turn());
        let (_, seq) = message(&mut state, "also").unwrap();
        assert_eq!(
            state.events_after(seq - 1)[0].kind,
            EventKind::PromptSteered {
                prompt: text("also"),
                from: sender,
            }
        );
    }

    #[test]
    fn an_untitled_sender_is_named_by_its_id_alone() {
        let header = message_header(&Sender {
            session_id: "peer".into(),
            title: None,
        });
        assert!(
            header.starts_with("Message from Ox session peer. "),
            "{header}"
        );
    }

    #[test]
    fn a_peer_message_after_a_cancelled_turn_leaves_the_queue_held() {
        let mut state = started();
        prompt(&mut state).unwrap();
        enqueue(&mut state, "first");
        state.record(running());
        state.record(Change::Cancelled);
        state.record(idle(Some(StopReason::Cancelled)));
        message(&mut state, "hello").unwrap();
        assert_eq!(state.state(), SessionState::Running);
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(start_queued(&mut state), None);
        assert_eq!(queued_ids(&state), ["first"]);
    }

    #[test]
    fn queued_prompts_start_one_turn_each_in_order_after_normal_ends() {
        let mut state = started();
        prompt(&mut state).unwrap();
        enqueue(&mut state, "first");
        enqueue(&mut state, "second");
        assert_eq!(start_queued(&mut state), None, "a turn is running");
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(start_queued(&mut state).as_deref(), Some("first"));
        assert_eq!(start_queued(&mut state), None, "one prompt per turn");
        assert_eq!(queued_ids(&state), ["second"]);
        state.record(running());
        state.record(idle(None));
        assert_eq!(start_queued(&mut state).as_deref(), Some("second"));
        assert!(state.queued().is_empty());
        assert_eq!(state.state(), SessionState::Running);
    }

    #[test]
    fn a_prompt_queued_on_an_idle_session_starts_at_once() {
        let mut state = started();
        enqueue(&mut state, "first");
        assert_eq!(start_queued(&mut state).as_deref(), Some("first"));
        assert_eq!(kinds(&state, 1), ["prompt_started"]);
    }

    #[test]
    fn cancelling_holds_the_queue_until_a_prompt_is_sent() {
        let mut state = started();
        prompt(&mut state).unwrap();
        enqueue(&mut state, "first");
        enqueue(&mut state, "second");
        state.record(running());
        state.record(Change::Cancelled);
        state.record(idle(Some(StopReason::Cancelled)));
        assert_eq!(start_queued(&mut state), None);
        prompt(&mut state).unwrap();
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(start_queued(&mut state).as_deref(), Some("first"));
    }

    #[test]
    fn turns_that_end_other_than_normally_hold_the_queue_until_one_is_sent_now() {
        let ends: [(&str, Change); 3] = [
            ("max tokens", idle(Some(StopReason::MaxTokens))),
            ("refusal", idle(Some(StopReason::Refusal))),
            (
                "error",
                state_change(StateUpdate::Idle(
                    IdleStateUpdate::new().stop_reason(StopReason::Error(ErrorStopReason::new())),
                )),
            ),
        ];
        for (name, end) in ends {
            let mut state = started();
            prompt(&mut state).unwrap();
            enqueue(&mut state, "first");
            enqueue(&mut state, "second");
            state.record(running());
            state.record(end);
            assert_eq!(start_queued(&mut state), None, "{name}");
            state
                .request(Request::QueueSend {
                    id: "second".into(),
                })
                .unwrap();
            assert_eq!(queued_ids(&state), ["first"], "{name}");
            state.record(running());
            state.record(idle(Some(StopReason::EndTurn)));
            assert_eq!(start_queued(&mut state).as_deref(), Some("first"), "{name}");
        }
    }

    #[test]
    fn a_rejected_prompt_holds_the_queue() {
        let mut state = started();
        prompt(&mut state).unwrap();
        enqueue(&mut state, "first");
        state.record(rejected(&state, "not inserted"));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(start_queued(&mut state), None);
    }

    #[test]
    fn a_refused_send_leaves_the_prompt_queued() {
        let mut state = State::new("s".into());
        enqueue(&mut state, "first");
        let refused = state.request(Request::QueueSend { id: "first".into() });
        assert_eq!(refused.unwrap_err().code, code::INVALID_STATE);
        assert_eq!(queued_ids(&state), ["first"]);
    }

    #[test]
    fn steered_prompts_the_agent_does_not_take_are_dropped() {
        // Rejected.
        let mut state = started();
        prompt(&mut state).unwrap();
        prompt(&mut state).unwrap();
        let steered = last_steered(&state);
        state.record(Change::PromptRejected {
            prompt: steered,
            error: "cancelled".into(),
        });
        assert_eq!(
            state.events_after(3)[0].kind,
            EventKind::SteeredPromptDropped { prompt: steered }
        );
        assert_eq!(state.state(), SessionState::Running);

        // Left when the agent exits.
        let mut state = started();
        prompt(&mut state).unwrap();
        prompt(&mut state).unwrap();
        state.record(Change::Exited(None));
        assert_eq!(
            kinds(&state, 3),
            ["steered_prompt_dropped", "turn_ended", "agent_exited"]
        );

        // Left when oxd stopped.
        let mut state = State::restored("s".into(), None, "agent".into(), 3, Queue::default());
        state.fill(stored(vec![
            agent_started(),
            EventKind::PromptStarted {
                prompt: text("hello"),
                from: None,
            },
            EventKind::PromptSteered {
                prompt: text("also"),
                from: None,
            },
        ]));
        assert_eq!(
            state.events_after(3)[0].kind,
            EventKind::SteeredPromptDropped { prompt: 3 }
        );
        assert_eq!(kinds(&state, 4), ["turn_ended"]);
        assert_eq!(
            state.take_changed_queue().map(|queue| queue.paused),
            Some(true)
        );
    }

    #[test]
    fn a_steered_prompt_pending_at_a_turn_end_runs_before_the_queue() {
        let mut state = started();
        prompt(&mut state).unwrap();
        enqueue(&mut state, "first");
        prompt(&mut state).unwrap();
        let steered = last_steered(&state);
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(state.state(), SessionState::Idle);
        assert!(!state.quiet());
        assert_eq!(start_queued(&mut state), None);
        // The agent starts a turn of its own with the steered prompt.
        state.record(running());
        state.record(Change::PromptAccepted { prompt: steered });
        assert_eq!(start_queued(&mut state), None);
        state.record(idle(Some(StopReason::EndTurn)));
        assert_eq!(start_queued(&mut state).as_deref(), Some("first"));
    }

    #[test]
    fn queued_prompts_are_edited_in_place_and_removed_by_id() {
        let mut state = started();
        prompt(&mut state).unwrap();
        enqueue(&mut state, "first");
        enqueue(&mut state, "second");
        state
            .request(Request::QueueEdit(QueuedPrompt {
                id: "first".into(),
                prompt: text("edited"),
            }))
            .unwrap();
        assert_eq!(queued_ids(&state), ["first", "second"]);
        assert_eq!(state.queued()[0].prompt, text("edited"));
        state
            .request(Request::QueueRemove { id: "first".into() })
            .unwrap();
        assert_eq!(queued_ids(&state), ["second"]);
        for unknown in [
            Request::QueueEdit(QueuedPrompt {
                id: "first".into(),
                prompt: text("again"),
            }),
            Request::QueueRemove { id: "first".into() },
            Request::QueueSend { id: "first".into() },
        ] {
            assert_eq!(state.request(unknown).unwrap_err().code, code::NOT_FOUND);
        }
        assert_eq!(
            state
                .request(Request::QueueEdit(QueuedPrompt {
                    id: "second".into(),
                    prompt: Vec::new(),
                }))
                .unwrap_err()
                .code,
            code::INVALID_REQUEST
        );
    }

    #[test]
    fn states_other_than_idle_are_work() {
        let unknown = || {
            state_change(StateUpdate::Other(OtherStateUpdate::new(
                "_waiting",
                BTreeMap::new(),
            )))
        };
        let requires_action =
            || state_change(StateUpdate::RequiresAction(RequiresActionStateUpdate::new()));
        for change in [unknown(), requires_action()] {
            let mut state = started();
            state.record(change);
            assert_eq!(state.state(), SessionState::Running);
            assert_eq!(kinds(&state, 1), ["update", "turn_started"]);
            // An unknown state is never taken for idle.
            state.record(running());
            state.record(idle(None));
            assert_eq!(state.state(), SessionState::Idle);
        }
    }

    #[test]
    fn exit_ends_a_turn_the_agent_started() {
        let mut state = started();
        state.record(running());
        state.record(Change::Exited(None));
        assert_eq!(
            kinds(&state, 1),
            ["update", "turn_started", "turn_ended", "agent_exited"]
        );
        assert!(matches!(
            state.events_after(3)[0].kind,
            EventKind::TurnEnded {
                stop_reason: None,
                error: Some(_)
            }
        ));
    }

    #[test]
    fn a_turn_ends_once() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(running());
        state.record(idle(Some(StopReason::EndTurn)));
        state.record(idle(Some(StopReason::EndTurn)));
        state.record(rejected(&state, "late"));
        assert_eq!(
            kinds(&state, 1),
            ["prompt_started", "update", "update", "turn_ended", "update"]
        );
    }

    #[test]
    fn nothing_follows_session_closed() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(Change::Closed);
        let closed = state.last_seq();
        let (answer, _outcome) = oneshot::channel();
        for change in [
            chunk(),
            Change::ConfigOptionsSet(Vec::new()),
            Change::PermissionRequested {
                request: RequestPermissionRequest::new("agent", "Tool", Vec::new()),
                session_request: None,
                answer,
            },
            Change::Cancelled,
            rejected(&state, "late"),
            running(),
            Change::Exited(None),
            Change::Closed,
        ] {
            state.record(change);
        }
        assert_eq!(prompt(&mut state).unwrap_err().code, code::INVALID_STATE);
        assert_eq!(state.last_seq(), closed);
        state.subscribe(closed - 1).unwrap();
        let error = state.subscribe(closed).unwrap_err();
        assert_eq!(error.code, code::INVALID_REQUEST);
    }

    fn replayed(text: &str) -> Change {
        Change::Update(SessionUpdate::UserMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
            "message",
        )))
    }

    fn restored() -> State {
        let mut state = State::restored(
            "s".into(),
            Some("Fix the build".into()),
            "agent".into(),
            0,
            Queue::default(),
        );
        state.fill(Vec::new());
        state
    }

    /// The history of a session whose daemon stopped after `kinds`.
    fn stored(kinds: Vec<EventKind>) -> Vec<Event> {
        kinds
            .into_iter()
            .zip(1..)
            .map(|(kind, seq)| Event {
                session_id: "s".into(),
                seq,
                recorded_at: 0,
                kind,
            })
            .collect()
    }

    fn agent_started() -> EventKind {
        EventKind::AgentStarted {
            agent_session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }
    }

    fn requested(request_id: &str) -> EventKind {
        EventKind::PermissionRequested {
            request_id: request_id.into(),
            title: "Tool".into(),
            description: None,
            subject: None,
            options: Vec::new(),
            session_request: None,
        }
    }

    #[test]
    fn an_appended_event_carries_the_time_it_was_recorded() {
        let mut state = started();
        let before = crate::daemon::now();
        state.record(Change::Closed);
        let after = crate::daemon::now();
        let recorded_at = state.events_after(state.last_seq() - 1)[0].recorded_at;
        assert!((before..=after).contains(&recorded_at));
    }

    #[test]
    fn a_restored_session_holds_no_events_until_it_is_filled() {
        let mut state = State::restored("s".into(), None, "agent".into(), 3, Queue::default());
        assert!(!state.loaded());
        assert_eq!(state.last_seq(), 3);
        assert!(state.events_after(0).is_empty());
        state.record(Change::Closed);
        assert_eq!(state.last_seq(), 3);
        assert!(state.take_unlogged().is_empty());
    }

    #[test]
    fn fill_closes_the_turn_and_the_permission_request_a_stop_left_open() {
        let mut state = State::restored("s".into(), None, "agent".into(), 6, Queue::default());
        state.fill(stored(vec![
            agent_started(),
            EventKind::TurnStarted,
            requested("1"),
            EventKind::PermissionResolved {
                request_id: "1".into(),
                outcome: CancelledPermissionOutcome::new().into(),
            },
            requested("2"),
            requested("7"),
        ]));
        assert!(state.loaded());
        assert_eq!(
            kinds(&state, 0),
            [
                "agent_started",
                "turn_started",
                "permission_requested",
                "permission_resolved",
                "permission_requested",
                "permission_requested",
                "permission_resolved",
                "permission_resolved",
                "turn_ended",
            ]
        );
        let logged = state.take_unlogged();
        assert_eq!(logged.iter().map(|e| e.seq).collect::<Vec<_>>(), [7, 8, 9]);
        assert!(matches!(
            &logged[2].kind,
            EventKind::TurnEnded { error: Some(error), .. } if error == "oxd stopped during the turn"
        ));
        assert!(state.take_unlogged().is_empty());
    }

    #[test]
    fn fill_leaves_a_finished_history_as_it_is() {
        let mut state = State::restored("s".into(), None, "agent".into(), 4, Queue::default());
        state.fill(stored(vec![
            agent_started(),
            EventKind::TurnStarted,
            EventKind::TurnEnded {
                stop_reason: None,
                error: None,
            },
            EventKind::AgentParked,
        ]));
        assert_eq!(state.last_seq(), 4);
        assert!(state.take_unlogged().is_empty());
    }

    #[test]
    fn permission_request_ids_continue_after_the_history() {
        let mut state = State::restored("s".into(), None, "agent".into(), 2, Queue::default());
        state.fill(stored(vec![agent_started(), requested("5")]));
        state.load().unwrap();
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        state.record(running());
        let _outcome = request_permission(&mut state);
        let last = state.events_after(state.last_seq() - 1);
        assert!(matches!(
            &last[0].kind,
            EventKind::PermissionRequested { request_id, .. } if request_id == "6"
        ));
    }

    #[test]
    fn session_closed_is_not_logged() {
        let mut state = started();
        state.take_unlogged();
        state.record(Change::Closed);
        assert_eq!(kinds(&state, 1), ["session_closed"]);
        assert!(state.take_unlogged().is_empty());
    }

    #[test]
    fn a_restored_session_is_exited_and_loads_its_replay_as_history() {
        let mut state = restored();
        assert_eq!(state.state(), SessionState::Exited);
        assert_eq!(state.title(), Some("Fix the build"));
        assert_eq!(state.agent().unwrap().session_id, "agent");
        assert_eq!(state.last_seq(), 0);
        assert_eq!(prompt(&mut state).unwrap_err().code, code::INVALID_STATE);

        let resume = state.load().unwrap();
        assert_eq!(resume.session_id, "agent");
        assert!(resume.replay);
        assert_eq!(state.state(), SessionState::Starting);
        state.record(replayed("hello"));
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(kinds(&state, 0), ["update", "agent_started"]);
        assert_eq!(state.title(), Some("Fix the build"));
    }

    #[test]
    fn a_restored_session_whose_history_has_a_prompt_loads_without_replay() {
        let mut state = State::restored("s".into(), None, "agent".into(), 2, Queue::default());
        state.fill(stored(vec![
            EventKind::PromptStarted {
                prompt: vec![ContentBlock::Text(TextContent::new("Fix the build"))],
                from: None,
            },
            EventKind::TurnEnded {
                stop_reason: Some(StopReason::EndTurn),
                error: None,
            },
        ]));
        let resume = state.load().unwrap();
        assert!(!resume.replay);
        assert_eq!(resume.session_id, "agent");
    }

    fn idle_with(background: Vec<BackgroundWork>) -> Change {
        state_change(StateUpdate::Idle(
            IdleStateUpdate::new().background(background),
        ))
    }

    fn work() -> BackgroundWork {
        BackgroundWork::ToolCall(BackgroundToolCall::new("tool"))
    }

    #[test]
    fn a_session_is_quiet_when_the_agent_reports_no_background_work() {
        let mut state = started();
        assert!(!state.quiet(), "an agent that reports no list is not quiet");
        state.record(idle_with(vec![work()]));
        assert!(!state.quiet());
        state.record(idle_with(Vec::new()));
        assert!(state.quiet());
        state.record(idle(None));
        assert!(!state.quiet(), "a state without the list is not quiet");

        state.record(idle_with(Vec::new()));
        prompt(&mut state).unwrap();
        assert!(!state.quiet());
        state.record(idle_with(Vec::new()));
        state.record(running());
        state.record(idle_with(Vec::new()));
        assert!(state.quiet());
        let _answer = request_permission(&mut state);
        assert!(!state.quiet());
    }

    #[test]
    fn the_resume_snapshot_gives_the_session_its_background_work() {
        let resumed = |snapshot: Change| {
            let mut state = restored();
            state.load().unwrap();
            state.record(snapshot);
            state.record(Change::Started(Agent {
                session_id: "agent".into(),
                prompt_capabilities: PromptCapabilities::default(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            }));
            state
        };
        assert!(resumed(idle_with(Vec::new())).quiet());
        assert!(!resumed(idle_with(vec![work()])).quiet());
        assert!(!resumed(idle(None)).quiet());
    }

    #[test]
    fn parking_exits_a_quiet_session_and_records_it() {
        let mut state = started();
        state.record(idle_with(Vec::new()));
        state.record(Change::Parked);
        assert_eq!(state.state(), SessionState::Exited);
        assert_eq!(
            kinds(&state, 0),
            ["agent_started", "update", "agent_parked"]
        );
        assert_eq!(state.load().unwrap().session_id, "agent");

        let mut state = State::new("s".into());
        state.record(Change::Parked);
        assert_eq!(state.state(), SessionState::Starting);
        assert!(kinds(&state, 0).is_empty());
    }

    #[test]
    fn the_resume_snapshot_sets_the_state_without_recording_a_turn() {
        let resumed = |state: &mut State, snapshot: Change| {
            state.load().unwrap();
            state.record(replayed("hello"));
            state.record(snapshot);
            state.record(Change::Started(Agent {
                session_id: "agent".into(),
                prompt_capabilities: PromptCapabilities::default(),
                config_options: Vec::new(),
                available_commands: Vec::new(),
            }));
        };

        let mut state = restored();
        resumed(&mut state, idle(None));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(kinds(&state, 0), ["update", "update", "agent_started"]);

        let mut state = restored();
        resumed(&mut state, running());
        assert_eq!(state.state(), SessionState::Running);
        assert_eq!(kinds(&state, 0), ["update", "update", "agent_started"]);
        state.record(idle(Some(StopReason::Cancelled)));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(
            kinds(&state, 0),
            ["update", "update", "agent_started", "update"]
        );
        prompt(&mut state).unwrap();

        // The snapshot counts when the session keeps its history and drops
        // the replay, and a turn it reports ends silently if the agent exits.
        let mut state = started();
        state.record(Change::Exited(None));
        resumed(&mut state, running());
        assert_eq!(state.state(), SessionState::Running);
        state.record(Change::Exited(None));
        assert_eq!(
            kinds(&state, 0),
            [
                "agent_started",
                "agent_exited",
                "agent_started",
                "agent_exited"
            ]
        );
    }

    #[test]
    fn a_session_with_a_history_loads_without_duplicating_it() {
        let mut state = started();
        state.record(Change::Exited(None));
        assert!(!state.load().unwrap().replay);
        state.record(replayed("hello"));
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        assert_eq!(state.state(), SessionState::Idle);
        assert_eq!(
            kinds(&state, 0),
            ["agent_started", "agent_exited", "agent_started"]
        );
    }

    #[test]
    fn only_an_exited_session_with_an_agent_session_loads() {
        let mut state = State::new("s".into());
        assert_eq!(state.load().unwrap_err().code, code::INVALID_STATE);
        state.record(Change::Exited(Some("never started".into())));
        assert_eq!(state.state(), SessionState::Exited);
        assert_eq!(state.load().unwrap_err().code, code::INVALID_STATE);

        let mut state = started();
        assert_eq!(state.load().unwrap_err().code, code::INVALID_STATE);
        state.record(Change::Closed);
        assert_eq!(state.load().unwrap_err().code, code::INVALID_STATE);
    }

    #[test]
    fn a_load_that_fails_leaves_the_session_loadable() {
        let mut state = restored();
        state.load().unwrap();
        state.record(Change::Exited(Some("cannot load".into())));
        assert_eq!(state.state(), SessionState::Exited);
        assert_eq!(state.agent().unwrap().session_id, "agent");
        assert_eq!(kinds(&state, 0), ["agent_exited"]);

        // The failed load recorded no conversation, so the next one does.
        assert!(state.load().unwrap().replay);
        state.record(replayed("hello"));
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        assert_eq!(kinds(&state, 1), ["update", "agent_started"]);
    }

    #[test]
    fn a_load_that_fails_partway_through_its_replay_records_none_of_it() {
        let mut state = restored();
        state.load().unwrap();
        state.record(replayed("hello"));
        state.record(Change::Exited(Some("load interrupted".into())));
        assert_eq!(kinds(&state, 0), ["agent_exited"]);

        state.load().unwrap();
        state.record(replayed("hello"));
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        assert_eq!(
            kinds(&state, 0),
            ["agent_exited", "update", "agent_started"]
        );
    }

    fn info(update: SessionInfoUpdate) -> Change {
        Change::Update(SessionUpdate::SessionInfoUpdate(update))
    }

    #[test]
    fn session_info_updates_set_clear_and_keep_the_title() {
        // A loading agent replays its title before it starts.
        let mut state = State::new("s".into());
        state.record(info(SessionInfoUpdate::new().title("Replayed")));
        state.record(Change::Started(Agent {
            session_id: "agent".into(),
            prompt_capabilities: PromptCapabilities::default(),
            config_options: Vec::new(),
            available_commands: Vec::new(),
        }));
        assert_eq!(state.title(), Some("Replayed"));

        state.record(info(SessionInfoUpdate::new().title("Fix the build")));
        assert_eq!(state.title(), Some("Fix the build"));
        state.record(info(
            SessionInfoUpdate::new().updated_at("2026-11-11T00:00:00Z"),
        ));
        assert_eq!(state.title(), Some("Fix the build"));
        state.record(Change::Exited(None));
        assert_eq!(state.title(), Some("Fix the build"));

        let mut state = started();
        state.record(info(SessionInfoUpdate::new().title("Fix the build")));
        state.record(info(SessionInfoUpdate::new().title(None::<String>)));
        assert_eq!(state.title(), None);
    }

    /// A form like the fake agent's: `color` (required, `red` or `blue`),
    /// `extras` (any of `sprinkles` and `nuts`, at most one), `note` (2 to 5
    /// characters), `count` (1 to 9), and `rush`.
    fn order() -> ElicitationSchema {
        ElicitationSchema::new()
            .property(
                "color",
                StringPropertySchema::new().one_of(vec![
                    EnumOption::new("red", "Red"),
                    EnumOption::new("blue", "Blue"),
                ]),
                true,
            )
            .property(
                "extras",
                MultiSelectPropertySchema::titled(vec![
                    EnumOption::new("sprinkles", "Sprinkles"),
                    EnumOption::new("nuts", "Nuts"),
                ])
                .max_items(1),
                false,
            )
            .property(
                "note",
                StringPropertySchema::new().default_value("first"),
                false,
            )
    }

    /// Records an elicitation of `order()` and returns the receiver of its
    /// answer.
    fn elicit(state: &mut State) -> oneshot::Receiver<ElicitationAction> {
        let (answer, action) = oneshot::channel();
        state.record(Change::ElicitationRequested {
            message: "Order".into(),
            tool_call_id: None,
            schema: order(),
            answer,
        });
        action
    }

    fn accept(content: serde_json::Value) -> ElicitationAction {
        ElicitationAction::Accept(ElicitationAcceptAction::new().content(
            serde_json::from_value::<BTreeMap<String, ElicitationContentValue>>(content).unwrap(),
        ))
    }

    fn respond(
        state: &mut State,
        request_id: &str,
        action: ElicitationAction,
    ) -> Result<(), ErrorBody> {
        state
            .request(Request::Respond {
                request_id: request_id.into(),
                action,
            })
            .map(|_| ())
    }

    #[test]
    fn an_elicitation_awaits_input_unless_a_permission_request_is_open() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(running());
        let mut action = elicit(&mut state);
        assert_eq!(state.state(), SessionState::AwaitingInput);
        assert!(!state.quiet());
        let _outcome = request_permission(&mut state);
        assert_eq!(state.state(), SessionState::AwaitingPermission);
        state
            .request(Request::Permit {
                request_id: "2".into(),
                option_id: "allow".into(),
            })
            .unwrap();
        assert_eq!(state.state(), SessionState::AwaitingInput);

        // Each kind of request answers only to its own method.
        let error = state
            .request(Request::Permit {
                request_id: "1".into(),
                option_id: "allow".into(),
            })
            .unwrap_err();
        assert_eq!(error.code, code::INVALID_STATE);
        let error = respond(&mut state, "2", ElicitationAction::Decline).unwrap_err();
        assert_eq!(error.code, code::INVALID_STATE);

        respond(&mut state, "1", accept(json!({"color": "blue"}))).unwrap();
        assert_eq!(action.try_recv(), Ok(accept(json!({"color": "blue"}))));
        assert_eq!(state.state(), SessionState::Running);
        let error = respond(&mut state, "1", ElicitationAction::Decline).unwrap_err();
        assert_eq!(error.code, code::INVALID_STATE);
        assert_eq!(
            state.events_after(state.last_seq() - 1)[0].kind,
            EventKind::ElicitationResolved {
                request_id: "1".into(),
                action: accept(json!({"color": "blue"})),
            }
        );
    }

    #[test]
    fn cancelling_the_turn_leaves_an_elicitation_open() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(running());
        let mut action = elicit(&mut state);
        let mut outcome = request_permission(&mut state);
        state.record(Change::Cancelled);
        assert_eq!(
            outcome.try_recv(),
            Ok(CancelledPermissionOutcome::new().into())
        );
        assert!(action.try_recv().is_err());
        assert_eq!(state.state(), SessionState::AwaitingInput);
        respond(&mut state, "1", ElicitationAction::Decline).unwrap();
        assert_eq!(action.try_recv(), Ok(ElicitationAction::Decline));
    }

    #[test]
    fn exit_park_and_close_cancel_an_elicitation() {
        for (change, last) in [
            (Change::Exited(None), "agent_exited"),
            (Change::Parked, "agent_parked"),
            (Change::Closed, "session_closed"),
        ] {
            let mut state = started();
            prompt(&mut state).unwrap();
            let mut action = elicit(&mut state);
            state.record(change);
            assert_eq!(
                kinds(&state, 3),
                ["elicitation_resolved", "turn_ended", last]
            );
            assert_eq!(action.try_recv(), Ok(ElicitationAction::Cancel), "{last}");
        }
    }

    #[test]
    fn a_withdrawal_ends_only_the_elicitation_whose_receiver_closed() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(running());
        let first = elicit(&mut state);
        let mut second = elicit(&mut state);
        drop(first);
        // A client's answer to the withdrawn elicitation is refused.
        let error = respond(&mut state, "1", ElicitationAction::Decline).unwrap_err();
        assert_eq!(error.code, code::INVALID_STATE);
        state.record(Change::ElicitationWithdrawn);
        assert_eq!(
            state.events_after(state.last_seq() - 1)[0].kind,
            EventKind::ElicitationResolved {
                request_id: "1".into(),
                action: ElicitationAction::Cancel,
            }
        );
        assert_eq!(state.state(), SessionState::AwaitingInput);
        assert!(second.try_recv().is_err());
        respond(&mut state, "2", ElicitationAction::Cancel).unwrap();
        assert_eq!(second.try_recv(), Ok(ElicitationAction::Cancel));
    }

    #[test]
    fn fill_cancels_an_open_elicitation_and_ids_continue_past_it() {
        let mut state = State::restored("s".into(), None, "agent".into(), 3, Queue::default());
        state.fill(stored(vec![
            agent_started(),
            requested("1"),
            EventKind::ElicitationRequested {
                request_id: "4".into(),
                message: "Order".into(),
                tool_call_id: None,
                requested_schema: order(),
            },
        ]));
        assert_eq!(
            kinds(&state, 3),
            ["permission_resolved", "elicitation_resolved"]
        );
        assert_eq!(
            state.events_after(4)[0].kind,
            EventKind::ElicitationResolved {
                request_id: "4".into(),
                action: ElicitationAction::Cancel,
            }
        );
        state.load().unwrap();
        state.record(Change::Started(Agent::saved("agent".into())));
        let _action = elicit(&mut state);
        assert!(matches!(
            &state.events_after(state.last_seq() - 1)[0].kind,
            EventKind::ElicitationRequested { request_id, .. } if request_id == "5"
        ));
    }

    #[test]
    fn forms_a_client_cannot_render_are_refused() {
        let form = |schema: ElicitationSchema| {
            CreateElicitationRequest::new(
                ElicitationFormMode::new(
                    ElicitationSessionScope::new("agent").tool_call_id("t"),
                    schema,
                ),
                "Order",
            )
        };
        let (message, tool_call_id, schema) = check_form(form(order())).unwrap();
        assert_eq!(message, "Order");
        assert_eq!(tool_call_id, Some(ToolCallId::new("t")));
        assert_eq!(schema, order());

        let unknown = ElicitationPropertySchema::Other(OtherElicitationPropertySchema::new(
            "_point",
            BTreeMap::new(),
        ));
        let mut missing = ElicitationSchema::new();
        missing.required = Some(vec!["missing".into()]);
        check_form(form(ElicitationSchema::new().property(
            "location",
            unknown.clone(),
            false,
        )))
        .unwrap();
        let refused = [
            CreateElicitationRequest::new(
                ElicitationUrlMode::new(
                    ElicitationSessionScope::new("agent"),
                    "e",
                    "https://example.com",
                ),
                "Sign in",
            ),
            CreateElicitationRequest::new(
                ElicitationFormMode::new(ElicitationRequestScope::new(1), order()),
                "Order",
            ),
            form(ElicitationSchema::new().property("location", unknown, true)),
            form(missing),
            form(ElicitationSchema::new().boolean("rush", false)),
        ];
        for request in refused {
            let error = check_form(request).unwrap_err();
            assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);
        }
    }

    #[test]
    fn unseen_reads_how_turns_and_agents_ended() {
        let event = |kind| Event {
            session_id: "s".into(),
            seq: 1,
            recorded_at: 0,
            kind,
        };
        let turn = |stop_reason, error: Option<&str>| {
            event(EventKind::TurnEnded {
                stop_reason,
                error: error.map(String::from),
            })
        };
        let exited = |error: Option<&str>| {
            event(EventKind::AgentExited {
                error: error.map(String::from),
            })
        };
        let cases = [
            (
                "end turn",
                vec![turn(Some(StopReason::EndTurn), None)],
                Some(Unseen::Finished),
            ),
            (
                "max tokens",
                vec![turn(Some(StopReason::MaxTokens), None)],
                Some(Unseen::Finished),
            ),
            (
                "refusal",
                vec![turn(Some(StopReason::Refusal), None)],
                Some(Unseen::Finished),
            ),
            (
                "no stop reason",
                vec![turn(None, None)],
                Some(Unseen::Finished),
            ),
            (
                "cancelled",
                vec![turn(Some(StopReason::Cancelled), None)],
                None,
            ),
            (
                "error stop reason",
                vec![turn(Some(StopReason::Error(ErrorStopReason::new())), None)],
                Some(Unseen::Error),
            ),
            (
                "turn error",
                vec![turn(None, Some("boom"))],
                Some(Unseen::Error),
            ),
            (
                "agent exited with an error",
                vec![exited(Some("boom"))],
                Some(Unseen::Error),
            ),
            ("agent exited", vec![exited(None)], None),
            (
                "error after a finish",
                vec![turn(Some(StopReason::EndTurn), None), exited(Some("boom"))],
                Some(Unseen::Error),
            ),
            (
                "parked",
                vec![turn(None, Some("parked")), event(EventKind::AgentParked)],
                None,
            ),
            ("nothing ended", vec![event(EventKind::TurnStarted)], None),
        ];
        for (name, events, expected) in cases {
            assert_eq!(unseen(&events), expected, "{name}");
        }
    }
}
