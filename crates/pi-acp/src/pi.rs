use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};

use acp_bridge::McpServer;
use agent_client_protocol::v2::Responder;
use agent_client_protocol::v2::schema::{
    self as acp, ContentBlock, PromptResponse, StopReason, ToolCallContent,
};
use serde_json::{Value, json};
use tokio::{
    process::{Child, Command},
    sync::{mpsc, oneshot},
};
use uuid::Uuid;

use crate::{
    tools,
    turns::{Prompt, Turns, error_stop},
    updates::Updates,
};

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

/// A `pi --mode rpc` process that serves one session for as long as it
/// runs. Everything it writes becomes session updates, whether or not a
/// prompt is waiting. Dropping it closes Pi's stdin, which ends Pi.
pub struct Pi {
    /// Keeps stdin open until the session drops this handle.
    _input: mpsc::UnboundedSender<Value>,
    updates: Updates,
    turns: Arc<Mutex<Turns>>,
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
        let turns = Arc::new(Mutex::new(Turns::new(&input, updates.clone())));
        let output = Output {
            updates: updates.clone(),
            turns: turns.clone(),
            cwd: cwd.to_owned(),
            blocks: HashMap::new(),
            tools: HashMap::new(),
            last_stop: None,
        };
        tokio::spawn(output.read(child));
        Ok(Self {
            _input: input,
            updates,
            turns,
        })
    }

    pub fn snapshot(&self) -> acp::StateUpdate {
        self.updates.snapshot()
    }

    pub fn change(&self, command: Value) -> oneshot::Receiver<Result<Value, String>> {
        self.turns.lock().unwrap().change(command)
    }

    pub fn query(&self, command: Value) -> oneshot::Receiver<Result<Value, String>> {
        self.turns.lock().unwrap().query(command)
    }

    pub fn has_exited(&self) -> bool {
        self.turns.lock().unwrap().has_exited()
    }

    /// Submits a prompt. Once Pi accepts it, the session gets it as the user
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
        self.turns.lock().unwrap().prompt(
            command,
            Box::new(Prompt {
                message_id: message_id.to_owned(),
                content,
                responder,
            }),
        )
    }

    pub fn cancel(&self) {
        self.turns.lock().unwrap().cancel();
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
    turns: Arc<Mutex<Turns>>,
    cwd: PathBuf,
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
        self.updates.fail_open_tools();
        self.turns.lock().unwrap().exited(error);
    }

    /// Translates a record of Pi's output.
    fn translate(&mut self, line: &str) {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let role = &record["message"]["role"];
        let assistant = role == "assistant";
        match record["type"].as_str() {
            Some("response") => self.turns.lock().unwrap().response(&record),
            Some("agent_start") => self.turns.lock().unwrap().started(),
            Some("agent_settled") => self.settled(record["aborted"] == true),
            Some("message_start") if assistant => self.blocks.clear(),
            Some("message_start") if role == "user" => {
                self.turns.lock().unwrap().user_message_started();
            }
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
                Turns::dialog(&self.turns, record);
            }
            _ => {}
        }
    }

    /// Ends the turn once Pi has no more automatic work.
    fn settled(&mut self, aborted: bool) {
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
        self.turns.lock().unwrap().settled(aborted, stop_reason);
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

fn new_id() -> String {
    Uuid::new_v4().to_string()
}
