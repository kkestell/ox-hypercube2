use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

use agent_client_protocol::v2::schema::{self as acp, SessionUpdate};
use agent_client_protocol::v2::{ConnectionTo, Responder};
use agent_client_protocol::{Client, Error};
use uuid::Uuid;

use crate::{
    config::Config,
    content, history,
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

pub struct Bridge {
    sessions: Mutex<HashMap<String, Session>>,
    store: Store,
}

impl Bridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
            store: Store::new(Config::from_env().store_dir),
        })
    }

    pub fn initialize(request: &acp::InitializeRequest) -> acp::InitializeResponse {
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
        if let Err(error) = self.start_pi(id, connection) {
            eprintln!("failed to start pi for session {id}: {}", error.message);
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
        if let Err(error) = self.start_pi(&id, connection) {
            return responder.respond_with_error(error);
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
            let sessions = self.sessions.lock().unwrap();
            let pi = sessions[&id].pi.as_ref().expect("start_pi started pi");
            let path = replay.then(|| pi.session_file());
            Ok((id, cwd, path))
        })();
        let bridge = self.clone();
        let connection = connection.clone();
        async move {
            let (id, cwd, path) = prepared?;
            if let Some(path) = path {
                let path = path
                    .await
                    .map_err(|_| Error::new(-32603, "pi exited"))?
                    .map_err(|error| Error::new(-32603, error))?;
                if let Some(path) = path {
                    history::replay(&path, &cwd, &Updates::new(connection.clone(), &id))
                        .map_err(|error| store_error(&error))?;
                }
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
            Ok(acp::ResumeSessionResponse::new())
        }
    }

    /// Starts the session's Pi unless one is running.
    fn start_pi(&self, id: &str, connection: &ConnectionTo<Client>) -> Result<(), Error> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| Error::from(acp::Error::session_not_found(id.to_owned())))?;
        if session.pi.as_ref().is_some_and(|pi| !pi.has_exited()) {
            return Ok(());
        }
        let pi = Pi::start(
            &session.metadata.cwd,
            &session.metadata.pi_session_id,
            Updates::new(connection.clone(), id),
        )
        .map_err(|error| Error::new(-32603, error))?;
        session.pi = Some(pi);
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(-32602, message)
}

fn store_error(error: &std::io::Error) -> Error {
    Error::new(-32603, error.to_string())
}
