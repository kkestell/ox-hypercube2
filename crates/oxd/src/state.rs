//! A session's state machine and the event history it writes.

use std::sync::Arc;

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v2::schema::StopReason;
use agent_client_protocol::v2::schema::{
    AvailableCommand, BackgroundWork, CancelledPermissionOutcome, ContentBlock,
    CreateElicitationRequest, ElicitationAction, ElicitationMode, ElicitationPropertySchema,
    ElicitationSchema, ElicitationScope, EmbeddedResourceResource, PermissionOption,
    PromptCapabilities, RequestPermissionOutcome, RequestPermissionRequest,
    SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionUpdate, StateUpdate, TextContent, ToolCallId,
};
use ox_protocol::{
    ConfigSetting, ConfigValue, ErrorBody, Event, EventKind, QueuedPrompt, Sender, SessionRequest,
    SessionState, Unseen, code,
};
use tokio::sync::oneshot;

use crate::store::{LeftOpen, RowChange, SavedSession};
use crate::worktree::Worktree;

/// What an agent reports when its ACP session opens, with its available
/// commands kept current.
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
                Change::PermissionRequested { .. }
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

/// Everything about a session that changes: its phase, its queue, the seq
/// of its last event, and the rest of what the database saves for it. Its
/// events are stored, not held: each transition records its events in
/// `unlogged` for the database. `SessionClosed` ends a session's
/// subscriptions and is not stored. Each
/// `PromptStarted` and `TurnStarted` is followed by one `TurnEnded`, each
/// `PromptSteered` by one `SteeredPromptTaken` or `SteeredPromptDropped`,
/// each `PermissionRequested` by one `PermissionResolved`, and each
/// `ElicitationRequested` by one `ElicitationResolved`.
///
/// Each transition that changes what the database saves for the session
/// records the change in `unsaved`, as it records its events in `unlogged`.
pub struct State {
    session_id: String,
    phase: Phase,
    /// The title from the last recorded `session_info_update` that set or
    /// cleared it, or from `set_title`, whichever came last.
    title: Option<String>,
    last_seq: u64,
    /// Events recorded since `take_unlogged`, which the database has not been
    /// sent. `SessionClosed` is not stored.
    unlogged: Vec<Event>,
    /// The events of the change that closed the session, ending with
    /// `SessionClosed`. A deleted session's history is gone before it
    /// closes, so these are kept for its subscribers.
    closing: Vec<Event>,
    /// Whether the session's events hold neither an `AgentStarted` nor a
    /// `PromptStarted`, so that a load asks the agent to replay them.
    needs_replay: bool,
    /// The error of the last `AgentExited` recorded.
    exit_error: Option<String>,
    /// The ID of the next permission request or elicitation. Request IDs
    /// stay unique for the session's lifetime, across every agent it loads
    /// and every restart: each is at most the seq of the event that records
    /// it, so a restored session continues after its last seq.
    next_request: u64,
    queue: Queue,
    /// The worktree the session runs in. Unset when it could not be added.
    worktree: Option<Arc<Worktree>>,
    archived: bool,
    /// The values Ox set the agent to, in the order they are sent.
    config: Vec<ConfigSetting>,
    /// How the session's turns ended while it was out of focus.
    unseen: Option<Unseen>,
    /// The connections that focus the session.
    focused: usize,
    /// The subscriptions to the session's events.
    viewers: usize,
    /// Whether a branch move is under way in the session's checkout.
    moving: bool,
    /// Changes to the session's saved data since `take_unsaved`.
    unsaved: Vec<RowChange>,
}

#[derive(Debug)]
pub struct Resume {
    pub session_id: String,
    pub replay: bool,
}

/// The config an agent's reported options amount to: the current values of
/// its model, thought level and mode select options, by category, as
/// `model`, `effort` and `mode`, in that order.
pub fn reported_config(options: &[SessionConfigOption]) -> Vec<ConfigSetting> {
    [
        (SessionConfigOptionCategory::Model, "model"),
        (SessionConfigOptionCategory::ThoughtLevel, "effort"),
        (SessionConfigOptionCategory::Mode, "mode"),
    ]
    .into_iter()
    .filter_map(|(category, config_id)| {
        options.iter().find_map(|option| match &option.kind {
            SessionConfigKind::Select(select) if option.category.as_ref() == Some(&category) => {
                Some(ConfigSetting {
                    config_id: config_id.into(),
                    value: ConfigValue::Id(select.current_value.to_string()),
                })
            }
            _ => None,
        })
    })
    .collect()
}

impl State {
    /// A session just saved: `starting`, holding its events from the first.
    pub fn created(saved: &SavedSession, worktree: Option<Arc<Worktree>>) -> Self {
        let phase = Phase::Starting {
            replay: Some(Vec::new()),
            busy: false,
            background: None,
            previous: None,
            prompt: None,
        };
        Self::of(saved, worktree, phase)
    }

    /// A session read from the database or imported: `exited`, with the
    /// saved agent session if its agent ever opened one, continuing after
    /// its last stored event.
    pub fn restored(saved: &SavedSession, worktree: Option<Arc<Worktree>>) -> Self {
        let agent = saved.agent_session_id.clone().map(Agent::saved);
        Self::of(saved, worktree, Phase::Exited(agent))
    }

    fn of(saved: &SavedSession, worktree: Option<Arc<Worktree>>, phase: Phase) -> Self {
        Self {
            session_id: saved.id.clone(),
            phase,
            title: saved.title.clone(),
            last_seq: saved.last_seq,
            unlogged: Vec::new(),
            closing: Vec::new(),
            needs_replay: saved.needs_replay,
            exit_error: None,
            next_request: saved.last_seq + 1,
            queue: saved.queue.clone(),
            worktree,
            archived: saved.archived,
            config: saved.config.clone(),
            unseen: saved.unseen,
            focused: 0,
            viewers: 0,
            moving: false,
            unsaved: Vec::new(),
        }
    }

    /// Closes what a stop left open in the session's stored events: each
    /// open request is cancelled, each steered prompt the agent had not taken
    /// in is dropped, and an open turn ends with an error, which holds the
    /// queue.
    pub fn recover(&mut self, open: LeftOpen) {
        for (request_id, permission) in open.requests {
            self.append(if permission {
                EventKind::PermissionResolved {
                    request_id,
                    outcome: CancelledPermissionOutcome::new().into(),
                }
            } else {
                EventKind::ElicitationResolved {
                    request_id,
                    action: ElicitationAction::Cancel,
                }
            });
        }
        for prompt in open.steered {
            self.append(EventKind::SteeredPromptDropped { prompt });
        }
        if open.turn {
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

    /// The changes to the session's saved data since the last call, the
    /// last of each kind, for the database.
    pub fn take_unsaved(&mut self) -> Vec<RowChange> {
        let mut unsaved: Vec<RowChange> = Vec::new();
        for change in std::mem::take(&mut self.unsaved) {
            unsaved.retain(|kept| std::mem::discriminant(kept) != std::mem::discriminant(&change));
            unsaved.push(change);
        }
        unsaved
    }

    /// Records that the queue changed.
    fn queue_changed(&mut self) {
        self.unsaved.push(RowChange::Queue(self.queue.clone()));
    }

    /// The prompts waiting to start a turn each, in order.
    pub fn queued(&self) -> &[QueuedPrompt] {
        &self.queue.prompts
    }

    fn pause(&mut self) {
        if !self.queue.paused {
            self.queue.paused = true;
            self.queue_changed();
        }
    }

    fn resume(&mut self) {
        if self.queue.paused {
            self.queue.paused = false;
            self.queue_changed();
        }
    }

    /// Starts a turn with the queue's first prompt when the agent is idle,
    /// has no steered prompt to take in, the queue is not held, and no
    /// branch move is under way in the session's checkout. Returns the
    /// prompt to send and the seq of its `PromptStarted` event.
    pub fn start_queued(&mut self) -> Option<(Vec<ContentBlock>, u64)> {
        self.refuse_moving().ok()?;
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
        self.queue_changed();
        self.append(EventKind::PromptStarted {
            prompt: prompt.clone(),
            from: None,
        });
        Some((prompt, seq))
    }

    /// Starts a load: the session goes from `exited` to `starting` and
    /// returns what to request from the agent. A session whose events hold
    /// neither an agent start nor a prompt requests and records replay to
    /// recover its conversation.
    pub fn load(&mut self) -> Result<Resume, ErrorBody> {
        self.refuse_moving()?;
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
        let replay = self.needs_replay;
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
        self.title = Some(title.clone());
        self.unsaved.push(RowChange::Title(Some(title)));
    }

    pub fn state(&self) -> SessionState {
        self.phase.state()
    }

    /// What the agent reported when it started, with its current commands.
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

    /// The error of the last `AgentExited` the session recorded.
    pub fn exit_error(&self) -> Option<&str> {
        self.exit_error.as_deref()
    }

    /// The events of the change that closed the session, ending with
    /// `SessionClosed`. Empty while the session is open.
    pub fn closing(&self) -> &[Event] {
        &self.closing
    }

    /// The events with `seq > after_seq` that were recorded and not yet
    /// taken, which in tests is every event recorded, with `SessionClosed`
    /// last.
    #[cfg(test)]
    pub fn events_after(&self, after_seq: u64) -> Vec<Event> {
        self.unlogged
            .iter()
            .chain(self.closing.last())
            .filter(|event| event.seq > after_seq)
            .cloned()
            .collect()
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
                self.queue_changed();
                return Ok(None);
            }
            (_, Request::QueueEdit(queued)) => {
                self.check_queued(&queued.prompt)?;
                let index = self.queued_index(&queued.id)?;
                self.queue.prompts[index] = queued;
                self.queue_changed();
                return Ok(None);
            }
            (_, Request::QueueRemove { id }) => {
                let index = self.queued_index(&id)?;
                self.queue.prompts.remove(index);
                self.queue_changed();
                return Ok(None);
            }
            (_, Request::QueueSend { id }) => {
                let index = self.queued_index(&id)?;
                let sent = self.send_now(self.queue.prompts[index].prompt.clone(), None, true)?;
                self.queue.prompts.remove(index);
                self.queue_changed();
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
        self.refuse_moving()?;
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
        if let (Phase::Starting { previous, .. }, Change::Started(agent)) = (&self.phase, &change) {
            if previous.as_ref().map(|previous| &previous.session_id) != Some(&agent.session_id) {
                self.unsaved
                    .push(RowChange::AgentSessionId(agent.session_id.clone()));
            }
            // A session without a config takes the one the agent reports.
            if self.config.is_empty() {
                self.config = reported_config(&agent.config_options);
                if !self.config.is_empty() {
                    self.unsaved.push(RowChange::Config(self.config.clone()));
                }
            }
        }
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

    /// Refuses a turn or a load while a branch move is under way in the
    /// session's checkout.
    pub fn refuse_moving(&self) -> Result<(), ErrorBody> {
        if self.moving {
            return Err(ErrorBody {
                code: code::INVALID_STATE,
                message: "a branch move is in progress in this checkout".into(),
            });
        }
        Ok(())
    }

    /// Marks a branch move under way in the session's checkout, which an
    /// `idle` or `exited` session allows. Another session gives its state.
    pub fn begin_move(&mut self) -> Result<(), SessionState> {
        match self.state() {
            SessionState::Idle | SessionState::Exited => {
                self.moving = true;
                Ok(())
            }
            state => Err(state),
        }
    }

    /// Ends the branch move `begin_move` marked.
    pub fn end_move(&mut self) {
        self.moving = false;
    }

    pub fn archived(&self) -> bool {
        self.archived
    }

    pub fn set_archived(&mut self, archived: bool) {
        self.archived = archived;
        self.unsaved.push(RowChange::Archived(archived));
    }

    pub fn worktree(&self) -> Option<&Arc<Worktree>> {
        self.worktree.as_ref()
    }

    /// Records that the session's worktree could not be added: the session
    /// forgets its worktree and exits with `message`.
    pub fn fail_start(&mut self, message: String) {
        if let Some(worktree) = self.worktree.take() {
            self.unsaved
                .push(RowChange::WorktreeForgotten(worktree.id.clone()));
        }
        self.record(Change::Exited(Some(message)));
    }

    /// The values Ox set the agent to, in the order they are sent.
    pub fn config(&self) -> &[ConfigSetting] {
        &self.config
    }

    /// Records the agent's answers to the settings a client sent: each
    /// accepted setting replaces the one with its ID, or is added, and the
    /// other settings keep their values. An accepted `model` sent without an
    /// `effort` removes the session's effort, because a model without effort
    /// levels takes none. Returns the error naming each refused setting.
    pub fn settle_config(
        &mut self,
        answers: Vec<(ConfigSetting, Option<String>)>,
    ) -> Result<(), String> {
        let refusals = refusals(&answers);
        let sets_effort = answers
            .iter()
            .any(|(setting, _)| setting.config_id == "effort");
        for (setting, refusal) in answers {
            if refusal.is_some() {
                continue;
            }
            if setting.config_id == "model" && !sets_effort {
                self.config.retain(|current| current.config_id != "effort");
            }
            match self
                .config
                .iter_mut()
                .find(|current| current.config_id == setting.config_id)
            {
                Some(current) => *current = setting,
                None => self.config.push(setting),
            }
        }
        self.unsaved.push(RowChange::Config(self.config.clone()));
        if refusals.is_empty() {
            Ok(())
        } else {
            Err(refusals.join("\n"))
        }
    }

    pub fn unseen(&self) -> Option<Unseen> {
        self.unseen
    }

    /// Records the unseen outcome `events` give a session out of focus. An
    /// error replaces a finish but not the reverse.
    pub fn note_unseen(&mut self, events: &[Event]) {
        if self.focused > 0 {
            return;
        }
        let unseen = match unseen(events) {
            Some(Unseen::Error) => Unseen::Error,
            Some(Unseen::Finished) if self.unseen.is_none() => Unseen::Finished,
            _ => return,
        };
        if self.unseen != Some(unseen) {
            self.unseen = Some(unseen);
            self.unsaved.push(RowChange::Unseen(Some(unseen)));
        }
    }

    /// Counts a connection's focus on the session, which clears its unseen
    /// outcome.
    pub fn focus(&mut self) {
        self.focused += 1;
        if self.unseen.take().is_some() {
            self.unsaved.push(RowChange::Unseen(None));
        }
    }

    pub fn unfocus(&mut self) {
        self.focused -= 1;
    }

    /// Counts a subscription to the session's events.
    pub fn view(&mut self) {
        self.viewers += 1;
    }

    pub fn unview(&mut self) {
        self.viewers -= 1;
    }

    /// Whether the session may be parked: quiet and unviewed.
    pub fn parkable(&self) -> bool {
        self.quiet() && self.viewers == 0
    }

    /// Whether the session is closed, as a deleted session is.
    pub fn closed(&self) -> bool {
        matches!(self.phase, Phase::Closed)
    }

    fn append(&mut self, kind: EventKind) {
        if let EventKind::Update(SessionUpdate::SessionInfoUpdate(update)) = &kind {
            let title = match &update.title {
                MaybeUndefined::Value(title) => Some(Some(title.clone())),
                MaybeUndefined::Null => Some(None),
                MaybeUndefined::Undefined => None,
            };
            if let Some(title) = title
                && title != self.title
            {
                self.title.clone_from(&title);
                self.unsaved.push(RowChange::Title(title));
            }
        }
        match &kind {
            EventKind::AgentStarted { .. } | EventKind::PromptStarted { .. } => {
                self.needs_replay = false;
            }
            EventKind::AgentExited { error } => self.exit_error.clone_from(error),
            _ => {}
        }
        self.last_seq += 1;
        let event = Event {
            session_id: self.session_id.clone(),
            seq: self.last_seq,
            recorded_at: crate::daemon::now(),
            kind,
        };
        if matches!(event.kind, EventKind::SessionClosed) {
            // `unlogged` holds the closing change's events, which `apply`
            // takes after each change.
            self.closing.clone_from(&self.unlogged);
            self.closing.push(event);
        } else {
            self.unlogged.push(event);
        }
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

/// The refused settings of `answers`, each as `The agent refused <id>
/// <value>: <error>`.
pub fn refusals(answers: &[(ConfigSetting, Option<String>)]) -> Vec<String> {
    answers
        .iter()
        .filter_map(|(setting, refusal)| {
            let value = match &setting.value {
                ConfigValue::Id(id) => id.clone(),
                ConfigValue::Boolean(value) => value.to_string(),
            };
            refusal
                .as_ref()
                .map(|error| format!("The agent refused {} {value}: {error}", setting.config_id))
        })
        .collect()
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
        ContentChunk, ElicitationAcceptAction, ElicitationContentValue, ElicitationFormMode,
        ElicitationRequestScope, ElicitationSessionScope, ElicitationUrlMode, EnumOption,
        ErrorStopReason, IdleStateUpdate, MultiSelectPropertySchema,
        OtherElicitationPropertySchema, PermissionOptionKind, RunningStateUpdate,
        SessionConfigSelectOption, StopReason, StringPropertySchema, TextContent,
    };
    use serde_json::json;

    use super::*;

    /// A session as `add_session` saves it.
    fn saved() -> SavedSession {
        SavedSession {
            number: 1,
            id: "s".into(),
            workspace_id: "w".into(),
            command: vec!["agent".into()],
            cwd: "/work".into(),
            title: None,
            agent_session_id: None,
            worktree_id: None,
            last_seq: 0,
            archived: false,
            created_at: 0,
            unseen: None,
            queue: Queue::default(),
            config: Vec::new(),
            needs_replay: true,
        }
    }

    fn created() -> State {
        State::created(&saved(), None)
    }

    fn started() -> State {
        let mut state = created();
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
        let mut state = created();
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
    fn a_first_prompt_is_recorded_while_starting_and_runs_once_started() {
        let mut state = created();
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
            let mut state = created();
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
        let mut state = created();
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
        let mut state = restored_at(3);
        state.recover(LeftOpen {
            turn: true,
            requests: Vec::new(),
            steered: vec![3],
        });
        assert_eq!(
            state.events_after(3)[0].kind,
            EventKind::SteeredPromptDropped { prompt: 3 }
        );
        assert_eq!(kinds(&state, 4), ["turn_ended"]);
        assert!(state.take_unsaved().contains(&RowChange::Queue(Queue {
            prompts: Vec::new(),
            paused: true,
        })));
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
    fn nothing_follows_session_closed() {
        let mut state = started();
        prompt(&mut state).unwrap();
        state.record(Change::Closed);
        let closed = state.last_seq();
        let (answer, _outcome) = oneshot::channel();
        for change in [
            chunk(),
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

    #[test]
    fn reported_config_reads_model_effort_and_mode_by_category() {
        let select = |id: &str, value: &str, category: Option<SessionConfigOptionCategory>| {
            SessionConfigOption::select(
                id.to_owned(),
                id.to_owned(),
                value.to_owned(),
                vec![SessionConfigSelectOption::new(
                    value.to_owned(),
                    value.to_owned(),
                )],
            )
            .category(category)
        };
        let options = [
            select("mode", "code", Some(SessionConfigOptionCategory::Mode)),
            select("speed", "fast", None),
            select(
                "thinking",
                "high",
                Some(SessionConfigOptionCategory::ThoughtLevel),
            ),
            SessionConfigOption::boolean("fast", "Fast", true),
            select(
                "model",
                "openai/one",
                Some(SessionConfigOptionCategory::Model),
            ),
        ];
        let setting = |config_id: &str, value: &str| ConfigSetting {
            config_id: config_id.into(),
            value: ConfigValue::Id(value.into()),
        };
        assert_eq!(
            reported_config(&options),
            [
                setting("model", "openai/one"),
                setting("effort", "high"),
                setting("mode", "code"),
            ]
        );
        assert_eq!(reported_config(&options[..2]), [setting("mode", "code")]);
        assert!(reported_config(&[]).is_empty());
    }

    fn replayed(text: &str) -> Change {
        Change::Update(SessionUpdate::UserMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
            "message",
        )))
    }

    fn restored() -> State {
        State::restored(
            &SavedSession {
                title: Some("Fix the build".into()),
                ..saved_with_agent(0)
            },
            None,
        )
    }

    /// A session restored with agent session `agent` and stored events that
    /// end at `last_seq`.
    fn restored_at(last_seq: u64) -> State {
        State::restored(&saved_with_agent(last_seq), None)
    }

    fn saved_with_agent(last_seq: u64) -> SavedSession {
        SavedSession {
            agent_session_id: Some("agent".into()),
            last_seq,
            ..saved()
        }
    }

    #[test]
    fn closing_a_restored_session_records_session_closed_after_its_last_seq() {
        let mut state = restored_at(3);
        state.subscribe(3).unwrap();
        state.record(Change::Closed);
        assert_eq!(state.last_seq(), 4);
        assert_eq!(state.closing().last().unwrap().seq, 4);
        assert_eq!(kinds(&state, 3), ["session_closed"]);
        assert!(state.take_unlogged().is_empty());
        state.subscribe(3).unwrap();
        assert_eq!(state.subscribe(4).unwrap_err().code, code::INVALID_REQUEST);
    }

    #[test]
    fn recover_closes_the_turn_and_the_requests_a_stop_left_open() {
        let mut state = restored_at(6);
        state.recover(LeftOpen {
            turn: true,
            requests: vec![("2".into(), true), ("5".into(), false)],
            steered: Vec::new(),
        });
        assert_eq!(
            kinds(&state, 6),
            ["permission_resolved", "elicitation_resolved", "turn_ended"]
        );
        let logged = state.take_unlogged();
        assert_eq!(logged.iter().map(|e| e.seq).collect::<Vec<_>>(), [7, 8, 9]);
        assert!(matches!(
            &logged[0].kind,
            EventKind::PermissionResolved {
                request_id,
                outcome: RequestPermissionOutcome::Cancelled(_),
            } if request_id == "2"
        ));
        assert_eq!(
            logged[1].kind,
            EventKind::ElicitationResolved {
                request_id: "5".into(),
                action: ElicitationAction::Cancel,
            }
        );
        assert!(matches!(
            &logged[2].kind,
            EventKind::TurnEnded { error: Some(error), .. } if error == "oxd stopped during the turn"
        ));
        assert_eq!(
            state.take_unsaved(),
            [RowChange::Queue(Queue {
                prompts: Vec::new(),
                paused: true,
            })]
        );
    }

    #[test]
    fn recover_with_nothing_open_records_nothing() {
        let mut state = restored_at(4);
        state.recover(LeftOpen::default());
        assert_eq!(state.last_seq(), 4);
        assert!(state.take_unlogged().is_empty());
        assert!(state.take_unsaved().is_empty());
    }

    #[test]
    fn request_ids_of_a_restored_session_continue_after_its_last_seq() {
        let mut state = restored_at(5);
        state.load().unwrap();
        state.record(Change::Started(Agent::saved("agent".into())));
        state.record(running());
        let _outcome = request_permission(&mut state);
        let last = state.events_after(state.last_seq() - 1);
        assert!(matches!(
            &last[0].kind,
            EventKind::PermissionRequested { request_id, .. } if request_id == "6"
        ));
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

        // Its events now hold an agent start, so the next load replays
        // nothing.
        state.record(Change::Exited(None));
        assert!(!state.load().unwrap().replay);
    }

    #[test]
    fn a_restored_session_whose_events_hold_a_prompt_loads_without_replay() {
        let mut state = State::restored(
            &SavedSession {
                needs_replay: false,
                ..saved_with_agent(2)
            },
            None,
        );
        let resume = state.load().unwrap();
        assert!(!resume.replay);
        assert_eq!(resume.session_id, "agent");
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
    fn a_restored_session_that_never_started_cannot_load() {
        let mut state = State::restored(
            &SavedSession {
                last_seq: 3,
                ..saved()
            },
            None,
        );
        assert_eq!(state.state(), SessionState::Exited);
        let refused = state.load().unwrap_err();
        assert_eq!(refused.code, code::INVALID_STATE);
        assert_eq!(
            refused.message,
            "the session never started, so it has nothing to load"
        );
    }

    #[test]
    fn a_moving_session_refuses_turns_and_loads_and_holds_its_queue() {
        let moved = |refused: ErrorBody| {
            assert_eq!(refused.code, code::INVALID_STATE);
            assert_eq!(
                refused.message,
                "a branch move is in progress in this checkout"
            );
        };
        let mut state = started();
        state.begin_move().unwrap();
        moved(prompt(&mut state).unwrap_err());
        moved(message(&mut state, "hello").unwrap_err());
        enqueue(&mut state, "first");
        assert_eq!(start_queued(&mut state), None);
        moved(
            state
                .request(Request::QueueSend { id: "first".into() })
                .unwrap_err(),
        );
        assert_eq!(queued_ids(&state), ["first"]);
        state.end_move();
        assert_eq!(start_queued(&mut state).as_deref(), Some("first"));

        let mut state = started();
        state.record(Change::Exited(None));
        state.begin_move().unwrap();
        moved(state.load().unwrap_err());
        state.end_move();
        state.load().unwrap();
    }

    fn model(value: &str) -> ConfigSetting {
        ConfigSetting {
            config_id: "model".into(),
            value: ConfigValue::Id(value.into()),
        }
    }

    #[test]
    fn a_settled_config_sets_only_the_settings_the_agent_accepted() {
        let effort = |value: &str| ConfigSetting {
            config_id: "effort".into(),
            value: ConfigValue::Id(value.into()),
        };
        let mode = |value: &str| ConfigSetting {
            config_id: "mode".into(),
            value: ConfigValue::Id(value.into()),
        };
        let mut state = State::created(
            &SavedSession {
                config: vec![model("fast"), effort("low"), mode("code")],
                ..saved()
            },
            None,
        );
        state.settle_config(vec![(mode("plan"), None)]).unwrap();
        assert_eq!(state.config(), [model("fast"), effort("low"), mode("plan")]);
        assert_eq!(
            state.take_unsaved(),
            [RowChange::Config(vec![
                model("fast"),
                effort("low"),
                mode("plan")
            ])]
        );

        state
            .settle_config(vec![(model("smart"), None), (effort("high"), None)])
            .unwrap();
        assert_eq!(
            state.config(),
            [model("smart"), effort("high"), mode("plan")]
        );

        let refused = state
            .settle_config(vec![
                (model("fast"), None),
                (effort("max"), Some("no".into())),
            ])
            .unwrap_err();
        assert_eq!(refused, "The agent refused effort max: no");
        assert_eq!(
            state.config(),
            [model("fast"), effort("high"), mode("plan")]
        );

        state.settle_config(vec![(model("tiny"), None)]).unwrap();
        assert_eq!(state.config(), [model("tiny"), mode("plan")]);
    }
}
