//! Workflow runs: a run's record, and the runner that advances its steps:
//! agent steps in child sessions, shell steps and checks as commands.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use agent_client_protocol::v2::schema::{
    ContentBlock, MessageId, SessionConfigKind, SessionConfigOptionCategory, SessionUpdate,
    StopReason, TextContent,
};
use ox_protocol::{
    CommandInfo, CommandKind, CommandState, ConfigValue, ErrorBody, Event, EventKind,
    MaybeUndefined, RunInfo, RunState, StepInfo, StepState, code,
};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::daemon::{Daemon, now};
use crate::session::Session;
use crate::shell::{self, Finished, Inputs};
use crate::store::{SavedRun, Store};
use crate::workflow::{Action, AgentStep, Check, Step, Workflow};
use crate::worktree::{self, Git, Worktree, WorktreePlan};

/// A workflow run: its saved record, the worktree it owns, and, while it is
/// running, the runner that advances its steps.
pub struct Run {
    pub id: String,
    pub number: u64,
    pub workspace_id: String,
    workflow: String,
    name: String,
    input: String,
    /// When the run was started, in milliseconds since the Unix epoch.
    created_at: u64,
    pub worktree: Arc<Worktree>,
    record: Mutex<RunRecord>,
    store: Arc<Store>,
    /// Counts the changes to the run's `RunInfo`. Changed under the record
    /// lock.
    version: AtomicU64,
    /// The daemon's signal that the lists changed, sent when this run's
    /// `RunInfo` changes.
    changed: watch::Sender<()>,
    /// Set when the run is cancelled.
    cancel: watch::Sender<bool>,
    runner: Mutex<Option<JoinHandle<()>>>,
}

/// What changes as a run advances.
struct RunRecord {
    state: RunState,
    error: Option<String>,
    steps: Vec<StepInfo>,
}

/// Why a step stopped before it succeeded.
enum Ended {
    /// The step failed with `error`. `output` is the reply of a repair turn
    /// after which a postcondition still failed.
    Failed {
        error: String,
        output: Option<String>,
    },
    Cancelled,
}

/// A step that failed with `error` and no output.
fn failed(error: impl Into<String>) -> Ended {
    Ended::Failed {
        error: error.into(),
        output: None,
    }
}

impl Run {
    /// The live run for `saved`, whose worktree is `worktree`. A running run
    /// advances once `spawn` starts its runner.
    pub fn new(
        saved: SavedRun,
        worktree: Arc<Worktree>,
        store: Arc<Store>,
        changed: watch::Sender<()>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: saved.id,
            number: saved.number,
            workspace_id: saved.workspace_id,
            workflow: saved.workflow,
            name: saved.name,
            input: saved.input,
            created_at: saved.created_at,
            worktree,
            record: Mutex::new(RunRecord {
                state: saved.state,
                error: saved.error,
                steps: saved.steps,
            }),
            store,
            version: AtomicU64::new(0),
            changed,
            cancel: watch::Sender::new(false),
            runner: Mutex::new(None),
        })
    }

    /// Starts the runner, which adds the worktree `plan` describes with
    /// `git`, then runs `workflow`'s steps in it, their agents and commands
    /// with `env`.
    pub fn spawn(
        self: &Arc<Self>,
        daemon: Arc<Daemon>,
        workflow: Workflow,
        git: Git,
        plan: WorktreePlan,
        env: BTreeMap<String, String>,
    ) {
        let runner = tokio::spawn(advance(self.clone(), daemon, workflow, git, plan, env));
        *self.runner.lock().unwrap() = Some(runner);
    }

    pub fn info(&self) -> RunInfo {
        self.info_of(&self.lock())
    }

    /// The run's `RunInfo` and its version, which grows with each change.
    pub fn versioned_info(&self) -> (RunInfo, u64) {
        let record = self.lock();
        (self.info_of(&record), self.version())
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn running(&self) -> bool {
        self.lock().state == RunState::Running
    }

    /// The IDs of the run's child sessions.
    pub fn session_ids(&self) -> Vec<String> {
        self.lock()
            .steps
            .iter()
            .filter_map(|step| step.session_id.clone())
            .collect()
    }

    /// Clears the session of the step that ran in session `id`, which was
    /// deleted. The database clears it with the session's row.
    pub fn forget_session(&self, id: &str) {
        self.update(|record| {
            for step in &mut record.steps {
                if step.session_id.as_deref() == Some(id) {
                    step.session_id = None;
                }
            }
        });
    }

    /// Asks the runner to stop. Refused for a run that is not running.
    pub fn cancel(&self) -> Result<(), ErrorBody> {
        if !self.running() {
            return Err(ErrorBody {
                code: code::INVALID_STATE,
                message: "the run is not running".into(),
            });
        }
        self.cancel.send_replace(true);
        Ok(())
    }

    /// Stops the runner, leaving the run as it is.
    pub fn close(&self) {
        let runner = self.runner.lock().unwrap().take();
        if let Some(runner) = runner {
            runner.abort();
        }
    }

    fn lock(&self) -> MutexGuard<'_, RunRecord> {
        self.record.lock().unwrap()
    }

    fn info_of(&self, record: &RunRecord) -> RunInfo {
        RunInfo {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            workflow: self.workflow.clone(),
            name: self.name.clone(),
            input: self.input.clone(),
            worktree_id: self.worktree.id.clone(),
            state: record.state,
            error: record.error.clone(),
            steps: record.steps.clone(),
            created_at: self.created_at,
        }
    }

    /// Changes the record and signals the change.
    fn update(&self, change: impl FnOnce(&mut RunRecord)) {
        let mut record = self.lock();
        change(&mut record);
        self.version.fetch_add(1, Ordering::Release);
        self.changed.send_replace(());
    }

    /// Records that step `position` runs in session `session_id`, which the
    /// database saved with the session.
    pub fn set_step_session(&self, position: usize, session_id: &str) {
        self.update(|record| record.steps[position].session_id = Some(session_id.into()));
    }

    /// Saves and records that step `position` is running, and when it
    /// started.
    fn start_step(&self, position: usize) {
        let started_at = now();
        if let Err(error) = self.store.start_step(&self.id, position, started_at) {
            eprintln!("oxd: saving step {position} of run {}: {error}", self.id);
        }
        self.update(|record| {
            let step = &mut record.steps[position];
            step.state = StepState::Running;
            step.started_at = Some(started_at);
        });
    }

    /// Saves and records step `position`'s success and output.
    fn succeed_step(&self, position: usize, output: String) {
        if let Err(error) =
            self.store
                .set_step(&self.id, position, StepState::Succeeded, Some(&output))
        {
            eprintln!("oxd: saving step {position} of run {}: {error}", self.id);
        }
        self.update(|record| {
            let step = &mut record.steps[position];
            step.state = StepState::Succeeded;
            step.output = Some(output);
        });
    }

    /// Ends the run in `state` with `error`, and step `position`, if one was
    /// running, in `step_state` with `output`.
    fn end(
        &self,
        state: RunState,
        error: Option<String>,
        step: Option<(usize, StepState, Option<String>)>,
    ) {
        if let Some((position, step_state, output)) = &step
            && let Err(error) =
                self.store
                    .set_step(&self.id, *position, *step_state, output.as_deref())
        {
            eprintln!("oxd: saving step {position} of run {}: {error}", self.id);
        }
        if let Err(error) = self.store.set_run_state(&self.id, state, error.as_deref()) {
            eprintln!("oxd: saving run {}: {error}", self.id);
        }
        self.update(|record| {
            if let Some((position, step_state, output)) = step {
                record.steps[position].state = step_state;
                record.steps[position].output = output;
            }
            record.state = state;
            record.error = error;
        });
    }

    /// Records that step `step` started `command`, and returns the command's
    /// index among the step's commands.
    fn start_command(&self, step: usize, kind: CommandKind, command: &str) -> usize {
        let info = CommandInfo {
            kind,
            command: command.into(),
            state: CommandState::Running,
            exit_code: None,
            output: String::new(),
            truncated: false,
        };
        let index = self.lock().steps[step].commands.len();
        self.save_command(step, index, &info);
        self.update(|record| record.steps[step].commands.push(info));
        index
    }

    /// Records how command `index` of step `step` ended.
    fn finish_command(
        &self,
        step: usize,
        index: usize,
        state: CommandState,
        exit_code: Option<i32>,
        output: String,
        truncated: bool,
    ) {
        let mut info = self.lock().steps[step].commands[index].clone();
        info.state = state;
        info.exit_code = exit_code;
        info.output = output;
        info.truncated = truncated;
        self.save_command(step, index, &info);
        self.update(|record| record.steps[step].commands[index] = info);
    }

    fn save_command(&self, step: usize, index: usize, info: &CommandInfo) {
        if let Err(error) = self.store.save_command(&self.id, step, index, info) {
            eprintln!(
                "oxd: saving command {index} of step {step} of run {}: {error}",
                self.id
            );
        }
    }

    fn cancelled(&self) -> bool {
        *self.cancel.borrow()
    }
}

/// Adds the run's worktree, then runs each step in it, giving each the
/// previous step's output, or the run's input, until a step fails, the run
/// is cancelled, or every step has succeeded.
async fn advance(
    run: Arc<Run>,
    daemon: Arc<Daemon>,
    workflow: Workflow,
    git: Git,
    plan: WorktreePlan,
    env: BTreeMap<String, String>,
) {
    if let Err(mut message) = worktree::add(&git, &plan).await {
        if let Err(error) = worktree::roll_back(&git, &plan, true).await {
            message = format!("{message}; undoing it failed: {error}");
        }
        run.end(RunState::Failed, Some(message), None);
        return;
    }
    let mut cancel = run.cancel.subscribe();
    let mut input = run.input.clone();
    for (position, step) in workflow.steps.iter().enumerate() {
        if run.cancelled() {
            run.end(RunState::Cancelled, None, None);
            return;
        }
        run.start_step(position);
        let context = StepContext {
            run: &run,
            daemon: &daemon,
            position,
            cwd: &plan.cwd,
            env: &env,
        };
        match context.run(step, &input, &mut cancel).await {
            Ok(output) => {
                run.succeed_step(position, output.clone());
                input = output;
            }
            Err(Ended::Failed { error, output }) => {
                run.end(
                    RunState::Failed,
                    Some(error),
                    Some((position, StepState::Failed, output)),
                );
                return;
            }
            Err(Ended::Cancelled) => {
                run.end(
                    RunState::Cancelled,
                    None,
                    Some((position, StepState::Cancelled, None)),
                );
                return;
            }
        }
    }
    run.end(RunState::Succeeded, None, None);
}

/// What a step runs with.
struct StepContext<'a> {
    run: &'a Arc<Run>,
    daemon: &'a Arc<Daemon>,
    position: usize,
    /// The directory the step's agent and commands run in.
    cwd: &'a Path,
    env: &'a BTreeMap<String, String>,
}

impl StepContext<'_> {
    /// Runs the step's preconditions, its action, and its postconditions,
    /// with `input`, and returns the step's output. An agent step whose
    /// postcondition fails gets one repair turn, after which the
    /// postconditions run again.
    async fn run(
        &self,
        step: &Step,
        input: &str,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<String, Ended> {
        if let Some((_, finished)) = self
            .checks(
                CommandKind::Precondition,
                &step.preconditions,
                input,
                cancel,
            )
            .await?
        {
            return Err(failed(format!("a precondition {}", outcome(&finished))));
        }
        match &step.action {
            Action::Shell { command } => {
                let finished = self
                    .command(CommandKind::Shell, command, input, cancel)
                    .await?;
                if !finished.succeeded() {
                    return Err(failed(format!("the command {}", outcome(&finished))));
                }
                if let Some((_, failure)) = self
                    .checks(
                        CommandKind::Postcondition,
                        &step.postconditions,
                        input,
                        cancel,
                    )
                    .await?
                {
                    return Err(failed(format!("a postcondition {}", outcome(&failure))));
                }
                Ok(finished.stdout)
            }
            Action::Agent { agent, prompt } => {
                let session = self.start_agent(agent, cancel).await?;
                let mut blocks = vec![ContentBlock::Text(TextContent::new(prompt.clone()))];
                if !input.is_empty() {
                    blocks.push(ContentBlock::Text(TextContent::new(input)));
                }
                let reply = turn(&session, blocks, cancel).await?;
                let Some((check, failure)) = self
                    .checks(
                        CommandKind::Postcondition,
                        &step.postconditions,
                        input,
                        cancel,
                    )
                    .await?
                else {
                    return Ok(reply);
                };
                let repair = ContentBlock::Text(TextContent::new(repair_prompt(check, &failure)));
                let reply = turn(&session, vec![repair], cancel).await?;
                match self
                    .checks(
                        CommandKind::Postcondition,
                        &step.postconditions,
                        input,
                        cancel,
                    )
                    .await?
                {
                    None => Ok(reply),
                    Some((_, failure)) => Err(Ended::Failed {
                        error: format!(
                            "a postcondition {} after the repair turn",
                            outcome(&failure)
                        ),
                        output: Some(reply),
                    }),
                }
            }
        }
    }

    /// Runs `checks` in order until one fails, and returns that one with
    /// how it ended.
    async fn checks<'c>(
        &self,
        kind: CommandKind,
        checks: &'c [Check],
        input: &str,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Option<(&'c Check, Finished)>, Ended> {
        for check in checks {
            let finished = self.command(kind, &check.shell, input, cancel).await?;
            if !finished.succeeded() {
                return Ok(Some((check, finished)));
            }
        }
        Ok(None)
    }

    /// Runs `command` as one of the step's commands, with `input` as the
    /// step's input, and records it. Cancelling the run stops it.
    async fn command(
        &self,
        kind: CommandKind,
        command: &str,
        input: &str,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Finished, Ended> {
        let index = self.run.start_command(self.position, kind, command);
        let inputs = Inputs {
            run: &self.run.input,
            step: input,
        };
        let stop = async {
            let _ = cancel.wait_for(|cancelled| *cancelled).await;
        };
        match shell::run(command, self.cwd, self.env, inputs, stop).await {
            Ok(finished) => {
                let state = if finished.succeeded() {
                    CommandState::Succeeded
                } else {
                    CommandState::Failed
                };
                self.run.finish_command(
                    self.position,
                    index,
                    state,
                    finished.exit_code(),
                    finished.output.clone(),
                    finished.truncated,
                );
                Ok(finished)
            }
            Err(stopped) => {
                self.run.finish_command(
                    self.position,
                    index,
                    CommandState::Cancelled,
                    None,
                    stopped.output,
                    stopped.truncated,
                );
                Err(Ended::Cancelled)
            }
        }
    }

    /// Starts the step's session, waits for its agent, and sets its config
    /// options.
    async fn start_agent(
        &self,
        agent: &AgentStep,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Arc<Session>, Ended> {
        let session = self
            .daemon
            .add_step_session(
                self.run,
                self.position,
                agent.harness,
                self.cwd,
                self.env.clone(),
            )
            .map_err(|error| failed(format!("starting the agent: {}", error.message)))?;
        let mut cursor = 0;
        'starting: loop {
            for event in next_events(&session, &mut cursor, cancel).await? {
                match event.kind {
                    EventKind::AgentStarted { .. } => break 'starting,
                    EventKind::AgentExited { error } => {
                        let error = error.unwrap_or_else(|| "it exited".into());
                        return Err(failed(format!("the agent did not start: {error}")));
                    }
                    _ => {}
                }
            }
        }
        let sets_options = [&agent.model, &agent.effort, &agent.permission_mode]
            .iter()
            .any(|value| value.is_some());
        if sets_options && session.info().config_options.is_empty() {
            // Pi's bridge reports its options in an update once Pi starts,
            // after the session opens.
            // TODO: An agent that never reports options leaves the step
            // waiting until the run is cancelled.
            'reported: loop {
                for event in next_events(&session, &mut cursor, cancel).await? {
                    if let EventKind::Update(SessionUpdate::ConfigOptionUpdate(_)) = event.kind {
                        break 'reported;
                    }
                }
            }
        }
        configure(&session, agent, cancel).await?;
        Ok(session)
    }
}

/// How a command ended, as a run's error gives it.
fn outcome(finished: &Finished) -> String {
    match finished.exit {
        None => format!("could not start: {}", finished.output),
        Some(_) => finished.result(),
    }
}

/// The prompt of the repair turn after postcondition `check` failed.
fn repair_prompt(check: &Check, failure: &Finished) -> String {
    let mut prompt = format!(
        "A postcondition of this step failed after your turn. Fix what it checks so that it \
        passes, or reply with the reason you cannot.\n\nCommand:\n{}\n\nResult: {}\n",
        check.shell.trim_end(),
        failure.result()
    );
    let output = failure.output.trim_end();
    if !output.is_empty() {
        let _ = write!(prompt, "\nOutput:\n{output}\n");
    }
    prompt
}

/// Prompts the session with `prompt` and returns the turn's final reply.
/// Cancelling the run cancels the turn.
async fn turn(
    session: &Session,
    prompt: Vec<ContentBlock>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String, Ended> {
    let seq = session
        .prompt(prompt)
        .map_err(|error| failed(format!("prompting the agent: {}", error.message)))?;
    match final_reply(session, seq, cancel).await {
        Err(Ended::Cancelled) => {
            session.cancel();
            Err(Ended::Cancelled)
        }
        result => result,
    }
}

/// Sets each config option the step gives a value, found by its category,
/// that the agent does not already have at that value.
async fn configure(
    session: &Session,
    agent: &AgentStep,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), Ended> {
    let settings = [
        (SessionConfigOptionCategory::Model, "model", &agent.model),
        (
            SessionConfigOptionCategory::ThoughtLevel,
            "effort",
            &agent.effort,
        ),
        (
            SessionConfigOptionCategory::Mode,
            "permission mode",
            &agent.permission_mode,
        ),
    ];
    for (category, label, value) in settings {
        let Some(value) = value else {
            continue;
        };
        // Setting one option can change the others, so each is looked up
        // in the agent's latest options.
        let options = session.info().config_options;
        let Some((config_id, current)) = options.iter().find_map(|option| {
            let SessionConfigKind::Select(select) = &option.kind else {
                return None;
            };
            (option.category.as_ref() == Some(&category)).then(|| {
                (
                    option.config_id.to_string(),
                    select.current_value.to_string(),
                )
            })
        }) else {
            return Err(failed(format!("the agent has no {label} option")));
        };
        if current == *value {
            continue;
        }
        tokio::select! {
            set = session.set_config_option(config_id, ConfigValue::Id(value.clone())) => {
                set.map_err(|error| {
                    failed(format!("setting the {label} to {value}: {}", error.message))
                })?;
            }
            _ = cancel.wait_for(|cancelled| *cancelled) => return Err(Ended::Cancelled),
        }
    }
    Ok(())
}

/// Waits for the turn that prompt `seq` started to end, and returns the text
/// of the last agent message it started. A turn that ends other than with
/// `end_turn` fails the step.
async fn final_reply(
    session: &Session,
    seq: u64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String, Ended> {
    let mut cursor = seq;
    // Each agent message of the turn, in the order they started.
    let mut messages: Vec<(MessageId, String)> = Vec::new();
    loop {
        for event in next_events(session, &mut cursor, cancel).await? {
            match event.kind {
                EventKind::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                    message(&mut messages, chunk.message_id).push_str(&text(&[chunk.content]));
                }
                EventKind::Update(SessionUpdate::AgentMessage(update)) => {
                    let text = match update.content {
                        MaybeUndefined::Value(content) => Some(text(&content)),
                        MaybeUndefined::Null => Some(String::new()),
                        MaybeUndefined::Undefined => None,
                    };
                    let message = message(&mut messages, update.message_id);
                    if let Some(text) = text {
                        *message = text;
                    }
                }
                EventKind::TurnEnded { stop_reason, error } => {
                    return match (stop_reason, error) {
                        (_, Some(error)) => Err(failed(error)),
                        (Some(StopReason::EndTurn), None) => {
                            Ok(messages.pop().map(|(_, text)| text).unwrap_or_default())
                        }
                        (Some(StopReason::Error(stopped)), None) => {
                            Err(failed(stopped.error.map_or_else(
                                || "the turn ended with an error".into(),
                                |error| error.message,
                            )))
                        }
                        (stop_reason, None) => Err(failed(format!(
                            "the turn ended with stop reason {}",
                            stop_reason_name(stop_reason.as_ref())
                        ))),
                    };
                }
                _ => {}
            }
        }
    }
}

/// The text of message `id`, added after the others if it is new.
fn message(messages: &mut Vec<(MessageId, String)>, id: MessageId) -> &mut String {
    let index = match messages.iter().position(|(known, _)| *known == id) {
        Some(index) => index,
        None => {
            messages.push((id, String::new()));
            messages.len() - 1
        }
    };
    &mut messages[index].1
}

fn text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

fn stop_reason_name(stop_reason: Option<&StopReason>) -> String {
    let Some(stop_reason) = stop_reason else {
        return "none".into();
    };
    serde_json::to_value(stop_reason)
        .ok()
        .and_then(|value| value["stopReason"].as_str().map(String::from))
        .unwrap_or_else(|| format!("{stop_reason:?}"))
}

/// The session's events after `cursor`, which then moves past them, unless
/// the run is cancelled first. A session that closes was deleted.
async fn next_events(
    session: &Session,
    cursor: &mut u64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Vec<Event>, Ended> {
    let events = tokio::select! {
        events = session.events_after(*cursor) => events,
        _ = cancel.wait_for(|cancelled| *cancelled) => return Err(Ended::Cancelled),
    };
    if events
        .iter()
        .any(|event| matches!(event.kind, EventKind::SessionClosed))
    {
        return Err(failed("the step's session was deleted"));
    }
    if let Some(last) = events.last() {
        *cursor = last.seq;
    }
    Ok(events)
}
