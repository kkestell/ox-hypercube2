use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_client_protocol::v2::schema::{self as acp, SessionUpdate};
use agent_client_protocol::v2::{ConnectionTo, Responder};
use agent_client_protocol::{Client, Error};
use uuid::Uuid;

use crate::{
    content,
    pi::Pi,
    updates::{Updates, notify},
    validation,
};

/// A session opened on this connection.
struct Session {
    cwd: PathBuf,
    /// The Pi session every Pi of this session opens.
    pi_session_id: String,
    title: Option<String>,
    /// The session's Pi, once started.
    pi: Option<Pi>,
}

pub struct Bridge {
    sessions: Mutex<HashMap<String, Session>>,
}

impl Bridge {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(HashMap::new()),
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
        self.sessions.lock().unwrap().insert(
            id.clone(),
            Session {
                cwd,
                pi_session_id: Uuid::new_v4().to_string(),
                title: None,
                pi: None,
            },
        );
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
        let message_id = Uuid::new_v4().to_string();
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions.get_mut(&id).expect("start_pi found the session");
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

        let text = content::prompt_text(&request.prompt);
        if session.title.is_none() && !text.trim().is_empty() {
            let title: String = text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(120)
                .collect();
            session.title = Some(title.clone());
            drop(sessions);
            notify(
                connection,
                &id,
                SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title(title)),
            );
        }
        Ok(())
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
            &session.cwd,
            &session.pi_session_id,
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
