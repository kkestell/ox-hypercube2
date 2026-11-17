use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};

use acp_bridge::McpServer;
use agent_client_protocol::v2::Responder;
use agent_client_protocol::v2::schema::{
    self as acp, ContentBlock, ErrorStopReason, PromptResponse, StopReason, ToolCallContent,
};
use serde_json::{Value, json};
use tokio::{
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::AbortHandle,
    time::sleep,
};
use uuid::Uuid;

use crate::{dialogs, tools, updates::Updates};

/// The Pi extension that registers the client's MCP servers.
const MCP_EXTENSION: &str = include_str!("mcp-servers.js");

/// Writes `MCP_EXTENSION` to `<temp dir>/pi-acp/mcp-servers.js` and returns
/// the path. The file is replaced through a rename, so a Pi another bridge
/// starts meanwhile reads a whole file.
fn write_mcp_extension() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("pi-acp");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("mcp-servers.js");
    let staging = dir.join(format!("mcp-servers.js.{}", std::process::id()));
    std::fs::write(&staging, MCP_EXTENSION).map_err(|e| e.to_string())?;
    std::fs::rename(&staging, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A prompt waiting for Pi's answer to its `prompt` command, or for Pi to
/// take it into the run it steers.
struct Prompt {
    message_id: String,
    content: Vec<ContentBlock>,
    responder: Responder<PromptResponse>,
}

/// A command waiting for Pi's response.
enum Pending {
    Prompt(Box<Prompt>),
    /// `clear_queue` or `abort`, named for the log.
    Stop(&'static str),
    Request(oneshot::Sender<Result<Value, String>>),
}

#[derive(Default)]
struct State {
    counter: u64,
    /// Client forms still waiting for an answer, by Pi request ID.
    dialogs: HashMap<String, AbortHandle>,
    /// Commands Pi has not answered, by ID.
    pending: HashMap<String, Pending>,
    /// Prompts Pi queued to steer its run, in the order Pi takes them in.
    steering: VecDeque<Box<Prompt>>,
    /// The client cancelled while prompts waited for Pi's answer. Pi's abort
    /// does not stop a run that has not started, nor a steer it has yet to
    /// queue, so the bridge stops again as Pi answers each of them.
    abort_on_start: bool,
    /// Why Pi exited, once it has.
    exited: Option<String>,
}

impl State {
    fn withdraw_dialogs(&mut self, input: Option<&mpsc::UnboundedSender<Value>>) {
        for (id, task) in self.dialogs.drain() {
            task.abort();
            if let Some(input) = input {
                let _ =
                    input.send(json!({"type":"extension_ui_response", "id":id, "cancelled":true}));
            }
        }
    }

    /// Stops Pi's run. Pi's `abort` goes on to deliver the steering queue, so
    /// the queue is cleared first and its prompts fail as cancelled.
    fn stop(&mut self, input: &mpsc::UnboundedSender<Value>) {
        self.send(
            input,
            json!({"type": "clear_queue"}),
            Pending::Stop("clear_queue"),
        );
        self.send(input, json!({"type": "abort"}), Pending::Stop("abort"));
        for prompt in self.steering.drain(..) {
            let _ = prompt
                .responder
                .respond_with_error(agent_client_protocol::Error::request_cancelled());
        }
    }

    /// Sends `command` to Pi under a new ID and records what waits for the
    /// response.
    fn send(&mut self, input: &mpsc::UnboundedSender<Value>, mut command: Value, pending: Pending) {
        self.counter += 1;
        let id = self.counter.to_string();
        command["id"] = json!(id);
        self.pending.insert(id, pending);
        let _ = input.send(command);
    }
}

/// A `pi --mode rpc` process that serves one session for as long as it
/// runs. Everything it writes becomes session updates, whether or not a
/// prompt is waiting. Dropping it closes Pi's stdin, which ends Pi.
pub struct Pi {
    input: mpsc::UnboundedSender<Value>,
    updates: Updates,
    state: Arc<Mutex<State>>,
}

impl Pi {
    /// Starts Pi in `cwd` on the Pi session `pi_session_id`, which Pi opens,
    /// or creates when it does not exist, with `instructions` appended to
    /// Pi's system prompt. The extension `MCP_EXTENSION` registers
    /// `mcp_servers` with Pi, their tools declared to the model directly.
    pub fn start(
        cwd: &Path,
        pi_session_id: &str,
        instructions: Option<&str>,
        mcp_servers: &[McpServer],
        updates: Updates,
    ) -> Result<Self, String> {
        let mut command = Command::new("pi");
        command.args(["--mode", "rpc", "--approve"]);
        command.args(["--session-id", pi_session_id]);
        if let Some(instructions) = instructions {
            command.arg("--append-system-prompt").arg(instructions);
        }
        if !mcp_servers.is_empty() {
            let extension = write_mcp_extension()
                .map_err(|e| format!("failed to write the MCP extension: {e}"))?;
            let servers: serde_json::Map<_, _> = mcp_servers
                .iter()
                .map(|server| {
                    let mut entry = server.config();
                    entry["exposure"] = json!("direct");
                    (server.name.clone(), entry)
                })
                .collect();
            command
                .arg("-e")
                .arg(extension)
                .env("PI_ACP_MCP_SERVERS", Value::Object(servers).to_string());
        }
        let mut child = command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to start pi: {e}"))?;
        let (input, commands) = mpsc::unbounded_channel();
        tokio::spawn(acp_bridge::pipes::write_input(
            child.stdin.take().expect("piped stdin"),
            commands,
        ));
        let state = Arc::new(Mutex::new(State::default()));
        let output = Output {
            updates: updates.clone(),
            state: state.clone(),
            input: input.downgrade(),
            cwd: cwd.to_owned(),
            running: false,
            starting: 0,
            blocks: HashMap::new(),
            tools: HashMap::new(),
            last_stop: None,
        };
        tokio::spawn(output.read(child));
        Ok(Self {
            input,
            updates,
            state,
        })
    }

    pub fn snapshot(&self) -> acp::StateUpdate {
        self.updates.snapshot()
    }

    pub fn request(&self, command: Value) -> oneshot::Receiver<Result<Value, String>> {
        let (send, receive) = oneshot::channel();
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.exited {
            let _ = send.send(Err(error.clone()));
        } else {
            state.send(&self.input, command, Pending::Request(send));
        }
        receive
    }

    pub fn has_exited(&self) -> bool {
        self.state.lock().unwrap().exited.is_some()
    }

    /// Sends a prompt. Once Pi accepts it, the session gets it as the user
    /// message `message_id` and `responder` answers with that ID. If Pi has
    /// exited, the responder comes back.
    pub fn prompt(
        &self,
        message_id: &str,
        content: Vec<ContentBlock>,
        message: &str,
        images: &[Value],
        responder: Responder<PromptResponse>,
    ) -> Result<(), Box<Responder<PromptResponse>>> {
        let mut state = self.state.lock().unwrap();
        if state.exited.is_some() {
            return Err(Box::new(responder));
        }
        let mut command = json!({
            "type": "prompt",
            "message": message,
            // Pi ignores this while idle, and steers its run with the prompt
            // while it works on a run of its own.
            "streamingBehavior": "steer",
        });
        if !images.is_empty() {
            command["images"] = json!(images);
        }
        state.send(
            &self.input,
            command,
            Pending::Prompt(Box::new(Prompt {
                message_id: message_id.to_owned(),
                content,
                responder,
            })),
        );
        Ok(())
    }

    /// Stops Pi's run, including the run of a prompt Pi has yet to answer,
    /// and fails the prompts waiting to steer it.
    pub fn abort(&self) {
        let mut state = self.state.lock().unwrap();
        if state.exited.is_some() {
            return;
        }
        if state
            .pending
            .values()
            .any(|pending| matches!(pending, Pending::Prompt(_)))
        {
            state.abort_on_start = true;
        }
        state.withdraw_dialogs(Some(&self.input));
        state.stop(&self.input);
    }
}

/// A text or thinking block of the assistant message Pi is streaming.
struct Block {
    message_id: String,
    /// A delta of the block has been sent.
    streamed: bool,
}

/// A tool call that has opened and not ended.
struct Tool {
    name: String,
    args: Value,
    /// The file the call names existed when the call opened.
    existed: bool,
}

/// Pi's output, translated as it arrives.
struct Output {
    updates: Updates,
    state: Arc<Mutex<State>>,
    /// Pi's stdin, for answers to its requests. It is weak so that the
    /// session's `Pi` alone keeps stdin open.
    input: mpsc::WeakUnboundedSender<Value>,
    cwd: PathBuf,
    /// Pi has started a run and not settled.
    running: bool,
    /// Prompts Pi started whose user message has not started yet. Their
    /// user messages precede those of the prompts steering their runs.
    starting: usize,
    /// The blocks of the assistant message being streamed, by content index.
    blocks: HashMap<u64, Block>,
    tools: HashMap<String, Tool>,
    /// The stop reason and error message of the run's last assistant message.
    last_stop: Option<(String, Option<String>)>,
}

impl Output {
    /// Translates Pi's output until Pi exits, then ends what it left open.
    async fn read(mut self, mut child: Child) {
        let error =
            match acp_bridge::pipes::read_output(&mut child, |line| self.translate(line)).await {
                Ok((status, stderr)) => format!(
                    "pi exited with {status}: {}",
                    String::from_utf8_lossy(&stderr).trim()
                ),
                Err(error) => format!("pi process failed: {error}"),
            };
        let (pending, steering) = {
            let mut state = self.state.lock().unwrap();
            state.exited = Some(error.clone());
            state.withdraw_dialogs(None);
            (
                std::mem::take(&mut state.pending),
                std::mem::take(&mut state.steering),
            )
        };
        let fail = |prompt: Box<Prompt>| {
            let _ = prompt
                .responder
                .respond_with_error(agent_client_protocol::Error::new(-32603, error.clone()));
        };
        for (_, pending) in pending {
            match pending {
                Pending::Prompt(prompt) => fail(prompt),
                Pending::Request(send) => {
                    let _ = send.send(Err(error.clone()));
                }
                Pending::Stop(_) => {}
            }
        }
        steering.into_iter().for_each(fail);

        self.updates.fail_open_tools();
        self.updates.idle(error_stop(error));
    }

    fn send(&self, message: Value) {
        if let Some(input) = self.input.upgrade() {
            let _ = input.send(message);
        }
    }

    /// Translates a record of Pi's output.
    fn translate(&mut self, line: &str) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let role = &record["message"]["role"];
        let assistant = role == "assistant";
        match record["type"].as_str() {
            Some("response") => self.response(&record),
            Some("agent_start") => {
                self.running = true;
                self.updates.working();
            }
            Some("agent_settled") => self.settled(record["aborted"] == true),
            Some("message_start") if assistant => self.blocks.clear(),
            Some("message_start") if role == "user" => self.user_message_started(),
            Some("message_update") => self.message_update(&record["assistantMessageEvent"]),
            Some("message_end") if assistant => self.assistant_message(&record["message"]),
            Some("tool_execution_start") => {
                if let Some(id) = record["toolCallId"].as_str() {
                    self.updates.tool_started(id);
                }
            }
            Some("tool_execution_end") => self.tool_ended(&record),
            Some("extension_ui_request")
                if matches!(
                    record["method"].as_str(),
                    Some("select" | "confirm" | "input" | "editor")
                ) =>
            {
                self.dialog(record);
            }
            _ => {}
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "Pi's dialog timeout is the behavior"
    )]
    fn dialog(&self, request: Value) {
        let Some((message, schema)) = dialogs::form(&request) else {
            self.send(dialogs::response(&request, None));
            return;
        };
        let Some(id) = request["id"].as_str().map(str::to_owned) else {
            return;
        };
        let updates = self.updates.clone();
        let state = self.state.clone();
        let input = self.input.clone();
        let mut locked = self.state.lock().unwrap();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            let ask = updates.ask(message, schema);
            // Pi silently forgets timed-out dialogs; it sends no withdrawal record.
            // An extension's private AbortSignal cannot be observed over RPC.
            let action = if let Some(timeout) = dialogs::timeout(&request) {
                tokio::select! {
                    action = ask => action,
                    () = sleep(timeout) => None,
                }
            } else {
                ask.await
            };
            let mut state = state.lock().unwrap();
            if state.dialogs.remove(&task_id).is_some()
                && let Some(input) = input.upgrade()
            {
                let _ = input.send(dialogs::response(&request, action));
            }
        });
        locked.dialogs.insert(id, task.abort_handle());
    }

    /// Resolves the command Pi answered.
    fn response(&mut self, response: &Value) {
        let id = response["id"].as_str().unwrap_or_default();
        let pending = self.state.lock().unwrap().pending.remove(id);
        match pending {
            Some(Pending::Prompt(prompt)) => self.prompt_answered(prompt, response),
            Some(Pending::Request(send)) => {
                let result = if response["success"] == true {
                    Ok(response["data"].clone())
                } else {
                    Err(response["error"]
                        .as_str()
                        .unwrap_or("pi refused the command")
                        .to_owned())
                };
                let _ = send.send(result);
            }
            Some(Pending::Stop(command)) => {
                if response["success"] != true {
                    eprintln!("pi refused to {command}: {}", response["error"]);
                }
            }
            None => eprintln!("pi answered an unknown command: {response}"),
        }
    }

    /// Answers a prompt once Pi accepted or refused it. A prompt an
    /// extension command handled ends its turn unless the command started a
    /// run. A prompt Pi queued to steer its run waits for Pi to take it in.
    fn prompt_answered(&mut self, prompt: Box<Prompt>, response: &Value) {
        let abort = {
            let mut state = self.state.lock().unwrap();
            let abort = state.abort_on_start;
            if !state
                .pending
                .values()
                .any(|pending| matches!(pending, Pending::Prompt(_)))
            {
                state.abort_on_start = false;
            }
            abort
        };
        if response["success"] != true {
            let error = response["error"]
                .as_str()
                .unwrap_or("pi refused the prompt");
            let _ = prompt
                .responder
                .respond_with_error(agent_client_protocol::Error::new(-32603, error));
            return;
        }
        let disposition = &response["data"]["disposition"];
        if disposition == "queued" {
            let mut state = self.state.lock().unwrap();
            state.steering.push_back(prompt);
            if abort && let Some(input) = self.input.upgrade() {
                state.stop(&input);
            }
            return;
        }
        let _ = prompt
            .responder
            .respond(PromptResponse::new(prompt.message_id.clone()));
        self.updates
            .user_message(&prompt.message_id, prompt.content);
        self.updates.working();
        if disposition == "handled" {
            if !self.running {
                self.updates.idle(StopReason::EndTurn);
            }
        } else {
            self.starting += 1;
            if abort && let Some(input) = self.input.upgrade() {
                self.state.lock().unwrap().stop(&input);
            }
        }
    }

    /// Answers the next steering prompt once Pi takes it into its run. The
    /// user message of a prompt Pi started is already sent.
    fn user_message_started(&mut self) {
        if self.starting > 0 {
            self.starting -= 1;
            return;
        }
        let Some(prompt) = self.state.lock().unwrap().steering.pop_front() else {
            return;
        };
        let _ = prompt
            .responder
            .respond(PromptResponse::new(prompt.message_id.clone()));
        self.updates
            .user_message(&prompt.message_id, prompt.content);
    }

    /// Ends the turn once Pi has no more automatic work.
    fn settled(&mut self, aborted: bool) {
        if aborted {
            self.state
                .lock()
                .unwrap()
                .withdraw_dialogs(self.input.upgrade().as_ref());
        }
        self.running = false;
        self.updates.fail_open_tools();
        self.tools.clear();
        let last_stop = self.last_stop.take();
        let stop_reason = match last_stop {
            _ if aborted => StopReason::Cancelled,
            Some((reason, message)) if reason == "error" => {
                error_stop(message.unwrap_or_else(|| "pi reported an error".to_owned()))
            }
            Some((reason, _)) if reason == "length" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        };
        self.updates.idle(stop_reason);
    }

    /// Streams a delta of the assistant message.
    fn message_update(&mut self, event: &Value) {
        let Some(index) = event["contentIndex"].as_u64() else {
            return;
        };
        match event["type"].as_str() {
            Some("text_start" | "thinking_start") => {
                self.blocks.insert(
                    index,
                    Block {
                        message_id: new_id(),
                        streamed: false,
                    },
                );
            }
            Some(kind @ ("text_delta" | "thinking_delta")) => {
                let Some(delta) = event["delta"].as_str().filter(|delta| !delta.is_empty()) else {
                    return;
                };
                let block = self.blocks.entry(index).or_insert_with(|| Block {
                    message_id: new_id(),
                    streamed: false,
                });
                block.streamed = true;
                if kind == "text_delta" {
                    self.updates.text_chunk(&block.message_id, delta.to_owned());
                } else {
                    self.updates
                        .thought_chunk(&block.message_id, delta.to_owned());
                }
            }
            _ => {}
        }
    }

    /// Sends what streaming did not of a finished assistant message, and
    /// opens its tool calls.
    fn assistant_message(&mut self, message: &Value) {
        let blocks = std::mem::take(&mut self.blocks);
        for (index, block) in message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let streamed = blocks.get(&(index as u64));
            if streamed.is_some_and(|block| block.streamed) {
                continue;
            }
            let message_id = streamed.map_or_else(new_id, |block| block.message_id.clone());
            match block["type"].as_str() {
                Some("text") => {
                    if let Some(text) = block["text"].as_str().filter(|text| !text.is_empty()) {
                        self.updates.message(&message_id, text.to_owned());
                    }
                }
                Some("thinking") => {
                    if let Some(text) = block["thinking"].as_str().filter(|text| !text.is_empty()) {
                        self.updates.thought(&message_id, text.to_owned());
                    }
                }
                Some("toolCall") => {
                    let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                    else {
                        continue;
                    };
                    let args = block["arguments"].clone();
                    let existed = tools::path(&args, &self.cwd).is_some_and(|path| path.exists());
                    self.updates.open_tool(id, name, &args, &self.cwd);
                    self.tools.insert(
                        id.to_owned(),
                        Tool {
                            name: name.to_owned(),
                            args,
                            existed,
                        },
                    );
                }
                _ => {}
            }
        }
        self.last_stop = message["stopReason"].as_str().map(|reason| {
            (
                reason.to_owned(),
                message["errorMessage"].as_str().map(str::to_owned),
            )
        });
    }

    /// Ends a tool call with its result, after the change a successful
    /// `edit` or `write` made.
    fn tool_ended(&mut self, record: &Value) {
        let Some(id) = record["toolCallId"].as_str() else {
            return;
        };
        let failed = record["isError"] == true;
        let result = &record["result"];
        let mut content: Vec<ToolCallContent> = self
            .tools
            .remove(id)
            .filter(|_| !failed)
            .and_then(|tool| {
                tools::diff(
                    &tool.name,
                    &tool.args,
                    &result["details"],
                    &self.cwd,
                    Some(tool.existed),
                )
            })
            .into_iter()
            .collect();
        content.extend(
            crate::content::from_pi(&result["content"])
                .into_iter()
                .map(ToolCallContent::from),
        );
        self.updates.tool_ended(id, failed, content);
    }
}

fn error_stop(message: String) -> StopReason {
    StopReason::Error(ErrorStopReason::new().error(acp::Error::new(-32603, message)))
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}
