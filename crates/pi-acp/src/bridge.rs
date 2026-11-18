use std::{
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};

use acp_bridge::{McpServer, invalid, notify};
use agent_client_protocol::v2::schema::{self as acp, SessionUpdate};
use agent_client_protocol::v2::{ConnectionTo, Responder};
use agent_client_protocol::{Client, Error};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    commands, content, history, options,
    pi::Pi,
    updates::{self, Updates},
};

/// The session a bridge process serves. Its ID is also its Pi session ID,
/// and Pi keeps the session, with its model and thinking level.
struct Session {
    id: String,
    cwd: PathBuf,
    /// The client's system prompt, appended to Pi's.
    instructions: Option<String>,
    /// The client's MCP servers, which every Pi of the session registers.
    mcp_servers: Vec<McpServer>,
    /// The session has had a prompt, which titled it.
    titled: bool,
    /// The session's Pi, once started.
    pi: Option<Pi>,
}

struct Description {
    state: Value,
    config_options: Vec<acp::SessionConfigOption>,
    available_commands: Vec<acp::AvailableCommand>,
}

/// A bridge process serves one session, opened with `session/new` or
/// `session/resume`.
pub struct Bridge {
    session: Mutex<Option<Session>>,
}

impl Bridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            session: Mutex::new(None),
        })
    }
}

impl acp_bridge::Bridge for Bridge {
    fn initialize(&self, request: &acp::InitializeRequest) -> acp::InitializeResponse {
        acp::InitializeResponse::new(
            request.protocol_version,
            acp::Implementation::new("pi-acp", env!("CARGO_PKG_VERSION")).title("Pi ACP Bridge"),
        )
        .capabilities(
            acp::AgentCapabilities::new().session(
                acp::SessionCapabilities::new()
                    .prompt(
                        acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new()),
                    )
                    .system_prompt(acp::SessionSystemPromptCapabilities::new())
                    .mcp(acp::McpCapabilities::new().stdio(acp::McpStdioCapabilities::new())),
            ),
        )
    }

    /// Opens the session and starts its Pi, whose config options and
    /// commands the response carries. A Pi that fails to start or to
    /// describe itself is logged and the response has neither; the first
    /// prompt starts Pi again.
    fn new_session(
        self: &Arc<Self>,
        request: acp::NewSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> impl Future<Output = Result<acp::NewSessionResponse, Error>> + Send + 'static {
        let opened = self.open_new(request);
        let description = opened
            .as_ref()
            .ok()
            .and_then(|id| match self.start_pi(id, connection) {
                Ok(_) => Some(self.describe(id)),
                Err(error) => {
                    eprintln!("failed to start pi for session {id}: {}", error.message);
                    None
                }
            });
        async move {
            let id = opened?;
            let response = acp::NewSessionResponse::new(id.clone());
            let Some(description) = description else {
                return Ok(response);
            };
            Ok(match description.await {
                Ok(description) => response
                    .config_options(description.config_options)
                    .available_commands(description.available_commands),
                Err(error) => {
                    eprintln!("failed to describe pi for session {id}: {}", error.message);
                    response
                }
            })
        }
    }

    /// Reports the new session idle.
    fn start_new(self: &Arc<Self>, id: &str, connection: &ConnectionTo<Client>) {
        notify(
            connection,
            id,
            SessionUpdate::StateUpdate(updates::idle_state()),
        );
    }

    /// Stops the session's run, whether a prompt or Pi started it.
    fn cancel(&self, id: &str) {
        if let Ok(mut open) = self.require(id)
            && let Some(pi) = &open_session(&mut open).pi
        {
            pi.cancel();
        }
    }

    /// Submits a prompt to its session's Pi and answers once Pi takes it in.
    /// The bridge records the prompt before returning so a following cancel
    /// can stop it.
    fn prompt(
        self: &Arc<Self>,
        request: &acp::PromptRequest,
        responder: Responder<acp::PromptResponse>,
        connection: &ConnectionTo<Client>,
    ) -> agent_client_protocol::Result<()> {
        let id = request.session_id.to_string();
        let (message, images) = match content::pi_prompt(&request.prompt) {
            Ok(prompt) => prompt,
            Err(error) => return responder.respond_with_error(invalid(error)),
        };
        match self.start_pi(&id, connection) {
            Ok(true) => self.publish_description(&id, connection),
            Ok(false) => {}
            Err(error) => return responder.respond_with_error(error),
        }
        let text = content::prompt_text(&request.prompt);
        let mut open = match self.require(&id) {
            Ok(open) => open,
            Err(error) => return responder.respond_with_error(error),
        };
        let session = open_session(&mut open);
        let message_id = Uuid::new_v4().to_string();
        let pi = session.pi.as_ref().expect("start_pi started pi");
        if let Err(responder) = pi.prompt(
            &message_id,
            request.prompt.clone(),
            &message,
            &images,
            responder,
        ) {
            return responder.respond_with_error(Error::new(-32603, "pi exited"));
        }
        if !session.titled
            && let Some(title) = acp_bridge::title(&text)
        {
            session.titled = true;
            drop(open);
            notify(
                connection,
                &id,
                SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title(title)),
            );
        }
        Ok(())
    }

    /// Opens the session and starts its Pi, which opens the Pi session.
    /// Replay and the current state precede the response, as on the live
    /// update path.
    fn resume_session(
        self: &Arc<Self>,
        request: acp::ResumeSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> impl Future<Output = Result<acp::ResumeSessionResponse, Error>> + Send + 'static {
        let prepared = (|| {
            let replay = match request.replay_from {
                None => false,
                Some(acp::ReplayFrom::Start(_)) => true,
                Some(_) => return Err(invalid("Unsupported replay cursor")),
            };
            let cwd = acp_bridge::cwd(&request.cwd.0).map_err(invalid)?;
            let mcp_servers = acp_bridge::parse_mcp(request.mcp_servers)?;
            check_pi_names(&mcp_servers)?;
            let instructions = acp_bridge::system_prompt(request.system_prompt)?;
            let id = request.session_id.to_string();
            if Uuid::parse_str(&id).is_err() {
                return Err(acp::Error::session_not_found(id).into());
            }
            {
                let mut open = self.session.lock().unwrap();
                if open.is_some() {
                    return Err(acp_bridge::already_open());
                }
                // Whether the session has had a prompt is known once Pi
                // names its session file.
                *open = Some(Session {
                    id: id.clone(),
                    cwd: cwd.clone(),
                    instructions,
                    mcp_servers,
                    titled: true,
                    pi: None,
                });
            }
            self.start_pi(&id, connection)?;
            let description = self.describe(&id);
            Ok((id, cwd, replay, description))
        })();
        let bridge = self.clone();
        let connection = connection.clone();
        async move {
            let (id, cwd, replay, description) = prepared?;
            let description = description.await?;
            let file = description.state["sessionFile"]
                .as_str()
                .map(std::path::Path::new);
            if replay && let Some(file) = file {
                history::replay(file, &cwd, &Updates::new(connection.clone(), &id))
                    .map_err(|error| Error::new(-32603, error.to_string()))?;
            }
            let mut open = bridge.require(&id)?;
            let session = open_session(&mut open);
            session.titled = file.is_some_and(history::has_prompts);
            notify(
                &connection,
                &id,
                SessionUpdate::StateUpdate(session.pi.as_ref().expect("started pi").snapshot()),
            );
            Ok(acp::ResumeSessionResponse::new()
                .config_options(description.config_options)
                .available_commands(description.available_commands))
        }
    }

    fn set_config(
        self: &Arc<Self>,
        request: &acp::SetSessionConfigOptionRequest,
        connection: ConnectionTo<Client>,
    ) -> impl Future<Output = Result<acp::SetSessionConfigOptionResponse, Error>> + Send + 'static
    {
        let id = request.session_id.to_string();
        let config_id = request.config_id.to_string();
        let mut effort = None;
        let change = (|| {
            drop(self.require(&id)?);
            let acp::SessionConfigOptionValue::Id { value } = &request.value else {
                return Err(invalid("Expected a select value"));
            };
            let value = value.to_string();
            let command = options::command(&config_id, &value)
                .ok_or_else(|| invalid("Unknown configuration option or value"))?;
            if config_id == "effort" {
                effort = Some(value);
            }
            if self.start_pi(&id, &connection)? {
                self.publish_description(&id, &connection);
            }
            let mut open = self.require(&id)?;
            let pi = open_session(&mut open).pi.as_ref().expect("started pi");
            Ok(pi.change(command))
        })();
        async move {
            let state = answer(change?).await?;
            let config_options = options::build(&state);
            notify(
                &connection,
                &id,
                SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(
                    config_options.clone(),
                )),
            );
            if let Some(effort) = effort {
                options::check_effort(&state, &effort).map_err(invalid)?;
            }
            Ok(acp::SetSessionConfigOptionResponse::new(config_options))
        }
    }
}

impl Bridge {
    /// Opens a new session, without its Pi, and returns its ID.
    fn open_new(&self, request: acp::NewSessionRequest) -> Result<String, Error> {
        let cwd = acp_bridge::cwd(&request.cwd.0).map_err(invalid)?;
        let mcp_servers = acp_bridge::parse_mcp(request.mcp_servers)?;
        check_pi_names(&mcp_servers)?;
        let instructions = acp_bridge::system_prompt(request.system_prompt)?;
        let mut open = self.session.lock().unwrap();
        if open.is_some() {
            return Err(acp_bridge::already_open());
        }
        let id = Uuid::new_v4().to_string();
        *open = Some(Session {
            id: id.clone(),
            cwd,
            instructions,
            mcp_servers,
            titled: false,
            pi: None,
        });
        Ok(id)
    }

    /// Sends all description queries to the session's Pi before returning,
    /// so later requests do not overtake their submission to Pi.
    fn describe(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Description, Error>> + Send + 'static + use<> {
        let description = self
            .require(id)
            .map(|mut open| description(open_session(&mut open).pi.as_ref().expect("started pi")));
        async move { description?.await }
    }

    fn publish_description(&self, id: &str, connection: &ConnectionTo<Client>) {
        let description = self.describe(id);
        let id = id.to_owned();
        let connection = connection.clone();
        tokio::spawn(async move {
            match description.await {
                Ok(description) => {
                    notify(
                        &connection,
                        &id,
                        SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(
                            description.config_options,
                        )),
                    );
                    notify(
                        &connection,
                        &id,
                        SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
                            description.available_commands,
                        )),
                    );
                }
                Err(error) => {
                    eprintln!("failed to describe pi for session {id}: {}", error.message);
                }
            }
        });
    }

    /// Starts the session's Pi unless one is running.
    fn start_pi(&self, id: &str, connection: &ConnectionTo<Client>) -> Result<bool, Error> {
        let mut open = self.require(id)?;
        let session = open_session(&mut open);
        if session.pi.as_ref().is_some_and(|pi| !pi.has_exited()) {
            return Ok(false);
        }
        let pi = Pi::start(
            &session.cwd,
            id,
            session.instructions.as_deref(),
            &session.mcp_servers,
            Updates::new(connection.clone(), id),
        )
        .map_err(|error| Error::new(-32603, error))?;
        session.pi = Some(pi);
        Ok(true)
    }

    /// The open session, when its ID is `id`.
    fn require(&self, id: &str) -> Result<MutexGuard<'_, Option<Session>>, Error> {
        let open = self.session.lock().unwrap();
        if open.as_ref().is_some_and(|session| session.id == id) {
            Ok(open)
        } else {
            Err(acp::Error::session_not_found(id.to_owned()).into())
        }
    }
}

/// Sends `pi` the description queries and returns the future of their
/// answers.
fn description(
    pi: &Pi,
) -> impl Future<Output = Result<Description, Error>> + Send + 'static + use<> {
    let queries = [
        pi.query(json!({"type":"get_state"})),
        pi.query(json!({"type":"get_commands"})),
    ];
    async move {
        let [state, catalog] = queries;
        let state = answer(state).await?;
        let catalog = answer(catalog).await?;
        Ok(Description {
            config_options: options::build(&state),
            available_commands: commands::catalog(&catalog["commands"]),
            state,
        })
    }
}

/// Refuses MCP server names Pi's `registerMcpServer` rejects: an empty name,
/// one with a character other than letters, digits, `_` and `-`, and one
/// matching an earlier name once `-` is read as `_`.
fn check_pi_names(servers: &[McpServer]) -> Result<(), Error> {
    let mut seen = Vec::new();
    for server in servers {
        let name = &server.name;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(invalid(format!("invalid MCP server name: {name}")));
        }
        let key = name.replace('-', "_");
        if seen.contains(&key) {
            return Err(invalid(format!("duplicate MCP server name: {name}")));
        }
        seen.push(key);
    }
    Ok(())
}

/// The session `require` found open.
fn open_session<'a>(open: &'a mut MutexGuard<'_, Option<Session>>) -> &'a mut Session {
    open.as_mut().expect("require found the session open")
}

async fn answer(response: oneshot::Receiver<Result<Value, String>>) -> Result<Value, Error> {
    response
        .await
        .map_err(|_| Error::new(-32603, "pi exited"))?
        .map_err(|error| Error::new(-32603, error))
}
