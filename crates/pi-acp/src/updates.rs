use std::{collections::HashSet, path::Path, sync::Mutex};

use agent_client_protocol::Client;
use agent_client_protocol::v2::ConnectionTo;
use agent_client_protocol::v2::schema::{
    self as acp, ContentBlock, ContentChunk, SessionUpdate, StateUpdate, StopReason, TextContent,
    ToolCallContent, ToolCallStatus, UpdateSessionNotification,
};
use serde_json::Value;

use crate::tools;

#[derive(Default)]
struct State {
    /// Pi is working on a turn, as the client was last told.
    working: bool,
    /// Tool calls that have opened and not ended.
    open: HashSet<String>,
}

/// Sends one session's updates to the client.
pub struct Updates {
    connection: ConnectionTo<Client>,
    session_id: String,
    state: Mutex<State>,
}

impl Updates {
    pub fn new(connection: ConnectionTo<Client>, session_id: &str) -> Self {
        Self {
            connection,
            session_id: session_id.to_owned(),
            state: Mutex::new(State::default()),
        }
    }

    fn send(&self, update: SessionUpdate) {
        notify(&self.connection, &self.session_id, update);
    }

    /// Tells the client that Pi started working, if it has not said so.
    pub fn working(&self) {
        let mut state = self.state.lock().unwrap();
        if !state.working {
            state.working = true;
            self.send(SessionUpdate::StateUpdate(state_update(true, None)));
        }
    }

    /// Tells the client that Pi stopped working, with why.
    pub fn idle(&self, stop_reason: StopReason) {
        let mut state = self.state.lock().unwrap();
        if state.working {
            state.working = false;
            self.send(SessionUpdate::StateUpdate(state_update(
                false,
                Some(stop_reason),
            )));
        }
    }

    /// Sends the user message a prompt Pi accepted carries into the
    /// conversation.
    pub fn user_message(&self, message_id: &str, content: Vec<ContentBlock>) {
        self.send(SessionUpdate::UserMessage(
            acp::UserMessage::new(message_id).content(content),
        ));
    }

    /// Sends a piece of the message Pi is streaming.
    pub fn text_chunk(&self, message_id: &str, text: String) {
        self.send(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
            message_id,
        )));
    }

    /// Sends a piece of the thought Pi is streaming.
    pub fn thought_chunk(&self, message_id: &str, text: String) {
        self.send(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
            message_id,
        )));
    }

    /// Sends a whole message.
    pub fn message(&self, message_id: &str, text: String) {
        self.send(SessionUpdate::AgentMessage(
            acp::AgentMessage::new(message_id)
                .content(vec![ContentBlock::Text(TextContent::new(text))]),
        ));
    }

    /// Sends a whole thought.
    pub fn thought(&self, message_id: &str, text: String) {
        self.send(SessionUpdate::AgentThought(
            acp::AgentThought::new(message_id)
                .content(vec![ContentBlock::Text(TextContent::new(text))]),
        ));
    }

    /// Opens a tool call Pi's assistant message makes.
    pub fn open_tool(&self, id: &str, name: &str, args: &Value, cwd: &Path) {
        self.state.lock().unwrap().open.insert(id.to_owned());
        self.send(SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(id.to_owned())
                .title(tools::title(name, args))
                .name(name.to_owned())
                .kind(tools::kind(name))
                .status(ToolCallStatus::Pending)
                .raw_input(args.clone())
                .locations(tools::locations(name, args, cwd)),
        ));
    }

    pub fn tool_started(&self, id: &str) {
        self.send(tool_status(id.to_owned(), ToolCallStatus::InProgress));
    }

    /// Ends a tool call with its whole content.
    pub fn tool_ended(&self, id: &str, failed: bool, content: Vec<ToolCallContent>) {
        self.state.lock().unwrap().open.remove(id);
        let status = if failed {
            ToolCallStatus::Failed
        } else {
            ToolCallStatus::Completed
        };
        self.send(SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(id.to_owned())
                .status(status)
                .content(content),
        ));
    }

    /// Marks every tool call that opened without ending as failed.
    pub fn fail_open_tools(&self) {
        let open = std::mem::take(&mut self.state.lock().unwrap().open);
        for id in open {
            self.send(tool_status(id, ToolCallStatus::Failed));
        }
    }
}

fn tool_status(id: String, status: ToolCallStatus) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(id).status(status))
}

/// A running or idle state. Pi has no background work, so the session can
/// park whenever it is idle.
fn state_update(running: bool, stop_reason: Option<StopReason>) -> StateUpdate {
    if running {
        StateUpdate::Running(acp::RunningStateUpdate::new().background(Some(Vec::new())))
    } else {
        StateUpdate::Idle(
            acp::IdleStateUpdate::new()
                .stop_reason(stop_reason)
                .background(Some(Vec::new())),
        )
    }
}

/// The state of a session that is idle with nothing running.
pub fn idle_state() -> StateUpdate {
    state_update(false, None)
}

pub fn notify(connection: &ConnectionTo<Client>, id: &str, update: SessionUpdate) {
    if let Err(error) =
        connection.send_notification(UpdateSessionNotification::new(id.to_owned(), update))
    {
        eprintln!("failed to send session update: {error}");
    }
}
