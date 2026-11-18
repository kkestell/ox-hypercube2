use std::collections::{HashMap, VecDeque};

use agent_client_protocol::Error;
use serde_json::{Value, json};
use tokio::sync::oneshot;

type Reply = oneshot::Sender<Result<Value, String>>;

/// What the bridge does after handling a client action or a Pi record.
#[derive(Debug)]
pub(super) enum Action<P> {
    Send(Value),
    Accept(P),
    Fail(P, Error),
    WithdrawDialogs,
    EndTurn,
}

enum Ordered<P> {
    Prompt(P),
    Change(Reply),
}

enum Pending<P> {
    Ordered(Ordered<P>),
    Query(Reply),
    Stop(&'static str),
}

/// Orders commands to Pi and matches untagged run events to client prompts.
pub(super) struct Turns<P> {
    counter: u64,
    pending: HashMap<String, Pending<P>>,
    /// Prompts and config changes wait in the order the client sent them.
    waiting: VecDeque<(Value, Ordered<P>)>,
    /// Prompts Pi queued to steer its run, in the order Pi takes them in.
    steering: VecDeque<P>,
    running: bool,
    /// Prompts Pi started whose user messages still precede those of steers.
    starting: usize,
    cancelling: bool,
    changing: bool,
    exited: Option<String>,
}

impl<P> Default for Turns<P> {
    fn default() -> Self {
        Self {
            counter: 0,
            pending: HashMap::new(),
            waiting: VecDeque::new(),
            steering: VecDeque::new(),
            running: false,
            starting: 0,
            cancelling: false,
            changing: false,
            exited: None,
        }
    }
}

impl<P> Turns<P> {
    pub(super) fn prompt(&mut self, command: Value, prompt: P) -> Result<Vec<Action<P>>, P> {
        if self.has_exited() {
            return Err(prompt);
        }
        self.waiting.push_back((command, Ordered::Prompt(prompt)));
        let mut actions = Vec::new();
        self.release(&mut actions);
        Ok(actions)
    }

    pub(super) fn change(&mut self, command: Value, reply: Reply) -> Vec<Action<P>> {
        if let Some(error) = &self.exited {
            let _ = reply.send(Err(error.clone()));
            return Vec::new();
        }
        self.waiting.push_back((command, Ordered::Change(reply)));
        let mut actions = Vec::new();
        self.release(&mut actions);
        actions
    }

    pub(super) fn query(&mut self, command: Value, reply: Reply) -> Vec<Action<P>> {
        if let Some(error) = &self.exited {
            let _ = reply.send(Err(error.clone()));
            return Vec::new();
        }
        vec![self.send(command, Pending::Query(reply))]
    }

    pub(super) fn cancel(&mut self) -> Vec<Action<P>> {
        if self.has_exited() {
            return Vec::new();
        }
        self.cancelling = true;
        let mut actions = vec![Action::WithdrawDialogs];
        self.stop(&mut actions);
        for (command, pending) in std::mem::take(&mut self.waiting) {
            if let Ordered::Prompt(prompt) = pending {
                actions.push(Action::Fail(prompt, Error::request_cancelled()));
            } else {
                self.waiting.push_back((command, pending));
            }
        }
        self.release(&mut actions);
        actions
    }

    pub(super) fn response(&mut self, response: &Value) -> Vec<Action<P>> {
        let id = response["id"].as_str().unwrap_or_default();
        let mut actions = Vec::new();
        match self.pending.remove(id) {
            Some(Pending::Ordered(Ordered::Prompt(prompt))) => {
                self.prompt_answered(prompt, response, &mut actions);
            }
            Some(Pending::Ordered(Ordered::Change(reply))) => {
                self.changing = false;
                match request_result(response) {
                    Ok(_) => {
                        // Pi reads state synchronously. Capture what this change
                        // applied before sending the next change or prompt.
                        actions
                            .push(self.send(json!({"type": "get_state"}), Pending::Query(reply)));
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
        self.release(&mut actions);
        actions
    }

    /// A handled prompt ends its turn unless the extension started a run.
    /// A queued prompt waits for its user message to start.
    fn prompt_answered(&mut self, prompt: P, response: &Value, actions: &mut Vec<Action<P>>) {
        if response["success"] != true {
            let error = response["error"]
                .as_str()
                .unwrap_or("pi refused the prompt");
            actions.push(Action::Fail(prompt, Error::new(-32603, error)));
            return;
        }
        match response["data"]["disposition"].as_str() {
            Some("queued") => self.steering.push_back(prompt),
            Some("handled") => {
                actions.push(Action::Accept(prompt));
                if !self.running {
                    actions.push(Action::EndTurn);
                }
                return;
            }
            _ => {
                // Pi can answer `started` before it emits `agent_start`.
                self.run_started();
                self.starting += 1;
                actions.push(Action::Accept(prompt));
            }
        }
        if self.cancelling {
            // A stop before Pi starts a run or queues a steer may miss it.
            self.stop(actions);
        }
    }

    pub(super) fn run_started(&mut self) {
        self.running = true;
    }

    pub(super) fn user_message_started(&mut self) -> Vec<Action<P>> {
        if self.starting > 0 {
            self.starting -= 1;
            return Vec::new();
        }
        self.steering
            .pop_front()
            .map(Action::Accept)
            .into_iter()
            .collect()
    }

    pub(super) fn settled(&mut self, aborted: bool) -> Vec<Action<P>> {
        let mut actions = Vec::new();
        if aborted {
            actions.push(Action::WithdrawDialogs);
        }
        self.running = false;
        self.starting = 0;
        self.release(&mut actions);
        actions
    }

    pub(super) fn takes_dialogs(&self) -> bool {
        !self.cancelling
    }

    pub(super) fn exited(&mut self, error: &str) -> Vec<Action<P>> {
        self.exited = Some(error.to_owned());
        let mut actions = vec![Action::WithdrawDialogs];
        for pending in self
            .pending
            .drain()
            .map(|(_, pending)| pending)
            .chain(
                self.waiting
                    .drain(..)
                    .map(|(_, request)| Pending::Ordered(request)),
            )
            .chain(
                self.steering
                    .drain(..)
                    .map(|prompt| Pending::Ordered(Ordered::Prompt(prompt))),
            )
        {
            match pending {
                Pending::Ordered(Ordered::Prompt(prompt)) => {
                    actions.push(Action::Fail(prompt, Error::new(-32603, error)));
                }
                Pending::Ordered(Ordered::Change(reply)) | Pending::Query(reply) => {
                    let _ = reply.send(Err(error.to_owned()));
                }
                Pending::Stop(_) => {}
            }
        }
        actions
    }

    pub(super) fn has_exited(&self) -> bool {
        self.exited.is_some()
    }

    fn send(&mut self, mut command: Value, pending: Pending<P>) -> Action<P> {
        self.counter += 1;
        let id = self.counter.to_string();
        command["id"] = json!(id);
        self.pending.insert(id, pending);
        Action::Send(command)
    }

    /// Pi's abort can deliver queued steers, so clear the queue first.
    fn stop(&mut self, actions: &mut Vec<Action<P>>) {
        for command in ["clear_queue", "abort"] {
            actions.push(self.send(json!({"type": command}), Pending::Stop(command)));
        }
        for prompt in self.steering.drain(..) {
            actions.push(Action::Fail(prompt, Error::request_cancelled()));
        }
    }

    fn release(&mut self, actions: &mut Vec<Action<P>>) {
        if self.cancelling
            && !self.running
            && !self
                .pending
                .values()
                .any(|pending| matches!(pending, Pending::Ordered(Ordered::Prompt(_))))
        {
            self.cancelling = false;
        }
        while let Some((_, request)) = self.waiting.front() {
            if self.changing || (self.cancelling && matches!(request, Ordered::Prompt(_))) {
                break;
            }
            let (command, request) = self.waiting.pop_front().unwrap();
            self.changing = matches!(request, Ordered::Change(_));
            actions.push(self.send(command, Pending::Ordered(request)));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn command(kind: &str) -> Value {
        json!({"type": kind})
    }

    fn sent(actions: &[Action<&str>], kind: &str) -> Value {
        let [Action::Send(command)] = actions else {
            panic!("expected one {kind} command, got {actions:?}");
        };
        assert_eq!(command["type"], kind);
        command.clone()
    }

    fn answer(command: &Value, data: &Value) -> Value {
        json!({"id": command["id"], "success": true, "data": data})
    }

    #[test]
    fn changes_hold_later_prompts_and_changes_until_pi_answers() {
        let mut turns = Turns::default();
        let (reply, mut model_reply) = oneshot::channel();
        let model = sent(&turns.change(command("set_model"), reply), "set_model");

        let (reply, mut effort_reply) = oneshot::channel();
        assert!(
            turns
                .change(command("set_thinking_level"), reply)
                .is_empty()
        );
        assert!(turns.prompt(command("prompt"), "first").unwrap().is_empty());
        let (reply, _later_reply) = oneshot::channel();
        assert!(turns.change(command("set_model"), reply).is_empty());

        let actions = turns.response(&answer(&model, &Value::Null));
        let [Action::Send(model_state), Action::Send(effort)] = actions.as_slice() else {
            panic!("expected a state query and an effort change, got {actions:?}");
        };
        assert_eq!(model_state["type"], "get_state");
        assert_eq!(effort["type"], "set_thinking_level");
        let model_snapshot = json!({"model": "second"});
        assert!(
            turns
                .response(&answer(model_state, &model_snapshot))
                .is_empty()
        );
        assert_eq!(model_reply.try_recv().unwrap(), Ok(model_snapshot));

        let actions = turns.response(&answer(effort, &Value::Null));
        let [
            Action::Send(effort_state),
            Action::Send(prompt),
            Action::Send(later_model),
        ] = actions.as_slice()
        else {
            panic!("expected a state query, a prompt and a model change, got {actions:?}");
        };
        assert_eq!(effort_state["type"], "get_state");
        assert_eq!(prompt["type"], "prompt");
        assert_eq!(later_model["type"], "set_model");
        let effort_snapshot = json!({"thinkingLevel": "low"});
        assert!(
            turns
                .response(&answer(effort_state, &effort_snapshot))
                .is_empty()
        );
        assert_eq!(effort_reply.try_recv().unwrap(), Ok(effort_snapshot));
    }

    #[test]
    fn dialogs_are_refused_from_a_cancel_until_the_cancelled_work_ends() {
        #[derive(Debug, Clone, Copy)]
        enum Event {
            Prompt,
            Answer(&'static str),
            RunStarted,
            Cancel,
            Settled,
        }
        use Event::*;

        let during_run = [(RunStarted, true), (Cancel, false), (Settled, true)];
        let before_started = [
            (Prompt, true),
            (Cancel, false),
            (Answer("started"), false),
            (RunStarted, false),
            (Settled, true),
        ];
        let before_handled = [(Prompt, true), (Cancel, false), (Answer("handled"), true)];
        let idle = [(Cancel, true)];
        for events in [&during_run[..], &before_started, &before_handled, &idle] {
            let mut turns = Turns::default();
            let mut prompt = Value::Null;
            for &(event, takes_dialogs) in events {
                match event {
                    Prompt => {
                        prompt = sent(&turns.prompt(command("prompt"), "first").unwrap(), "prompt");
                    }
                    Answer(disposition) => {
                        let actions =
                            turns.response(&answer(&prompt, &json!({"disposition": disposition})));
                        if disposition == "started" {
                            let [
                                Action::Accept("first"),
                                Action::Send(clear),
                                Action::Send(abort),
                            ] = actions.as_slice()
                            else {
                                panic!("expected to stop the run Pi just started, got {actions:?}");
                            };
                            assert_eq!(clear["type"], "clear_queue");
                            assert_eq!(abort["type"], "abort");
                        } else {
                            assert!(
                                matches!(
                                    actions.as_slice(),
                                    [Action::Accept("first"), Action::EndTurn]
                                ),
                                "{actions:?}"
                            );
                        }
                    }
                    RunStarted => turns.run_started(),
                    Cancel => {
                        assert!(matches!(
                            turns.cancel().first(),
                            Some(Action::WithdrawDialogs)
                        ));
                    }
                    Settled => {
                        turns.settled(true);
                    }
                }
                assert_eq!(
                    turns.takes_dialogs(),
                    takes_dialogs,
                    "{events:?} at {event:?}"
                );
            }
        }
    }

    #[test]
    fn a_prompt_sent_during_a_cancel_waits_for_the_cancelled_run_and_is_not_stopped() {
        let mut turns = Turns::default();
        let first = sent(&turns.prompt(command("prompt"), "first").unwrap(), "prompt");
        let started = json!({"disposition": "started"});
        turns.response(&answer(&first, &started));
        turns.cancel();
        assert!(turns.prompt(command("prompt"), "next").unwrap().is_empty());

        let actions = turns.settled(true);
        assert!(matches!(actions.first(), Some(Action::WithdrawDialogs)));
        let next = sent(&actions[1..], "prompt");
        let actions = turns.response(&answer(&next, &started));
        assert!(
            matches!(actions.as_slice(), [Action::Accept("next")]),
            "{actions:?}"
        );
    }
}
