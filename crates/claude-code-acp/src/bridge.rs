use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use acp_bridge::{McpServer, invalid, notify, parse_mcp};
use agent_client_protocol::v2::schema::{
    self as acp, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SessionUpdate,
};
use agent_client_protocol::v2::{ConnectionTo, Responder};
use agent_client_protocol::{Client, Error};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    content,
    history::History,
    runner::{Claude, StartRequest},
    updates::{self, Updates},
};

/// A config option to change.
enum Setting {
    Mode(String),
    Model(String),
    Effort(String),
}

/// A config change sent to its session's claude.
struct ConfigChange {
    id: String,
    setting: Setting,
    /// The receiver of claude's answer, when the session has a claude.
    accepted: Option<oneshot::Receiver<Result<(), String>>>,
}

/// The session a bridge process serves. Its ID is also its Claude session
/// ID, and Claude keeps its transcript. The client sends its working
/// directory, MCP servers and system prompt whenever it opens the session,
/// and sets its options after.
struct Session {
    id: String,
    cwd: PathBuf,
    mcp_servers: Vec<McpServer>,
    /// The client's system prompt, appended to Claude's.
    instructions: Option<String>,
    mode: String,
    model: String,
    effort: String,
    /// The session has had a prompt, which titled it.
    titled: bool,
    claude: Option<Claude>,
}

impl Session {
    fn new(
        id: String,
        cwd: PathBuf,
        mcp_servers: Vec<McpServer>,
        instructions: Option<String>,
        titled: bool,
    ) -> Self {
        Self {
            id,
            cwd,
            mcp_servers,
            instructions,
            mode: "manual".into(),
            model: "claude-opus-5-5".into(),
            effort: "high".into(),
            titled,
            claude: None,
        }
    }

    fn options(&self) -> Vec<SessionConfigOption> {
        vec![
            SessionConfigOption::select("model", "Model", self.model.clone(), model_options())
                .category(SessionConfigOptionCategory::Model),
            SessionConfigOption::select("effort", "Effort", self.effort.clone(), effort_options())
                .category(SessionConfigOptionCategory::ThoughtLevel),
            SessionConfigOption::select("mode", "Mode", self.mode.clone(), mode_options())
                .category(SessionConfigOptionCategory::Mode),
        ]
    }
}

/// A bridge process serves one session, opened with `session/new` or
/// `session/resume`.
pub struct Bridge {
    history: History,
    session: Mutex<Option<Session>>,
}

impl acp_bridge::Bridge for Bridge {
    fn initialize(&self, request: &acp::InitializeRequest) -> acp::InitializeResponse {
        acp::InitializeResponse::new(
            request.protocol_version,
            acp::Implementation::new("claude-code-acp", env!("CARGO_PKG_VERSION"))
                .title("Claude Code ACP Bridge"),
        )
        .capabilities(
            acp::AgentCapabilities::new().session(
                acp::SessionCapabilities::new()
                    .prompt(
                        acp::PromptCapabilities::new()
                            .image(acp::PromptImageCapabilities::new())
                            .embedded_context(acp::PromptEmbeddedContextCapabilities::new()),
                    )
                    .mcp(acp::McpCapabilities::new().stdio(acp::McpStdioCapabilities::new()))
                    .system_prompt(acp::SessionSystemPromptCapabilities::new()),
            ),
        )
    }

    /// Opens a new session without starting its claude: the client learns
    /// the session first, then `start_new` starts claude, whose commands
    /// follow as an update.
    fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> std::result::Result<acp::NewSessionResponse, Error> {
        let cwd = acp_bridge::cwd(&request.cwd.0).map_err(invalid)?;
        let servers = parse_mcp(request.mcp_servers)?;
        let instructions = acp_bridge::system_prompt(request.system_prompt)?;
        let mut open = self.session.lock().unwrap();
        if open.is_some() {
            return Err(acp_bridge::already_open());
        }
        let session = Session::new(
            Uuid::new_v4().to_string(),
            cwd,
            servers,
            instructions,
            false,
        );
        let response =
            acp::NewSessionResponse::new(session.id.clone()).config_options(session.options());
        *open = Some(session);
        Ok(response)
    }

    /// Starts a new session's claude once the client knows the session. A
    /// claude that fails to start fails the session's first prompt.
    fn start_new(self: &Arc<Self>, id: &str, connection: &ConnectionTo<Client>) {
        notify(
            connection,
            id,
            SessionUpdate::StateUpdate(updates::idle_state()),
        );
        let started = self
            .require(id)
            .and_then(|mut open| self.start_claude(open_session(&mut open), connection));
        if let Err(error) = started {
            eprintln!("failed to start claude for session {id}: {error}");
        }
    }

    /// Opens the session and starts its claude, which resumes the Claude
    /// session when it has a transcript. Replay and the current state
    /// precede the response, as on the live update path.
    fn resume_session(
        self: &Arc<Self>,
        request: acp::ResumeSessionRequest,
        connection: &ConnectionTo<Client>,
    ) -> impl Future<Output = std::result::Result<acp::ResumeSessionResponse, Error>> + Send + 'static
    {
        let prepared = (|| {
            let replay = match request.replay_from {
                None => false,
                Some(acp::ReplayFrom::Start(_)) => true,
                Some(_) => return Err(invalid("Unsupported replay cursor")),
            };
            let id = request.session_id.to_string();
            let cwd = acp_bridge::cwd(&request.cwd.0).map_err(invalid)?;
            let servers = parse_mcp(request.mcp_servers)?;
            let instructions = acp_bridge::system_prompt(request.system_prompt)?;
            if Uuid::parse_str(&id).is_err() {
                return Err(acp::Error::session_not_found(id).into());
            }
            let mut open = self.session.lock().unwrap();
            if open.is_some() {
                return Err(acp_bridge::already_open());
            }
            let titled = self.history.find(&id, &cwd);
            let session = open.insert(Session::new(
                id.clone(),
                cwd.clone(),
                servers,
                instructions,
                titled,
            ));
            self.start_claude(session, connection)?;
            Ok((id, cwd, replay))
        })();
        let bridge = self.clone();
        let connection = connection.clone();
        async move {
            let (id, cwd, replay) = prepared?;
            if replay {
                Updates::new(connection.clone(), &id).replay(
                    bridge.history.messages(&id, &cwd),
                    bridge.history.subagents(&id, &cwd),
                );
            }
            let mut open = bridge.require(&id)?;
            let session = open_session(&mut open);
            let claude = session
                .claude
                .as_ref()
                .expect("an open session has its claude");
            notify(&connection, &id, SessionUpdate::StateUpdate(claude.state()));
            Ok(acp::ResumeSessionResponse::new()
                .config_options(session.options())
                .available_commands(claude.commands()))
        }
    }

    /// Sends a config change to its session's claude and returns the future
    /// that applies it once claude accepts it. The change is sent before the
    /// future is first polled, so it reaches claude ahead of a prompt that
    /// follows it.
    fn set_config(
        self: &Arc<Self>,
        request: &acp::SetSessionConfigOptionRequest,
        connection: ConnectionTo<Client>,
    ) -> impl Future<Output = std::result::Result<acp::SetSessionConfigOptionResponse, Error>>
    + Send
    + 'static {
        let change = self.send_config(request);
        let bridge = self.clone();
        async move { bridge.apply_config(change?, &connection).await }
    }

    /// Interrupts the session's turn, whether a prompt or claude started it.
    fn cancel(&self, id: &str) {
        if let Ok(mut open) = self.require(id)
            && let Some(claude) = &open_session(&mut open).claude
        {
            claude.interrupt();
        }
    }

    /// Sends a prompt to its session's claude, which answers `responder`
    /// once it takes the prompt into a turn. The prompt is sent before this
    /// returns, so a cancel that arrives right after the prompt drops it.
    fn prompt(
        self: &Arc<Self>,
        request: &acp::PromptRequest,
        responder: Responder<acp::PromptResponse>,
        connection: &ConnectionTo<Client>,
    ) -> agent_client_protocol::Result<()> {
        let id = request.session_id.to_string();
        let mut open = match self.require(&id) {
            Ok(open) => open,
            Err(error) => return responder.respond_with_error(error),
        };
        let session = open_session(&mut open);
        let content = match content::claude_content(&request.prompt) {
            Ok(content) => content,
            Err(error) => return responder.respond_with_error(invalid(error)),
        };
        let text = content::prompt_text(&request.prompt);
        let message_id = Uuid::new_v4().to_string();

        // The initialization request precedes this prompt on stdin, including
        // when a native process that exited has to be restarted. Enqueue the
        // prompt before returning so a subsequent cancellation stays after it.
        if let Err(error) = self.start_claude(session, connection) {
            return responder.respond_with_error(error);
        }
        let claude = session
            .claude
            .as_ref()
            .expect("start_claude installed the process");
        if let Err(responder) =
            claude.prompt(&message_id, request.prompt.clone(), &content, responder)
        {
            return responder.respond_with_error(Error::new(-32603, "claude exited"));
        }

        if !session.titled
            && let Some(title) = acp_bridge::title(&text)
        {
            session.titled = true;
            notify(
                connection,
                &id,
                SessionUpdate::SessionInfoUpdate(acp::SessionInfoUpdate::new().title(title)),
            );
        }
        Ok(())
    }
}

impl Bridge {
    pub fn new(claude_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            history: History::new(claude_dir),
            session: Mutex::new(None),
        })
    }

    fn send_config(
        &self,
        request: &acp::SetSessionConfigOptionRequest,
    ) -> std::result::Result<ConfigChange, Error> {
        let id = request.session_id.to_string();
        let mut open = self.require(&id)?;
        let claude = open_session(&mut open).claude.as_ref();
        let acp::SessionConfigOptionValue::Id { value } = &request.value else {
            return Err(invalid("Expected a select value"));
        };
        let value = value.to_string();
        let (setting, accepted) = match request.config_id.to_string().as_str() {
            "mode" if is_mode(&value) => {
                let accepted = claude.map(|claude| claude.set_permission_mode(&value));
                (Setting::Mode(value), accepted)
            }
            "model" if is_model(&value) => {
                let accepted = claude.map(|claude| claude.set_model(&value));
                (Setting::Model(value), accepted)
            }
            "effort" if is_effort(&value) => {
                let accepted = claude.map(|claude| claude.set_effort(&value));
                (Setting::Effort(value), accepted)
            }
            _ => return Err(invalid("Unknown configuration option or value")),
        };
        Ok(ConfigChange {
            id,
            setting,
            accepted,
        })
    }

    async fn apply_config(
        &self,
        change: ConfigChange,
        connection: &ConnectionTo<Client>,
    ) -> std::result::Result<acp::SetSessionConfigOptionResponse, Error> {
        let ConfigChange {
            id,
            setting,
            accepted,
        } = change;
        // A claude that exits before it answers leaves the change to the
        // next claude, which starts with the session's options.
        if let Some(Ok(Err(error))) = match accepted {
            Some(accepted) => Some(accepted.await),
            None => None,
        } {
            return Err(Error::new(-32603, error));
        }
        let options = {
            let mut open = self.require(&id)?;
            let session = open_session(&mut open);
            match setting {
                Setting::Mode(mode) => session.mode = mode,
                Setting::Model(model) => session.model = model,
                Setting::Effort(effort) => session.effort = effort,
            }
            session.options()
        };
        notify(
            connection,
            &id,
            SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(options.clone())),
        );
        Ok(acp::SetSessionConfigOptionResponse::new(options))
    }

    /// Starts the session's one native process if needed and sends initialize
    /// before any prompt or config change can be queued on its input channel.
    fn start_claude(
        &self,
        session: &mut Session,
        connection: &ConnectionTo<Client>,
    ) -> std::result::Result<(), Error> {
        if session
            .claude
            .as_ref()
            .is_some_and(|claude| !claude.has_exited())
        {
            return Ok(());
        }
        let transcript = self.history.entries(&session.id, &session.cwd);
        let claude = Claude::start(
            &StartRequest {
                cwd: &session.cwd,
                session_id: &session.id,
                resume: self.history.find(&session.id, &session.cwd),
                model: &session.model,
                effort: &session.effort,
                mode: &session.mode,
                mcp_servers: &session.mcp_servers,
                instructions: session.instructions.as_deref(),
                transcript: &transcript,
            },
            Updates::new(connection.clone(), &session.id),
        )
        .map_err(|error| Error::new(-32603, error))?;
        session.claude = Some(claude);
        Ok(())
    }

    /// The open session, when its ID is `id`.
    fn require(&self, id: &str) -> std::result::Result<MutexGuard<'_, Option<Session>>, Error> {
        let open = self.session.lock().unwrap();
        if open.as_ref().is_some_and(|session| session.id == id) {
            Ok(open)
        } else {
            Err(acp::Error::session_not_found(id.to_owned()).into())
        }
    }
}

/// The session `require` found open.
fn open_session<'a>(open: &'a mut MutexGuard<'_, Option<Session>>) -> &'a mut Session {
    open.as_mut().expect("require found the session open")
}

/// The models a session can use, by id and name.
const MODELS: [(&str, &str); 4] = [
    ("claude-fable-5-2", "Fable 5.2"),
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-haiku-5-5", "Haiku 5.5"),
];

fn is_model(value: &str) -> bool {
    MODELS.iter().any(|(id, _)| *id == value)
}

fn model_options() -> Vec<SessionConfigSelectOption> {
    MODELS
        .into_iter()
        .map(|(id, name)| SessionConfigSelectOption::new(id, name))
        .collect()
}

/// The effort levels a session can use, by id and name.
const EFFORTS: [(&str, &str); 4] = [
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "XHigh"),
];

fn is_effort(value: &str) -> bool {
    EFFORTS.iter().any(|(id, _)| *id == value)
}

fn effort_options() -> Vec<SessionConfigSelectOption> {
    EFFORTS
        .into_iter()
        .map(|(id, name)| SessionConfigSelectOption::new(id, name))
        .collect()
}

fn is_mode(value: &str) -> bool {
    ["auto", "manual", "acceptEdits", "plan"].contains(&value)
}

fn mode_options() -> Vec<SessionConfigSelectOption> {
    [
        ("auto", "Auto", "Let Claude assess actions automatically"),
        ("manual", "Manual", "Ask before edits and commands"),
        (
            "acceptEdits",
            "Accept edits",
            "Accept file edits automatically",
        ),
        ("plan", "Plan", "Explore and plan without editing files"),
    ]
    .into_iter()
    .map(|(id, name, description)| {
        SessionConfigSelectOption::new(id, name).description(description)
    })
    .collect()
}
