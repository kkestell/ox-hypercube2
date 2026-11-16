//! What the Claude Code and Pi ACP bridges share: the ACP handler wiring,
//! the native process's pipes, and request checks.

pub mod pipes;
#[cfg(feature = "testing")]
pub mod testing;

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use agent_client_protocol::v2::schema::{self as acp, SessionUpdate, UpdateSessionNotification};
use agent_client_protocol::v2::{self, ConnectionTo, Responder};
use agent_client_protocol::{
    Agent, Client, Error, HandleDispatchFrom, JsonRpcResponse, Result, Stdio,
};
use serde_json::{Value, json};

/// A bridge's handling of the ACP requests every bridge answers. A bridge
/// process serves the one session it opens with `session/new` or
/// `session/resume`.
pub trait Bridge: Send + Sync + 'static {
    fn initialize(&self, request: &acp::InitializeRequest) -> acp::InitializeResponse;

    /// Opens a new session without starting its native process.
    fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> std::result::Result<acp::NewSessionResponse, Error>;

    /// Reports a new session idle and starts its native process, once the
    /// client knows the session.
    fn start_new(self: &Arc<Self>, id: &str, connection: &ConnectionTo<Client>);

    fn resume_session(
        self: &Arc<Self>,
        request: acp::ResumeSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> impl Future<Output = std::result::Result<acp::ResumeSessionResponse, Error>> + Send + 'static;

    /// Sends a config change before returning its completion future.
    fn set_config(
        self: &Arc<Self>,
        request: &acp::SetSessionConfigOptionRequest,
        connection: ConnectionTo<Client>,
    ) -> impl Future<Output = std::result::Result<acp::SetSessionConfigOptionResponse, Error>>
    + Send
    + 'static;

    fn prompt(
        self: &Arc<Self>,
        request: &acp::PromptRequest,
        responder: Responder<acp::PromptResponse>,
        connection: &ConnectionTo<Client>,
    ) -> Result<()>;

    fn cancel(&self, id: &str);
}

/// Serves `bridge` over stdio, with the handlers `builder` already has.
pub async fn serve<B: Bridge, H: HandleDispatchFrom<Client>>(
    builder: v2::Builder<Agent, H>,
    bridge: Arc<B>,
) -> Result<()> {
    builder
        .on_receive_request(
            {
                let bridge = bridge.clone();
                async move |request: acp::InitializeRequest,
                            responder: Responder<acp::InitializeResponse>,
                            _connection: ConnectionTo<Client>| {
                    responder.respond(bridge.initialize(&request))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let bridge = bridge.clone();
                async move |request: acp::NewSessionRequest,
                            responder: Responder<acp::NewSessionResponse>,
                            connection: ConnectionTo<Client>| {
                    let response = bridge.new_session(request);
                    let id = response.as_ref().ok().map(|new| new.session_id.to_string());
                    respond(responder, response)?;
                    if let Some(id) = id {
                        bridge.start_new(&id, &connection);
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let bridge = bridge.clone();
                async move |request: acp::ResumeSessionRequest,
                            responder: Responder<acp::ResumeSessionResponse>,
                            connection: ConnectionTo<Client>| {
                    let opening = bridge.resume_session(request, &connection);
                    connection.spawn(async move { respond(responder, opening.await) })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let bridge = bridge.clone();
                async move |request: acp::SetSessionConfigOptionRequest,
                            responder: Responder<acp::SetSessionConfigOptionResponse>,
                            connection: ConnectionTo<Client>| {
                    let change = bridge.set_config(&request, connection.clone());
                    connection.spawn(async move { respond(responder, change.await) })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let bridge = bridge.clone();
                async move |request: acp::PromptRequest,
                            responder: Responder<acp::PromptResponse>,
                            connection: ConnectionTo<Client>| {
                    bridge.prompt(&request, responder, &connection)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: acp::CancelSessionNotification,
                        _connection: ConnectionTo<Client>| {
                bridge.cancel(&notification.session_id.to_string());
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}

/// Answers `responder` with `result`'s response or error.
pub fn respond<T: JsonRpcResponse>(
    responder: Responder<T>,
    result: std::result::Result<T, Error>,
) -> Result<()> {
    match result {
        Ok(response) => responder.respond(response),
        Err(error) => responder.respond_with_error(error),
    }
}

pub fn notify(connection: &ConnectionTo<Client>, id: &str, update: SessionUpdate) {
    if let Err(error) =
        connection.send_notification(UpdateSessionNotification::new(id.to_owned(), update))
    {
        eprintln!("failed to send session update: {error}");
    }
}

/// The title of a session whose first prompt has `text`: its first 120
/// characters with whitespace collapsed. A blank prompt gives none.
pub fn title(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    Some(
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(120)
            .collect(),
    )
}

/// An invalid-params error.
pub fn invalid(message: impl Into<String>) -> Error {
    Error::new(-32602, message)
}

/// The text a session's `systemPrompt` appends to its harness's
/// instructions, its text blocks joined by blank lines. Bridges support the
/// `append` mode and text blocks.
pub fn system_prompt(
    prompt: Option<acp::SystemPrompt>,
) -> std::result::Result<Option<String>, Error> {
    let Some(prompt) = prompt else {
        return Ok(None);
    };
    if prompt.mode != acp::SystemPromptMode::Append {
        return Err(invalid("Unsupported system prompt mode"));
    }
    if prompt.content.is_empty() {
        return Err(invalid("The system prompt is empty"));
    }
    let texts = prompt
        .content
        .into_iter()
        .map(|block| match block {
            acp::ContentBlock::Text(text) => Ok(text.text),
            _ => Err(invalid("A system prompt holds text blocks only")),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(Some(texts.join("\n\n")))
}

/// The error of a `session/new` or `session/resume` on a connection that
/// already has its session.
pub fn already_open() -> Error {
    Error::invalid_request().data("this connection already has a session")
}

/// A session's working directory: an absolute path to a directory, with `.`
/// and `..` resolved.
pub fn cwd(path: &Path) -> std::result::Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("cwd must be an absolute path: {}", path.display()));
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    if !resolved.is_dir() {
        return Err(format!("cwd is not a directory: {}", resolved.display()));
    }
    Ok(resolved)
}

/// A stdio MCP server the client gives a session.
#[derive(Clone, Debug)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl McpServer {
    /// The server's `mcpServers` entry, which Claude's `--mcp-config` and
    /// Pi's `mcp.json` both take.
    pub fn config(&self) -> Value {
        json!({"command": self.command, "args": self.args, "env": self.env})
    }
}

/// The client's MCP servers. Bridges support stdio servers whose command is
/// an absolute path, as ACP v2 types it.
pub fn parse_mcp(servers: Vec<acp::McpServer>) -> std::result::Result<Vec<McpServer>, Error> {
    servers
        .into_iter()
        .map(|server| {
            let acp::McpServer::Stdio(server) = server else {
                return Err(invalid("only stdio MCP servers are supported"));
            };
            if !server.command.0.is_absolute() {
                return Err(invalid(format!(
                    "MCP server command must be an absolute path: {}",
                    server.command.0.display()
                )));
            }
            Ok(McpServer {
                name: server.name,
                command: server.command.0.to_string_lossy().into_owned(),
                args: server.args,
                env: server
                    .env
                    .into_iter()
                    .map(|var| (var.name, var.value))
                    .collect(),
            })
        })
        .collect()
}
