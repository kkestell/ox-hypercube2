use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    Error,
    v2::{
        Responder,
        schema::{self as acp, ContentBlock, ErrorStopReason, PromptResponse, StopReason},
    },
};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    task::AbortHandle,
    time::sleep,
};

use crate::{dialogs, updates::Updates};

type Reply = oneshot::Sender<Result<Value, String>>;

/// The response and message to send when Pi takes in a prompt.
pub(super) struct Prompt {
    pub message_id: String,
    pub content: Vec<ContentBlock>,
    pub responder: Responder<PromptResponse>,
}

impl Prompt {
    fn accept(self, updates: &Updates) {
        let _ = self
            .responder
            .respond(PromptResponse::new(self.message_id.clone()));
        updates.user_message(&self.message_id, self.content);
        updates.working();
    }

    fn fail(self, error: Error) {
        let _ = self.responder.respond_with_error(error);
    }
}

enum Ordered {
    Prompt(Box<Prompt>),
    Change(Reply),
}

enum Pending {
    Ordered(Ordered),
    Query(Reply),
    Stop(&'static str),
}

impl Pending {
    fn fail(self, error: &str) {
        match self {
            Self::Ordered(Ordered::Prompt(prompt)) => prompt.fail(Error::new(-32603, error)),
            Self::Ordered(Ordered::Change(reply)) | Self::Query(reply) => {
                let _ = reply.send(Err(error.to_owned()));
            }
            Self::Stop(_) => {}
        }
    }
}

/// Owns Pi's command order, prompt replies and dialogs under one lock.
/// The weak input leaves `Pi` alone responsible for keeping stdin open.
pub(super) struct Turns {
    input: mpsc::WeakUnboundedSender<Value>,
    updates: Updates,
    counter: u64,
    pending: HashMap<String, Pending>,
    waiting: VecDeque<(Value, Ordered)>,
    steering: VecDeque<Box<Prompt>>,
    dialogs: HashMap<String, AbortHandle>,
    running: bool,
    /// Started prompts whose user messages precede those of steers.
    starting: usize,
    cancelling: bool,
    exited: Option<String>,
}

impl Turns {
    pub(super) fn new(input: &mpsc::UnboundedSender<Value>, updates: Updates) -> Self {
        Self {
            input: input.downgrade(),
            updates,
            counter: 0,
            pending: HashMap::new(),
            waiting: VecDeque::new(),
            steering: VecDeque::new(),
            dialogs: HashMap::new(),
            running: false,
            starting: 0,
            cancelling: false,
            exited: None,
        }
    }

    pub(super) fn prompt(
        &mut self,
        command: Value,
        prompt: Box<Prompt>,
    ) -> Result<(), Box<Responder<PromptResponse>>> {
        if self.has_exited() {
            return Err(Box::new(prompt.responder));
        }
        self.waiting.push_back((command, Ordered::Prompt(prompt)));
        self.release();
        Ok(())
    }

    pub(super) fn change(&mut self, command: Value) -> oneshot::Receiver<Result<Value, String>> {
        let (reply, receive) = oneshot::channel();
        self.submit(command, Pending::Ordered(Ordered::Change(reply)));
        receive
    }

    pub(super) fn query(&mut self, command: Value) -> oneshot::Receiver<Result<Value, String>> {
        let (reply, receive) = oneshot::channel();
        self.submit(command, Pending::Query(reply));
        receive
    }

    fn submit(&mut self, command: Value, pending: Pending) {
        if let Some(error) = &self.exited {
            pending.fail(error);
        } else if let Pending::Ordered(request) = pending {
            self.waiting.push_back((command, request));
            self.release();
        } else {
            self.send(command, pending);
        }
    }

    pub(super) fn cancel(&mut self) {
        if self.has_exited() {
            return;
        }
        self.cancelling = true;
        self.withdraw_dialogs();
        self.stop();
        for (command, request) in std::mem::take(&mut self.waiting) {
            if let Ordered::Prompt(prompt) = request {
                prompt.fail(Error::request_cancelled());
            } else {
                self.waiting.push_back((command, request));
            }
        }
        self.release();
    }

    pub(super) fn response(&mut self, response: &Value) {
        let id = response["id"].as_str().unwrap_or_default();
        match self.pending.remove(id) {
            Some(Pending::Ordered(Ordered::Prompt(prompt))) => {
                self.prompt_answered(prompt, response);
            }
            Some(Pending::Ordered(Ordered::Change(reply))) => {
                match request_result(response) {
                    Ok(_) => {
                        // Pi reads state synchronously. Capture this change's
                        // result before dispatching later changes and prompts.
                        self.send(json!({"type": "get_state"}), Pending::Query(reply));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            Some(Pending::Query(reply)) => {
                let _ = reply.send(request_result(response));
            }
            Some(Pending::Stop(command)) => {
                if response["success"] != true {
                    eprintln!("pi refused to {command}: {}", response["error"]);
                }
            }
            None => eprintln!("pi answered an unknown command: {response}"),
        }
        self.release();
    }

    fn prompt_answered(&mut self, prompt: Box<Prompt>, response: &Value) {
        if response["success"] != true {
            let error = response["error"]
                .as_str()
                .unwrap_or("pi refused the prompt");
            prompt.fail(Error::new(-32603, error));
            return;
        }
        match response["data"]["disposition"].as_str() {
            Some("queued") => self.steering.push_back(prompt),
            Some("handled") => {
                prompt.accept(&self.updates);
                if !self.running {
                    self.updates.idle(StopReason::EndTurn);
                }
                return;
            }
            _ => {
                // Pi can answer `started` before it emits `agent_start`.
                self.running = true;
                self.starting += 1;
                prompt.accept(&self.updates);
            }
        }
        if self.cancelling {
            // An earlier abort may have preceded this run or queued steer.
            self.stop();
        }
    }

    pub(super) fn started(&mut self) {
        self.running = true;
        self.updates.working();
    }

    pub(super) fn user_message_started(&mut self) {
        if self.starting > 0 {
            self.starting -= 1;
        } else if let Some(prompt) = self.steering.pop_front() {
            prompt.accept(&self.updates);
        }
    }

    pub(super) fn settled(&mut self, aborted: bool, reason: StopReason) {
        if aborted {
            self.withdraw_dialogs();
        }
        self.running = false;
        self.starting = 0;
        self.updates.idle(reason);
        self.release();
    }

    pub(super) fn exited(&mut self, error: String) {
        self.exited = Some(error.clone());
        self.withdraw_dialogs();
        for (_, pending) in self.pending.drain() {
            pending.fail(&error);
        }
        for (_, request) in self.waiting.drain(..) {
            Pending::Ordered(request).fail(&error);
        }
        for prompt in self.steering.drain(..) {
            prompt.fail(Error::new(-32603, &error));
        }
        self.updates.idle(error_stop(error));
    }

    pub(super) fn has_exited(&self) -> bool {
        self.exited.is_some()
    }

    /// Sends the form and registers its task under the cancellation lock.
    #[expect(
        clippy::disallowed_methods,
        reason = "Pi's dialog timeout is the behavior"
    )]
    pub(super) fn dialog(turns: &Arc<Mutex<Self>>, request: Value) {
        let mut state = turns.lock().unwrap();
        let form = if state.cancelling {
            None
        } else {
            dialogs::form(&request)
        };
        let Some((message, schema)) = form else {
            state.write(dialogs::response(&request, None));
            return;
        };
        let Some(id) = request["id"].as_str().map(str::to_owned) else {
            return;
        };
        let ask = state.updates.ask(message, schema);
        let turns = turns.clone();
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            // Pi forgets timed-out dialogs without a withdrawal record.
            let action = if let Some(timeout) = dialogs::timeout(&request) {
                tokio::select! { action = ask => action, () = sleep(timeout) => None }
            } else {
                ask.await
            };
            let mut state = turns.lock().unwrap();
            if state.dialogs.remove(&task_id).is_some() {
                state.write(dialogs::response(&request, action));
            }
        });
        state.dialogs.insert(id, task.abort_handle());
    }

    fn withdraw_dialogs(&mut self) {
        for (id, task) in std::mem::take(&mut self.dialogs) {
            task.abort();
            self.write(json!({"type":"extension_ui_response", "id":id, "cancelled":true}));
        }
    }

    fn write(&self, command: Value) {
        if let Some(input) = self.input.upgrade() {
            let _ = input.send(command);
        }
    }

    fn send(&mut self, mut command: Value, pending: Pending) {
        self.counter += 1;
        let id = self.counter.to_string();
        command["id"] = json!(id);
        self.pending.insert(id, pending);
        self.write(command);
    }

    /// Pi's abort can deliver queued steers, so clear that queue first.
    fn stop(&mut self) {
        for command in ["clear_queue", "abort"] {
            self.send(json!({"type": command}), Pending::Stop(command));
        }
        for prompt in self.steering.drain(..) {
            prompt.fail(Error::request_cancelled());
        }
    }

    fn release(&mut self) {
        if self.cancelling
            && !self.running
            && !self
                .pending
                .values()
                .any(|pending| matches!(pending, Pending::Ordered(Ordered::Prompt(_))))
        {
            self.cancelling = false;
        }
        if self
            .pending
            .values()
            .any(|pending| matches!(pending, Pending::Ordered(Ordered::Change(_))))
        {
            return;
        }
        while let Some((_, request)) = self.waiting.front() {
            if self.cancelling && matches!(request, Ordered::Prompt(_)) {
                break;
            }
            let (command, request) = self.waiting.pop_front().unwrap();
            let changing = matches!(request, Ordered::Change(_));
            self.send(command, Pending::Ordered(request));
            if changing {
                break;
            }
        }
    }
}

fn request_result(response: &Value) -> Result<Value, String> {
    if response["success"] == true {
        Ok(response["data"].clone())
    } else {
        Err(response["error"]
            .as_str()
            .unwrap_or("pi refused the command")
            .to_owned())
    }
}

pub(super) fn error_stop(message: String) -> StopReason {
    StopReason::Error(ErrorStopReason::new().error(acp::Error::new(-32603, message)))
}
