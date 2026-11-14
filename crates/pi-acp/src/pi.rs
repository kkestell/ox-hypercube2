use std::{
    collections::HashMap,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};

use agent_client_protocol::v2::Responder;
use agent_client_protocol::v2::schema::{
    self as acp, ContentBlock, ErrorStopReason, PromptResponse, StopReason, TextContent,
    ToolCallContent,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{tools, updates::Updates};

/// A prompt waiting for Pi's answer to its `prompt` command.
struct Prompt {
    message_id: String,
    content: Vec<ContentBlock>,
    responder: Responder<PromptResponse>,
}

/// A command waiting for Pi's response.
enum Pending {
    Prompt(Box<Prompt>),
    Abort,
}

#[derive(Default)]
struct State {
    counter: u64,
    /// Commands Pi has not answered, by ID.
    pending: HashMap<String, Pending>,
    /// The client cancelled while a prompt waited for Pi's answer. Pi's abort
    /// does not stop a run that has not started, so the bridge aborts again
    /// once Pi starts or queues the prompt.
    abort_on_start: bool,
    /// Why Pi exited, once it has.
    exited: Option<String>,
    /// The session is closing, so Pi's exit is a cancellation.
    closing: bool,
}

impl State {
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
    state: Arc<Mutex<State>>,
    /// Closes once Pi has exited.
    exited: oneshot::Receiver<()>,
}

impl Pi {
    /// Starts Pi in `cwd` on the Pi session `pi_session_id`, which Pi opens,
    /// or creates when it does not exist.
    pub fn start(cwd: &Path, pi_session_id: &str, updates: Updates) -> Result<Self, String> {
        let mut child = Command::new("pi")
            .args(["--mode", "rpc", "--approve", "--session-id", pi_session_id])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("failed to start pi: {e}"))?;
        let (input, commands) = mpsc::unbounded_channel();
        tokio::spawn(write_input(
            child.stdin.take().expect("piped stdin"),
            commands,
        ));
        let state = Arc::new(Mutex::new(State::default()));
        let (exit, exited) = oneshot::channel();
        let output = Output {
            updates,
            state: state.clone(),
            input: input.downgrade(),
            cwd: cwd.to_owned(),
            running: false,
            blocks: HashMap::new(),
            tools: HashMap::new(),
            last_stop: None,
            _exit: exit,
        };
        tokio::spawn(output.read(child));
        Ok(Self {
            input,
            state,
            exited,
        })
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
            // Pi ignores this while idle, and queues the prompt after its
            // run while it works on a run of its own.
            "streamingBehavior": "followUp",
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

    /// Stops Pi's run, including the run of a prompt Pi has yet to answer.
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
        state.send(&self.input, json!({"type": "abort"}), Pending::Abort);
    }

    /// Stops Pi and returns once it has exited. Its turn is cancelled.
    pub async fn stop(self) {
        self.state.lock().unwrap().closing = true;
        self.abort();
        let Self { input, exited, .. } = self;
        // Pi exits when its stdin closes.
        drop(input);
        let _ = exited.await;
    }
}

/// Writes each command to Pi's stdin as a line, until the session drops its
/// `Pi` or Pi closes its stdin.
async fn write_input(mut stdin: ChildStdin, mut commands: mpsc::UnboundedReceiver<Value>) {
    while let Some(command) = commands.recv().await {
        let mut line = command.to_string().into_bytes();
        line.push(b'\n');
        if stdin.write_all(&line).await.is_err() {
            return;
        }
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
    /// The blocks of the assistant message being streamed, by content index.
    blocks: HashMap<u64, Block>,
    tools: HashMap<String, Tool>,
    /// The stop reason and error message of the run's last assistant message.
    last_stop: Option<(String, Option<String>)>,
    /// Dropped once Pi has exited.
    _exit: oneshot::Sender<()>,
}

impl Output {
    /// Translates Pi's output until Pi exits, then ends what it left open.
    async fn read(mut self, mut child: Child) {
        let mut stderr = Stderr::collect(child.stderr.take().expect("piped stderr"));
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        // A line being read; `read_until` keeps a partial line here when the
        // other branch wins.
        let mut line = Vec::new();
        let status = loop {
            tokio::select! {
                biased;
                read = stdout.read_until(b'\n', &mut line) => match read {
                    Ok(n) if n > 0 => {
                        self.translate_lines(&line);
                        line.clear();
                    }
                    _ => break child.wait().await,
                },
                status = child.wait() => {
                    // Everything Pi wrote is in the pipe; a process it
                    // started may hold the pipe open.
                    drain(&stdout, &mut line);
                    self.translate_lines(&line);
                    break status;
                }
            }
        };
        let error = match status {
            Ok(status) => format!(
                "pi exited with {status}: {}",
                String::from_utf8_lossy(&stderr.finish().await).trim()
            ),
            Err(error) => format!("pi process failed: {error}"),
        };
        let (pending, closing) = {
            let mut state = self.state.lock().unwrap();
            state.exited = Some(error.clone());
            (std::mem::take(&mut state.pending), state.closing)
        };
        for (_, pending) in pending {
            if let Pending::Prompt(prompt) = pending {
                let _ = prompt
                    .responder
                    .respond_with_error(agent_client_protocol::Error::new(-32603, error.clone()));
            }
        }
        self.updates.fail_open_tools();
        self.updates.idle(if closing {
            StopReason::Cancelled
        } else {
            error_stop(error)
        });
    }

    fn send(&self, message: Value) {
        if let Some(input) = self.input.upgrade() {
            let _ = input.send(message);
        }
    }

    /// Translates each complete or final line in `bytes`.
    fn translate_lines(&mut self, bytes: &[u8]) {
        for line in bytes.split(|&byte| byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if !line.is_empty() {
                self.translate(&String::from_utf8_lossy(line));
            }
        }
    }

    /// Translates a record of Pi's output.
    fn translate(&mut self, line: &str) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let assistant = record["message"]["role"] == "assistant";
        match record["type"].as_str() {
            Some("response") => self.response(&record),
            Some("agent_start") => {
                self.running = true;
                self.updates.working();
            }
            Some("agent_settled") => self.settled(record["aborted"] == true),
            Some("message_start") if assistant => self.blocks.clear(),
            Some("message_update") => self.message_update(&record["assistantMessageEvent"]),
            Some("message_end") if assistant => self.assistant_message(&record["message"]),
            Some("tool_execution_start") => {
                if let Some(id) = record["toolCallId"].as_str() {
                    self.updates.tool_started(id);
                }
            }
            Some("tool_execution_end") => self.tool_ended(&record),
            Some("extension_ui_request") => {
                // TODO(task 5): ask the client with a form elicitation.
                if matches!(
                    record["method"].as_str(),
                    Some("select" | "confirm" | "input" | "editor")
                ) {
                    self.send(json!({
                        "type": "extension_ui_response",
                        "id": record["id"],
                        "cancelled": true,
                    }));
                }
            }
            _ => {}
        }
    }

    /// Resolves the command Pi answered.
    fn response(&mut self, response: &Value) {
        let id = response["id"].as_str().unwrap_or_default();
        let pending = self.state.lock().unwrap().pending.remove(id);
        match pending {
            Some(Pending::Prompt(prompt)) => self.prompt_answered(prompt, response),
            Some(Pending::Abort) => {
                if response["success"] != true {
                    eprintln!("pi refused to abort: {}", response["error"]);
                }
            }
            None => eprintln!("pi answered an unknown command: {response}"),
        }
    }

    /// Answers a prompt once Pi accepted or refused it. A prompt an
    /// extension command handled ends its turn unless the command started a
    /// run.
    fn prompt_answered(&mut self, prompt: Box<Prompt>, response: &Value) {
        let abort = std::mem::take(&mut self.state.lock().unwrap().abort_on_start);
        if response["success"] != true {
            let error = response["error"]
                .as_str()
                .unwrap_or("pi refused the prompt");
            let _ = prompt
                .responder
                .respond_with_error(agent_client_protocol::Error::new(-32603, error));
            return;
        }
        let _ = prompt
            .responder
            .respond(PromptResponse::new(prompt.message_id.clone()));
        self.updates
            .user_message(&prompt.message_id, prompt.content);
        self.updates.working();
        if response["data"]["disposition"] == "handled" {
            if !self.running {
                self.updates.idle(StopReason::EndTurn);
            }
        } else if abort && let Some(input) = self.input.upgrade() {
            self.state
                .lock()
                .unwrap()
                .send(&input, json!({"type": "abort"}), Pending::Abort);
        }
    }

    /// Ends the turn once Pi has no more automatic work.
    fn settled(&mut self, aborted: bool) {
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
                    tool.existed,
                )
            })
            .into_iter()
            .collect();
        content.extend(
            result["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| match block["type"].as_str()? {
                    "text" => Some(ContentBlock::Text(TextContent::new(
                        block["text"].as_str()?,
                    ))),
                    "image" => Some(ContentBlock::Image(acp::ImageContent::new(
                        block["data"].as_str()?,
                        block["mimeType"].as_str()?,
                    ))),
                    _ => None,
                })
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

/// Appends what Pi's stdout holds once Pi has exited: the reader's buffer,
/// then the pipe's contents, read from the descriptor directly because the
/// runtime may not yet have seen the pipe become readable.
fn drain(stdout: &BufReader<ChildStdout>, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(stdout.buffer());
    read_buffered(stdout.get_ref(), bytes);
}

/// Reads Pi's stderr while Pi runs, so a full pipe never blocks it.
struct Stderr {
    finish: Option<oneshot::Sender<()>>,
    task: JoinHandle<Vec<u8>>,
}

impl Stderr {
    fn collect(stderr: ChildStderr) -> Self {
        let (finish, finished) = oneshot::channel();
        Self {
            finish: Some(finish),
            task: tokio::spawn(read_stderr(stderr, finished)),
        }
    }

    /// Everything Pi wrote to stderr. Call it once Pi has exited: what it
    /// wrote is then in the pipe, and a process it started that still holds
    /// the pipe open does not delay the result.
    async fn finish(&mut self) -> Vec<u8> {
        if let Some(finish) = self.finish.take() {
            let _ = finish.send(());
        }
        (&mut self.task).await.unwrap_or_default()
    }
}

impl Drop for Stderr {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_stderr(mut stderr: ChildStderr, mut finished: oneshot::Receiver<()>) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        tokio::select! {
            read = stderr.read(&mut chunk) => match read {
                Ok(n) if n > 0 => buffer.extend_from_slice(&chunk[..n]),
                _ => return buffer,
            },
            _ = &mut finished => {
                read_buffered(&stderr, &mut buffer);
                return buffer;
            }
        }
    }
}

/// Reads what the pipe holds now. Tokio opens child pipes non-blocking, so
/// `read` fails with `EAGAIN` instead of waiting once the pipe is empty.
fn read_buffered(pipe: &impl AsRawFd, buffer: &mut Vec<u8>) {
    let mut chunk = [0u8; 8192];
    loop {
        // SAFETY: reads at most `chunk.len()` bytes into `chunk` from a
        // descriptor `pipe` owns and keeps open for the call.
        let n = unsafe { libc::read(pipe.as_raw_fd(), chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..n.cast_unsigned()]);
    }
}
