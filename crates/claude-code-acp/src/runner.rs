use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    process::Stdio,
    sync::{Arc, Mutex},
};

use crate::{commands, updates::Updates};
use acp_bridge::McpServer;
use agent_client_protocol::v2::Responder;
use agent_client_protocol::v2::schema::{
    ContentBlock, ErrorStopReason, PromptResponse, StateUpdate, StopReason,
};
use serde_json::{Value, json};
use tokio::{
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::AbortHandle,
};

#[derive(Clone, Debug)]
pub enum Event {
    /// Text from the user, which a transcript carries as a message. The
    /// client already has the prompts of live turns.
    UserText(String),
    UserImage {
        data: String,
        mime_type: String,
    },
    UserDocument {
        data: String,
        mime_type: String,
    },
    Text(String),
    Thought(String),
    Tool {
        id: Option<String>,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        is_error: bool,
        output: Vec<ToolOutput>,
    },
}

/// Part of what a tool returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolOutput {
    Text(String),
    Image { data: String, mime_type: String },
}

#[derive(Debug)]
pub struct PermissionRequest {
    pub name: String,
    pub input: Value,
    pub tool_use_id: String,
    pub suggestions: Vec<Value>,
    pub suppress_always_allow_rule: bool,
}

#[derive(Debug)]
pub enum RunError {
    Cancelled,
    Failed(String),
}

pub struct StartRequest<'a> {
    pub cwd: &'a Path,
    /// The Claude session ID, which is also the ACP session ID.
    pub session_id: &'a str,
    /// Claude has a transcript of the session to continue.
    pub resume: bool,
    pub model: &'a str,
    pub effort: &'a str,
    pub mode: &'a str,
    pub mcp_servers: &'a [McpServer],
    /// The client's system prompt, appended to Claude's.
    pub instructions: Option<&'a str>,
    /// The resumed conversation's transcript entries, from which the bridge
    /// finds the scheduled prompts Claude restores.
    pub transcript: &'a [Value],
}

#[derive(Debug)]
pub enum Finish {
    EndTurn,
    MaxTurns,
}

/// A prompt waiting for claude to take it into a turn, which inserts it into
/// the conversation.
struct Prompt {
    content: Vec<ContentBlock>,
    responder: Responder<PromptResponse>,
}

impl Prompt {
    /// Answers the prompt with the error that kept claude from taking it.
    fn fail(self, error: RunError) {
        let _ = self.responder.respond_with_error(match error {
            RunError::Cancelled => agent_client_protocol::Error::request_cancelled(),
            RunError::Failed(error) => agent_client_protocol::Error::new(-32603, error),
        });
    }
}

#[derive(Default)]
struct State {
    /// Whether claude has yet to answer the initialize request.
    initializing: bool,
    /// Prompts claude has not taken yet, by the `uuid` claude knows them by.
    prompts: HashMap<String, Prompt>,
    /// Control requests waiting for claude's response, by request ID.
    controls: HashMap<String, oneshot::Sender<Result<(), String>>>,
    /// The tasks asking the client about claude's permission requests and
    /// questions, by request ID.
    permissions: HashMap<String, AbortHandle>,
    control_counter: u64,
    /// Why claude exited, once it has.
    exited: Option<String>,
    /// The client cancelled the running turn, so the turn ends as cancelled
    /// however claude stops it.
    cancelling: bool,
}

/// A `claude -p` process that reads stream-json messages from stdin for as
/// long as the session lives. It takes each prompt as a turn of one
/// conversation, and starts turns of its own when a background task it
/// started reports back. Everything it writes becomes session updates,
/// whether or not a prompt is waiting. Dropping it closes claude's stdin,
/// which ends claude.
pub struct Claude {
    input: mpsc::UnboundedSender<Value>,
    state: Arc<Mutex<State>>,
    updates: Arc<Updates>,
}

impl Claude {
    pub fn start(request: &StartRequest<'_>, updates: Updates) -> Result<Self, String> {
        let mut command = Command::new("claude");
        command
            .arg("-p")
            .args([
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--replay-user-messages",
            ])
            .args(["--model", request.model])
            .args(["--effort", request.effort]);
        // Claude uses every tool of the client's MCP servers without asking.
        if !request.mcp_servers.is_empty() {
            command.arg("--allowedTools").args(
                request
                    .mcp_servers
                    .iter()
                    .map(|server| format!("mcp__{}", server.name)),
            );
        }
        command.args([
            "--permission-mode",
            request.mode,
            "--permission-prompt-tool",
            "stdio",
        ]);
        if let Some(instructions) = request.instructions {
            command.arg("--append-system-prompt").arg(instructions);
        }
        if request.resume {
            command.arg("--resume").arg(request.session_id);
        } else {
            command.arg("--session-id").arg(request.session_id);
        }
        if !request.mcp_servers.is_empty() {
            let servers: BTreeMap<_, _> = request
                .mcp_servers
                .iter()
                .map(|server| (server.name.clone(), server.config()))
                .collect();
            command
                .arg("--mcp-config")
                .arg(json!({"mcpServers": servers}).to_string());
        }
        let mut child = command
            .current_dir(request.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("ANTHROPIC_AUTH_TOKEN")
            // Claude reports when it starts and stops working only if asked.
            .env("CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS", "1")
            // Claude reports `idle` at a turn's end while background tasks run,
            // and `running` when the turn they start begins.
            .env("CLAUDE_CODE_BG_TASKS_REPORT_RUNNING", "0")
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to start claude: {e}"))?;
        let (input, messages) = mpsc::unbounded_channel();
        let state = Arc::new(Mutex::new(State {
            initializing: true,
            ..State::default()
        }));
        tokio::spawn(acp_bridge::pipes::write_input(
            child.stdin.take().expect("piped stdin"),
            messages,
        ));
        updates.seed(request.transcript);
        let updates = Arc::new(updates);
        let output = Output {
            updates: updates.clone(),
            state: state.clone(),
            input: input.downgrade(),
            text_id: None,
            thought_id: None,
            unclaimed_error: None,
        };
        tokio::spawn(output.read(child));
        let _ = input.send(json!({
            "type": "control_request",
            "request_id": "initialize",
            "request": {"subtype": "initialize"}
        }));
        Ok(Self {
            input,
            state,
            updates,
        })
    }

    /// What claude is doing now, for a client that resumes the session.
    pub fn state(&self) -> StateUpdate {
        self.updates.state()
    }

    pub fn commands(&self) -> Vec<agent_client_protocol::v2::schema::AvailableCommand> {
        self.updates.commands()
    }

    pub fn has_exited(&self) -> bool {
        self.state.lock().unwrap().exited.is_some()
    }

    /// Sends a prompt. Once claude takes it into a turn, which inserts it
    /// into the conversation, the session gets it as the user message
    /// `message_id` and `responder` answers with that ID. Messages reach
    /// claude in the order they are sent, so an interrupt sent after this
    /// call stops this prompt's turn or drops the prompt. If claude has
    /// exited, the responder comes back.
    pub fn prompt(
        &self,
        message_id: &str,
        blocks: Vec<ContentBlock>,
        content: &[Value],
        responder: Responder<PromptResponse>,
    ) -> Result<(), Box<Responder<PromptResponse>>> {
        let mut state = self.state.lock().unwrap();
        if state.exited.is_some() {
            return Err(Box::new(responder));
        }
        state.prompts.insert(
            message_id.to_owned(),
            Prompt {
                content: blocks,
                responder,
            },
        );
        let _ = self.input.send(json!({
            "type": "user",
            "uuid": message_id,
            "session_id": "",
            "message": {"role": "user", "content": content},
            "parent_tool_use_id": null
        }));
        Ok(())
    }

    /// Stops the running turn and drops the prompts claude has not taken.
    pub fn interrupt(&self) {
        if self.updates.is_working() {
            self.state.lock().unwrap().cancelling = true;
        }
        drop(self.control(&json!({"subtype": "interrupt", "cancel_queued": true})));
    }

    /// Asks claude to change its permission mode and returns the receiver
    /// of its answer.
    pub fn set_permission_mode(&self, mode: &str) -> oneshot::Receiver<Result<(), String>> {
        self.control(&json!({"subtype": "set_permission_mode", "mode": mode}))
    }

    /// Asks claude to change its model and returns the receiver of its
    /// answer.
    pub fn set_model(&self, model: &str) -> oneshot::Receiver<Result<(), String>> {
        self.control(&json!({"subtype": "set_model", "model": model}))
    }

    /// Asks claude to change its effort level and returns the receiver of
    /// its answer.
    pub fn set_effort(&self, effort: &str) -> oneshot::Receiver<Result<(), String>> {
        self.control(&json!({
            "subtype": "apply_flag_settings",
            "settings": {"effortLevel": effort},
        }))
    }

    /// Sends a control request and returns the receiver of claude's answer,
    /// which closes if claude exits first.
    fn control(&self, request: &Value) -> oneshot::Receiver<Result<(), String>> {
        let (reply, answer) = oneshot::channel();
        let mut state = self.state.lock().unwrap();
        if state.exited.is_none() {
            state.control_counter += 1;
            let id = format!("control-{}", state.control_counter);
            state.controls.insert(id.clone(), reply);
            let _ = self.input.send(json!({
                "type": "control_request",
                "request_id": id,
                "request": request
            }));
        }
        answer
    }
}

/// Claude's output, translated as it arrives.
struct Output {
    updates: Arc<Updates>,
    state: Arc<Mutex<State>>,
    /// Claude's stdin, for answers to its requests. It is weak so that the
    /// session's `Claude` alone keeps stdin open.
    input: mpsc::WeakUnboundedSender<Value>,
    /// The message and thought being streamed, once they have sent a delta.
    text_id: Option<String>,
    thought_id: Option<String>,
    /// The error of a turn that ended while a prompt waited to be taken. It
    /// is why the prompt fails if claude exits, as when claude cannot resume
    /// the conversation.
    unclaimed_error: Option<String>,
}

impl Output {
    /// Translates claude's output until claude exits, then ends what it left
    /// open.
    async fn read(mut self, mut child: Child) {
        let error =
            match acp_bridge::pipes::read_output(&mut child, |line| self.translate(line)).await {
                Ok((status, stderr)) => format!(
                    "claude exited with {status}: {}",
                    String::from_utf8_lossy(&stderr).trim()
                ),
                Err(error) => format!("claude process failed: {error}"),
            };
        let (prompts, cancelling) = {
            let mut state = self.state.lock().unwrap();
            state.exited = Some(error.clone());
            state.initializing = false;
            state.controls.clear();
            for (_, permission) in state.permissions.drain() {
                permission.abort();
            }
            (std::mem::take(&mut state.prompts), state.cancelling)
        };
        self.updates.exited();
        let result = if cancelling {
            Err(RunError::Cancelled)
        } else {
            Err(RunError::Failed(error.clone()))
        };
        if self.updates.is_working() {
            self.updates.idle(stop_reason(&result));
        }
        let failure = self.unclaimed_error.take().unwrap_or(error);
        for (_, prompt) in prompts {
            prompt.fail(RunError::Failed(failure.clone()));
        }
    }

    fn send(&self, message: Value) {
        if let Some(input) = self.input.upgrade() {
            let _ = input.send(message);
        }
    }

    /// Translates a line of claude's output.
    fn translate(&mut self, line: &str) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
        };
        // A subagent's messages show as activity of the tool call that
        // started it; its streamed text is not shown.
        if let Some(parent) = value["parent_tool_use_id"].as_str() {
            if matches!(value["type"].as_str(), Some("assistant" | "user")) {
                self.updates.scheduling(&value, false);
                for event in message_events(&value) {
                    self.updates.subagent_event(parent, event);
                }
            }
            return;
        }
        // Claude echoes a prompt once its turn has taken it.
        if value["isReplay"] == true {
            if let Some(uuid) = value["uuid"].as_str() {
                self.started(uuid);
            }
            return;
        }
        let event = if value["type"] == "stream_event" {
            &value["event"]
        } else {
            &value
        };
        match event.get("type").and_then(Value::as_str) {
            Some("control_request") => self.control_request(event),
            Some("control_cancel_request") => {
                let permission = event["request_id"]
                    .as_str()
                    .and_then(|id| self.state.lock().unwrap().permissions.remove(id));
                if let Some(permission) = permission {
                    permission.abort();
                }
            }
            Some("control_response") => {
                let response = &event["response"];
                if response["request_id"] == "initialize" {
                    self.state.lock().unwrap().initializing = false;
                    if response["subtype"] == "error" {
                        // Prompts that fail once claude exits report why.
                        self.unclaimed_error = Some(
                            response["error"]
                                .as_str()
                                .unwrap_or("claude refused to initialize")
                                .to_owned(),
                        );
                    } else {
                        self.updates
                            .set_commands(commands::catalog(&response["response"]["commands"]));
                    }
                    return;
                }
                let reply = response["request_id"]
                    .as_str()
                    .and_then(|id| self.state.lock().unwrap().controls.remove(id));
                if let Some(reply) = reply {
                    let _ = reply.send(if response["subtype"] == "error" {
                        Err(response["error"]
                            .as_str()
                            .unwrap_or("claude refused the request")
                            .to_owned())
                    } else {
                        Ok(())
                    });
                }
            }
            Some("command_lifecycle") if event["state"] == "started" => {
                if let Some(uuid) = event["command_uuid"].as_str() {
                    self.started(uuid);
                }
            }
            Some("content_block_delta") => match event["delta"]["type"].as_str() {
                Some("text_delta") => {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        let id = self.text_id.get_or_insert_with(new_id);
                        self.updates.text_chunk(id, text.into());
                    }
                }
                Some("thinking_delta") => {
                    if let Some(text) = event["delta"]["thinking"]
                        .as_str()
                        .filter(|text| !text.is_empty())
                    {
                        let id = self.thought_id.get_or_insert_with(new_id);
                        self.updates.thought_chunk(id, text.into());
                    }
                }
                _ => {}
            },
            Some("assistant" | "user") => {
                self.updates.scheduling(&value, true);
                for event in message_events(&value) {
                    match event {
                        // Deltas already streamed this text.
                        Event::Text(_) if self.text_id.is_some() => {}
                        Event::Thought(_) if self.thought_id.is_some() => {}
                        // The client has the prompts it sent; the rest of
                        // what claude reports as the user is its own notes.
                        Event::UserText(_)
                        | Event::UserImage { .. }
                        | Event::UserDocument { .. } => {}
                        event => self.updates.event(event),
                    }
                }
                self.text_id = None;
                self.thought_id = None;
            }
            Some("system") => match event["subtype"].as_str() {
                Some("commands_changed") => {
                    self.updates
                        .set_commands(commands::catalog(&event["commands"]));
                }
                Some("session_state_changed") => {
                    // Claude's third state, waiting for a permission, is part
                    // of working; the client has the request itself.
                    match event["state"].as_str() {
                        Some("running") => self.updates.working(),
                        Some("idle") => self.updates.idle_without_reason(),
                        _ => {}
                    }
                }
                Some("task_started") => {
                    if let (Some(task), Some(tool)) =
                        (event["task_id"].as_str(), event["tool_use_id"].as_str())
                    {
                        self.updates.task_started(task, tool);
                    }
                }
                Some("task_notification") => {
                    if let Some(tool) = event["tool_use_id"].as_str() {
                        self.updates.task_notification(
                            tool,
                            event["status"].as_str().unwrap_or(""),
                            event["summary"].as_str().unwrap_or(""),
                        );
                    }
                }
                Some("background_tasks_changed") => {
                    let tasks = event["tasks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|task| task["task_id"].as_str().map(str::to_owned))
                        .collect();
                    self.updates.background_tasks(tasks);
                }
                _ => {}
            },
            Some("result") => self.result(&value),
            _ => {}
        }
    }

    /// Notes that claude took the prompt sent as `uuid` into a turn, which
    /// inserts it into the conversation. Claude says so with a `started`
    /// lifecycle line and again by echoing the prompt; the first one counts.
    fn started(&mut self, uuid: &str) {
        let prompt = self.state.lock().unwrap().prompts.remove(uuid);
        if let Some(prompt) = prompt {
            self.unclaimed_error = None;
            self.updates.user_message(uuid, prompt.content);
            let _ = prompt.responder.respond(PromptResponse::new(uuid));
            self.updates.working();
        }
    }

    /// Asks the client about a tool permission, or `AskUserQuestion`'s
    /// questions, while output keeps flowing, and refuses any other request.
    fn control_request(&self, event: &Value) {
        let Some(request_id) = event["request_id"].as_str() else {
            return;
        };
        let request = &event["request"];
        if request["subtype"] != "can_use_tool" {
            self.send(json!({
                "type": "control_response",
                "response": {
                    "subtype": "error",
                    "request_id": request_id,
                    "error": "Unsupported control request"
                }
            }));
            return;
        }
        let permission = PermissionRequest {
            name: request["tool_name"].as_str().unwrap_or("Tool").to_owned(),
            input: request["input"].clone(),
            tool_use_id: request["tool_use_id"].as_str().unwrap_or("").to_owned(),
            suggestions: request["permission_suggestions"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
            suppress_always_allow_rule: request["suppress_always_allow_rule"]
                .as_bool()
                .unwrap_or(false),
        };
        let updates = self.updates.clone();
        let input = self.input.clone();
        let id = request_id.to_owned();
        let task = tokio::spawn(async move {
            let answer = if permission.name == "AskUserQuestion" {
                updates.ask_user(permission).await
            } else {
                updates.permission(permission).await
            };
            if let Some(input) = input.upgrade() {
                let _ = input.send(json!({
                    "type": "control_response",
                    "response": {"subtype": "success", "request_id": id, "response": answer}
                }));
            }
        });
        self.state
            .lock()
            .unwrap()
            .permissions
            .insert(request_id.to_owned(), task.abort_handle());
    }

    /// Ends a turn. An interrupt also drops the prompts claude had not taken.
    fn result(&mut self, result: &Value) {
        let aborted = result["terminal_reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("aborted"));
        let (dropped, waiting, cancelling) = {
            let mut state = self.state.lock().unwrap();
            for (_, permission) in state.permissions.drain() {
                permission.abort();
            }
            let dropped = if aborted {
                std::mem::take(&mut state.prompts)
            } else {
                HashMap::new()
            };
            (
                dropped,
                !state.prompts.is_empty(),
                std::mem::take(&mut state.cancelling),
            )
        };
        let outcome = match result_outcome(result) {
            _ if aborted => Err(RunError::Cancelled),
            Err(_) if cancelling => Err(RunError::Cancelled),
            outcome => outcome,
        };
        if self.state.lock().unwrap().initializing
            && let Err(RunError::Failed(error)) = &outcome
        {
            self.unclaimed_error = Some(error.clone());
            return;
        }
        self.updates.fail_open_tools();
        self.text_id = None;
        self.thought_id = None;
        for (_, prompt) in dropped {
            prompt.fail(RunError::Cancelled);
        }
        // A failure while a prompt waits and no turn runs is not a turn's:
        // the error is why the prompt fails if claude exits.
        if let (Err(RunError::Failed(error)), true, false) =
            (&outcome, waiting, self.updates.is_working())
        {
            self.unclaimed_error = Some(error.clone());
            return;
        }
        self.updates.idle(stop_reason(&outcome));
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The ACP stop reason of a turn's outcome. A failed turn ends with the
/// error as its details.
pub fn stop_reason(result: &Result<Finish, RunError>) -> StopReason {
    match result {
        Ok(Finish::EndTurn) => StopReason::EndTurn,
        Ok(Finish::MaxTurns) => StopReason::MaxTurnRequests,
        Err(RunError::Cancelled) => StopReason::Cancelled,
        Err(RunError::Failed(error)) => StopReason::Error(ErrorStopReason::new().error(
            agent_client_protocol::v2::schema::Error::new(-32603, error.clone()),
        )),
    }
}

fn result_outcome(result: &Value) -> Result<Finish, RunError> {
    if result["subtype"] == "error_max_turns" {
        return Ok(Finish::MaxTurns);
    }
    if result["is_error"].as_bool() != Some(true) {
        return Ok(Finish::EndTurn);
    }
    let errors = result["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|errors| !errors.is_empty());
    let message = result["result"]
        .as_str()
        .filter(|message| !message.is_empty())
        .map(str::to_owned)
        .or(errors)
        .unwrap_or_else(|| "claude reported an error".into());
    Err(RunError::Failed(message))
}

/// The events of one Claude message: an `assistant` or `user` line of
/// claude's stream-json output, or the same entry in a transcript. Meta
/// messages, which Claude adds for the model, have none, and neither do
/// empty text and thinking blocks, which Claude records when it does not
/// display thinking.
pub fn message_events(entry: &Value) -> Vec<Event> {
    if entry["isMeta"] == true {
        return Vec::new();
    }
    let user = entry["type"] == "user";
    let content = &entry["message"]["content"];
    if let Some(text) = content.as_str() {
        return if user {
            vec![Event::UserText(text.to_owned())]
        } else {
            vec![Event::Text(text.to_owned())]
        };
    }
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter_map(|block| {
            let text = |key: &str| {
                block[key]
                    .as_str()
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            };
            match (user, block["type"].as_str()?) {
                (true, "text") => text("text").map(Event::UserText),
                (true, "image") => Some(Event::UserImage {
                    data: block["source"]["data"].as_str()?.to_owned(),
                    mime_type: block["source"]["media_type"].as_str()?.to_owned(),
                }),
                (true, "document") => Some(Event::UserDocument {
                    data: block["source"]["data"].as_str()?.to_owned(),
                    mime_type: block["source"]["media_type"].as_str()?.to_owned(),
                }),
                (true, "tool_result") => Some(Event::ToolResult {
                    id: text("tool_use_id")?,
                    is_error: block["is_error"].as_bool().unwrap_or(false),
                    output: tool_output(&block["content"]),
                }),
                (false, "text") => text("text").map(Event::Text),
                (false, "thinking") => text("thinking").map(Event::Thought),
                (false, "tool_use") => Some(Event::Tool {
                    id: text("id"),
                    name: text("name")?,
                    input: block["input"].clone(),
                }),
                _ => None,
            }
        })
        .collect()
}

/// What a tool result holds: a string, or text and image blocks. Other
/// blocks show as their JSON.
fn tool_output(content: &Value) -> Vec<ToolOutput> {
    if let Some(text) = content.as_str() {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![ToolOutput::Text(text.to_owned())]
        };
    }
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter_map(|block| match block["type"].as_str() {
            Some("text") => block["text"]
                .as_str()
                .filter(|text| !text.is_empty())
                .map(|text| ToolOutput::Text(text.to_owned())),
            Some("image") if block["source"]["type"] == "base64" => Some(ToolOutput::Image {
                data: block["source"]["data"].as_str()?.to_owned(),
                mime_type: block["source"]["media_type"].as_str()?.to_owned(),
            }),
            _ => Some(ToolOutput::Text(block.to_string())),
        })
        .collect()
}
