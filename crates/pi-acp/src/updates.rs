use std::{
    collections::HashSet,
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
};

use acp_bridge::notify;
use agent_client_protocol::Client;
use agent_client_protocol::v2::ConnectionTo;
use agent_client_protocol::v2::schema::{
    self as acp, ContentBlock, ContentChunk, SessionUpdate, StateUpdate, StopReason, TextContent,
    ToolCallContent, ToolCallStatus,
};
use serde_json::Value;

use crate::tools;

#[derive(Default)]
struct State {
    /// Pi is working on a turn, as the client was last told.
    working: bool,
    asking: usize,
    sent: Sent,
    /// Tool calls that have opened and not ended.
    open: HashSet<String>,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum Sent {
    #[default]
    Idle,
    Running,
    RequiresAction,
}

/// Sends one session's updates to the client.
#[derive(Clone)]
pub struct Updates {
    connection: ConnectionTo<Client>,
    session_id: String,
    state: Arc<Mutex<State>>,
}

impl Updates {
    pub fn new(connection: ConnectionTo<Client>, session_id: &str) -> Self {
        Self {
            connection,
            session_id: session_id.to_owned(),
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    fn send(&self, update: SessionUpdate) {
        notify(&self.connection, &self.session_id, update);
    }

    pub fn snapshot(&self) -> StateUpdate {
        state_update(Self::current(&self.state.lock().unwrap()), None)
    }

    fn current(state: &State) -> Sent {
        if !state.working {
            Sent::Idle
        } else if state.asking > 0 {
            Sent::RequiresAction
        } else {
            Sent::Running
        }
    }

    fn publish(&self, state: &mut State, stop_reason: Option<StopReason>) {
        let current = Self::current(state);
        if current != state.sent {
            state.sent = current;
            self.send(SessionUpdate::StateUpdate(state_update(
                current,
                stop_reason,
            )));
        }
    }

    pub fn working(&self) {
        let mut state = self.state.lock().unwrap();
        state.working = true;
        self.publish(&mut state, None);
    }

    pub fn idle(&self, stop_reason: StopReason) {
        let mut state = self.state.lock().unwrap();
        state.working = false;
        self.publish(&mut state, Some(stop_reason));
    }

    pub fn ask(
        &self,
        message: String,
        schema: acp::ElicitationSchema,
    ) -> impl Future<Output = Option<acp::ElicitationAction>> + Send + 'static {
        let updates = self.clone();
        async move {
            let _asking = Asking::new(updates.clone());
            updates
                .connection
                .send_request(acp::CreateElicitationRequest::new(
                    acp::ElicitationFormMode::new(
                        acp::ElicitationSessionScope::new(updates.session_id.clone()),
                        schema,
                    ),
                    message,
                ))
                .block_task()
                .await
                .ok()
                .map(|response| response.action)
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

/// Holds the requires-action state for the life of a client request.
struct Asking(Updates);

impl Asking {
    fn new(updates: Updates) -> Self {
        {
            let mut state = updates.state.lock().unwrap();
            state.asking += 1;
            updates.publish(&mut state, None);
        }
        Self(updates)
    }
}

impl Drop for Asking {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.asking -= 1;
        self.0.publish(&mut state, None);
    }
}

/// Pi has no background work, so an idle session can park.
fn state_update(state: Sent, stop_reason: Option<StopReason>) -> StateUpdate {
    match state {
        Sent::Idle => StateUpdate::Idle(
            acp::IdleStateUpdate::new()
                .stop_reason(stop_reason)
                .background(Some(Vec::new())),
        ),
        Sent::Running => {
            StateUpdate::Running(acp::RunningStateUpdate::new().background(Some(Vec::new())))
        }
        Sent::RequiresAction => StateUpdate::RequiresAction(
            acp::RequiresActionStateUpdate::new().background(Some(Vec::new())),
        ),
    }
}

/// The state of a session that is idle with nothing running.
pub fn idle_state() -> StateUpdate {
    state_update(Sent::Idle, None)
}
