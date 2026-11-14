use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use agent_client_protocol::v2::schema::{self as acp, SessionUpdate};
use agent_client_protocol::v2::{ConnectionTo, Responder};
use agent_client_protocol::{Client, Error};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    commands,
    config::Config,
    content, history, options,
    pi::Pi,
    store::{Session as Metadata, Store},
    updates::{Updates, notify},
    validation,
};

/// A session opened on this connection.
struct Session {
    metadata: Metadata,
    /// The session's Pi, once started.
    pi: Option<Pi>,
}

struct Description {
    state: Value,
    config_options: Vec<acp::SessionConfigOption>,
    available_commands: Vec<acp::AvailableCommand>,
}

pub struct Bridge {
    sessions: Mutex<HashMap<String, Session>>,
    store: Store,
    forms: AtomicBool,
}

impl Bridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            forms: AtomicBool::new(false),
            store: Store::new(Config::from_env().store_dir),
        })
    }

    pub fn initialize(&self, request: &acp::InitializeRequest) -> acp::InitializeResponse {
        self.forms.store(
            request
                .capabilities
                .elicitation
                .as_ref()
                .is_some_and(acp::ElicitationCapabilities::supports_form),
            Ordering::Relaxed,
        );
        acp::InitializeResponse::new(
            request.protocol_version,
            acp::Implementation::new("pi-acp", env!("CARGO_PKG_VERSION")).title("Pi ACP Bridge"),
        )
        .capabilities(
            acp::AgentCapabilities::new().session(
                acp::SessionCapabilities::new().prompt(
                    acp::PromptCapabilities::new().image(acp::PromptImageCapabilities::new()),
                ),
            ),
        )
    }

    /// Opens a new session without starting its Pi: the client learns the
    /// session first, then `start_new` starts Pi.
    pub fn new_session(
        &self,
        request: &acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, Error> {
        let cwd = validation::cwd(&request.cwd.0).map_err(invalid)?;
        if !request.mcp_servers.is_empty() {
            return Err(invalid("MCP servers are not supported"));
        }
        let id = Uuid::new_v4().simple().to_string();
        let metadata = Metadata {
            cwd,
            pi_session_id: Uuid::new_v4().to_string(),
            title: None,
        };
        self.store
            .create(id.clone(), &metadata)
            .map_err(|error| store_error(&error))?;
        self.sessions
            .lock()
            .unwrap()
            .insert(id.clone(), Session { metadata, pi: None });
        Ok(acp::NewSessionResponse::new(id))
    }

    /// Starts a new session's Pi once the client knows the session. A Pi that
    /// fails to start is started again by the session's first prompt.
    pub fn start_new(&self, id: &str, connection: &ConnectionTo<Client>) {
        match self.start_pi(id, connection) {
            Ok(true) => self.publish_description(id, connection),
            Ok(false) => {}
            Err(error) => eprintln!("failed to start pi for session {id}: {}", error.message),
        }
    }

    /// Closes a session: its Pi stops and later requests for it fail. The
    /// returned future finishes once Pi has exited. A session that is not
    /// open closes without effect.
    pub fn close_session(&self, id: &str) -> impl Future<Output = ()> + Send + 'static {
        let pi = self
            .sessions
            .lock()
            .unwrap()
            .remove(id)
            .and_then(|session| session.pi);
        async move {
            if let Some(pi) = pi {
                pi.stop().await;
            }
        }
    }

    /// Stops the session's run, whether a prompt or Pi started it.
    pub fn cancel(&self, id: &str) {
        if let Some(pi) = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .and_then(|session| session.pi.as_ref())
        {
            pi.abort();
        }
    }

    /// Sends a prompt to its session's Pi, which answers `responder` once it
    /// accepts the prompt. The prompt is sent before this returns, so a
    /// cancel that arrives right after the prompt follows it.
    pub fn prompt(
        &self,
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
        // Save the title before submitting a prompt so registry errors can be
        // returned to its caller without accepting work first.
        let text = content::prompt_text(&request.prompt);
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions.get_mut(&id).expect("start_pi found the session");
        let title = if session.metadata.title.is_none() && !text.trim().is_empty() {
            Some(
                text.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(120)
                    .collect::<String>(),
            )
        } else {
            None
        };
        if let Some(title) = &title {
            if let Err(error) = self
                .store
                .update(&id, |metadata| metadata.title = Some(title.clone()))
            {
                return responder.respond_with_error(store_error(&error));
            }
            session.metadata.title = Some(title.clone());
        }
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

        if let Some(title) = title {
            drop(sessions);
            notify(
                connection,
                &id,
                SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title(title)),
            );
        }
        Ok(())
    }

    pub fn resume_session(
        self: &Arc<Self>,
        request: &acp::ResumeSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> impl Future<Output = Result<acp::ResumeSessionResponse, Error>> + Send + 'static {
        let prepared = (|| {
            let replay = match request.replay_from {
                None => false,
                Some(acp::ReplayFrom::Start(_)) => true,
                Some(_) => return Err(invalid("Unsupported replay cursor")),
            };
            let cwd = validation::cwd(&request.cwd.0).map_err(invalid)?;
            if !request.mcp_servers.is_empty() {
                return Err(invalid("MCP servers are not supported"));
            }
            let id = request.session_id.to_string();
            let metadata = self
                .store
                .get(&id)
                .map_err(|error| store_error(&error))?
                .ok_or_else(|| Error::from(acp::Error::session_not_found(id.clone())))?;
            if metadata.cwd != cwd {
                return Err(invalid("Session cwd does not match"));
            }
            self.sessions
                .lock()
                .unwrap()
                .entry(id.clone())
                .or_insert(Session { metadata, pi: None });
            self.start_pi(&id, connection)?;
            let description = self.describe(&id);
            Ok((id, cwd, replay, description))
        })();
        let bridge = self.clone();
        let connection = connection.clone();
        async move {
            let (id, cwd, replay, description) = prepared?;
            let description = description.await?;
            if replay && let Some(path) = description.state["sessionFile"].as_str() {
                history::replay(
                    std::path::Path::new(path),
                    &cwd,
                    &Updates::new(
                        connection.clone(),
                        &id,
                        bridge.forms.load(Ordering::Relaxed),
                    ),
                )
                .map_err(|error| store_error(&error))?;
            }
            let sessions = bridge.sessions.lock().unwrap();
            let session = sessions
                .get(&id)
                .ok_or_else(|| Error::from(acp::Error::session_not_found(id.clone())))?;
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

    /// Sends all description queries before returning, so later requests do
    /// not overtake their submission to Pi.
    fn describe(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Description, Error>> + Send + 'static + use<> {
        let queries = (|| {
            let sessions = self.sessions.lock().unwrap();
            let session = sessions
                .get(id)
                .ok_or_else(|| Error::from(acp::Error::session_not_found(id.to_owned())))?;
            let pi = session.pi.as_ref().expect("started pi");
            Ok::<_, Error>([
                pi.request(json!({"type":"get_state"})),
                pi.request(json!({"type":"get_available_models"})),
                pi.request(json!({"type":"get_available_thinking_levels"})),
                pi.request(json!({"type":"get_commands"})),
            ])
        })();
        async move {
            let [state, models, levels, catalog] = queries?;
            let state = answer(state).await?;
            let models = answer(models).await?;
            let levels = answer(levels).await?;
            let catalog = answer(catalog).await?;
            Ok(Description {
                config_options: options::build(&state, &models["models"], &levels["levels"]),
                available_commands: commands::catalog(&catalog["commands"]),
                state,
            })
        }
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

    /// Sends the change before returning its completion future.
    pub fn set_config(
        self: &Arc<Self>,
        request: &acp::SetSessionConfigOptionRequest,
        connection: ConnectionTo<Client>,
    ) -> impl Future<Output = Result<acp::SetSessionConfigOptionResponse, Error>> + Send + 'static
    {
        let id = request.session_id.to_string();
        let change = (|| {
            if !self.sessions.lock().unwrap().contains_key(&id) {
                return Err(Error::from(acp::Error::session_not_found(id.clone())));
            }
            let acp::SessionConfigOptionValue::Id { value } = &request.value else {
                return Err(invalid("Expected a select value"));
            };
            let command = options::command(&request.config_id.to_string(), &value.to_string())
                .ok_or_else(|| invalid("Unknown configuration option or value"))?;
            if self.start_pi(&id, &connection)? {
                self.publish_description(&id, &connection);
            }
            let sessions = self.sessions.lock().unwrap();
            let pi = sessions[&id].pi.as_ref().expect("started pi");
            // TODO: Pi handles RPC lines concurrently; a following prompt can
            // start on the previous model while set_model awaits authentication.
            Ok(pi.request(command))
        })();
        let bridge = self.clone();
        async move {
            answer(change?).await?;
            let description = bridge.describe(&id).await?;
            notify(
                &connection,
                &id,
                SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(
                    description.config_options.clone(),
                )),
            );
            Ok(acp::SetSessionConfigOptionResponse::new(
                description.config_options,
            ))
        }
    }

    /// Starts the session's Pi unless one is running.
    fn start_pi(&self, id: &str, connection: &ConnectionTo<Client>) -> Result<bool, Error> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| Error::from(acp::Error::session_not_found(id.to_owned())))?;
        if session.pi.as_ref().is_some_and(|pi| !pi.has_exited()) {
            return Ok(false);
        }
        let pi = Pi::start(
            &session.metadata.cwd,
            &session.metadata.pi_session_id,
            Updates::new(connection.clone(), id, self.forms.load(Ordering::Relaxed)),
        )
        .map_err(|error| Error::new(-32603, error))?;
        session.pi = Some(pi);
        Ok(true)
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(-32602, message)
}

fn store_error(error: &std::io::Error) -> Error {
    Error::new(-32603, error.to_string())
}

async fn answer(response: oneshot::Receiver<Result<Value, String>>) -> Result<Value, Error> {
    response
        .await
        .map_err(|_| Error::new(-32603, "pi exited"))?
        .map_err(|error| Error::new(-32603, error))
}
