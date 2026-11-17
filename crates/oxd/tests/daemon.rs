#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ox_protocol::schema::{
    AudioContent, ContentBlock, ElicitationAcceptAction, ElicitationAction, EmbeddedResource,
    EmbeddedResourceResource, ImageContent, PromptCapabilities, RequestPermissionOutcome,
    ResourceLink, SessionConfigKind, SessionConfigOption, SessionUpdate, StateUpdate, StopReason,
    TextContent, TextResourceContents,
};
use ox_protocol::{
    Checkout, Client, ConfigSetting, ConfigValue, DirEntry, End, EntryKind, Error, Event,
    EventKind, Listing, ListingChange, PROTOCOL_VERSION, SessionInfo, SessionState, Stream,
    Subscription, Unseen, WorkspaceInfo, code,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::timeout;

mod common;

use common::*;

impl Daemon {
    /// The workspace for the daemon's directory, which sessions run in.
    async fn workspace(&self, client: &Client) -> WorkspaceInfo {
        match workspaces(client)
            .await
            .into_iter()
            .find(|w| w.path == self.cwd())
        {
            Some(workspace) => workspace,
            None => client.add_workspace(self.cwd()).await.unwrap(),
        }
    }

    async fn new_session(&self, client: &Client) -> (SessionInfo, u64) {
        self.new_session_with(client, &[]).await
    }

    /// Creates a session whose fake agent gets `args`, waits for the agent
    /// to start, and returns the session and the seq of its `agent_started`
    /// event.
    async fn new_session_with(&self, client: &Client, args: &[&str]) -> (SessionInfo, u64) {
        let workspace = self.workspace(client).await;
        let session = client
            .new_session(
                &workspace.id,
                fake_agent(args),
                BTreeMap::new(),
                Checkout::Workspace,
                Vec::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(session.state, SessionState::Starting);
        let mut events = client.subscribe(&session.id, 0).await.unwrap();
        let started = next(&mut events).await;
        assert!(
            matches!(started.kind, EventKind::AgentStarted { .. }),
            "{started:?}"
        );
        let session = info(client, &session.id).await.unwrap();
        assert_eq!(session.state, SessionState::Idle);
        (session, started.seq)
    }
}

/// The command that runs the fake agent with `args`.
fn fake_agent(args: &[&str]) -> Vec<String> {
    std::iter::once(FAKE_AGENT)
        .chain(args.iter().copied())
        .map(String::from)
        .collect()
}

/// A connection that speaks the wire protocol itself and reads only when
/// asked to.
struct Raw {
    lines: Lines<BufReader<OwnedReadHalf>>,
    write: OwnedWriteHalf,
    next_id: u64,
}

impl Raw {
    async fn connect(daemon: &Daemon) -> Self {
        let stream = UnixStream::connect(&daemon.socket).await.unwrap();
        let (read, write) = stream.into_split();
        let mut raw = Self {
            lines: BufReader::new(read).lines(),
            write,
            next_id: 1,
        };
        raw.send("hello", json!({"protocol_version": PROTOCOL_VERSION}))
            .await;
        raw
    }

    /// Sends a request and returns its id.
    async fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({"id": id, "method": method, "params": params});
        self.write
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        id
    }

    async fn read(&mut self) -> Value {
        let line = timeout(TIMEOUT, self.lines.next_line())
            .await
            .expect("timed out waiting for a message")
            .unwrap()
            .expect("the daemon closed the connection");
        serde_json::from_str(&line).unwrap()
    }

    /// Reads until the answer to request `id`.
    async fn answer(&mut self, id: u64) -> Value {
        loop {
            let message = self.read().await;
            if message["id"] == id {
                return message;
            }
        }
    }

    /// Reads until `last` matches a message, and returns every message read.
    async fn messages_until(&mut self, last: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let message = self.read().await;
            let done = last(&message);
            messages.push(message);
            if done {
                return messages;
            }
        }
    }

    /// Reads until `last` matches an event message, and returns every event
    /// message read.
    async fn events_until(&mut self, last: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut events = Vec::new();
        loop {
            let message = self.read().await;
            if message["event"].is_object() {
                let done = last(&message);
                events.push(message);
                if done {
                    return events;
                }
            }
        }
    }
}

fn raw_turn_ended(message: &Value) -> bool {
    message["event"]["kind"]["type"] == "turn_ended"
}

/// Why the subscription has no more events.
async fn end(subscription: &mut Subscription) -> End {
    timeout(TIMEOUT, subscription.next())
        .await
        .expect("subscription did not end")
        .expect_err("subscription did not end")
}

/// A watch and the listing its changes add up to.
struct Listings {
    watch: Stream<Vec<ListingChange>>,
    current: Listing,
}

async fn listings(client: &Client) -> Listings {
    Listings {
        watch: client.watch().await.unwrap(),
        current: Listing::default(),
    }
}

impl Listings {
    /// The listing after each of the watch's messages, up to and including
    /// the first one matching `is_last`.
    async fn until(&mut self, is_last: impl Fn(&Listing) -> bool) -> Vec<Listing> {
        let mut listings = Vec::new();
        loop {
            let changes = next_changes(&mut self.watch).await;
            for change in changes {
                apply(&mut self.current, change);
            }
            let done = is_last(&self.current);
            listings.push(self.current.clone());
            if done {
                return listings;
            }
        }
    }
}

/// The state of session `id` in a sessions list, if it is listed.
fn state_in(list: &[SessionInfo], id: &str) -> Option<SessionState> {
    list.iter()
        .find(|session| session.id == id)
        .map(|session| session.state)
}

fn chunk(event: &Event) -> Option<&str> {
    match &event.kind {
        EventKind::Update(SessionUpdate::AgentMessageChunk(chunk)) => match &chunk.content {
            ContentBlock::Text(text) => Some(&text.text),
            _ => None,
        },
        _ => None,
    }
}

fn chunks(events: &[Event]) -> Vec<&str> {
    events.iter().filter_map(chunk).collect()
}

/// The name of each event: the `state_update` states and `user_message`
/// updates by their own names, other updates as `update`.
fn update_names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match &event.kind {
            EventKind::PromptStarted { .. } => "prompt_started",
            EventKind::PromptSteered { .. } => "prompt_steered",
            EventKind::SteeredPromptTaken { .. } => "steered_prompt_taken",
            EventKind::SteeredPromptDropped { .. } => "steered_prompt_dropped",
            EventKind::TurnStarted => "turn_started",
            EventKind::TurnEnded { .. } => "turn_ended",
            EventKind::Update(SessionUpdate::UserMessage(_)) => "user_message",
            EventKind::Update(SessionUpdate::AgentMessageChunk(_)) => "agent_message_chunk",
            EventKind::Update(SessionUpdate::StateUpdate(StateUpdate::Running(_))) => "running",
            EventKind::Update(SessionUpdate::StateUpdate(StateUpdate::Idle(_))) => "idle",
            EventKind::Update(SessionUpdate::StateUpdate(_)) => "state",
            EventKind::Update(_) => "update",
            _ => "other",
        })
        .collect()
}

/// Asserts that the events' seqs increase by one from `first`.
fn assert_contiguous(events: &[Event], first: u64) {
    let seqs: Vec<u64> = events.iter().map(|event| event.seq).collect();
    let expected: Vec<u64> = (first..first + events.len() as u64).collect();
    assert_eq!(seqs, expected);
}

/// The current value of a select config option.
fn current(options: &[SessionConfigOption], id: &str) -> String {
    let option = options
        .iter()
        .find(|option| &*option.config_id.0 == id)
        .unwrap_or_else(|| panic!("no config option {id}"));
    match &option.kind {
        SessionConfigKind::Select(select) => select.current_value.to_string(),
        other => panic!("config option {id} is {other:?}"),
    }
}

fn remote_code(error: Error) -> i32 {
    match error {
        Error::Remote(body) => body.code,
        other => panic!("expected a daemon error, got {other}"),
    }
}

fn running(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Update(SessionUpdate::StateUpdate(StateUpdate::Running(_)))
    )
}

/// The prompt of a `prompt_started` event.
fn started_prompt(event: &Event) -> &[ContentBlock] {
    match &event.kind {
        EventKind::PromptStarted { prompt, .. } => prompt,
        other => panic!("expected prompt_started, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_sent_during_a_turn_is_taken_into_it() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("take")).await.unwrap();
    until(&mut events, running).await;
    client.prompt(&session.id, text("also")).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(
        update_names(&turn),
        [
            "prompt_steered",
            "user_message",
            "steered_prompt_taken",
            "idle",
            "turn_ended"
        ]
    );
    assert_eq!(
        turn[0].kind,
        EventKind::PromptSteered {
            prompt: text("also"),
            from: None,
        }
    );
    assert_eq!(
        turn[2].kind,
        EventKind::SteeredPromptTaken {
            prompt: turn[0].seq
        }
    );
    assert_eq!(
        turn[4].kind,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::EndTurn),
            error: None
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_prompts_are_sent_when_turns_end_with_no_client_connected() {
    let daemon = Daemon::start().await;
    let gate = tempfile::tempdir().unwrap();
    mkfifos(gate.path());
    let gated = format!(
        "shell {}",
        json!(format!(
            "echo blocked > '{0}/blocked'; read _ < '{0}/release'",
            gate.path().display()
        ))
    );
    let client = daemon.client().await;
    let (session, _) = daemon.new_session(&client).await;
    client.prompt(&session.id, text(&gated)).await.unwrap();
    let first = client
        .queue_add(&session.id, text("text \"first\""))
        .await
        .unwrap();
    client
        .queue_add(&session.id, text("text \"second\""))
        .await
        .unwrap();
    let queued = info(&client, &session.id).await.unwrap().queue;
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0].id, first);
    blocked(gate.path()).await;
    drop(client);

    release(gate.path()).await;
    let client = daemon.client().await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let mut prompts = Vec::new();
    while prompts.len() < 3 {
        let event = next(&mut events).await;
        if matches!(event.kind, EventKind::PromptStarted { .. }) {
            prompts.push(started_prompt(&event).to_vec());
        }
    }
    assert_eq!(
        prompts[1..],
        [text("text \"first\""), text("text \"second\"")]
    );
    let rest = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&rest), ["second"]);
    assert!(info(&client, &session.id).await.unwrap().queue.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_holds_the_queue_and_drops_a_prompt_the_agent_has_not_taken() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("wait")).await.unwrap();
    until(&mut events, running).await;
    client.prompt(&session.id, text("pid")).await.unwrap();
    client
        .queue_add(&session.id, text("text \"queued\""))
        .await
        .unwrap();
    client.cancel(&session.id).await.unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert_eq!(
        update_names(&rest),
        [
            "prompt_steered",
            "steered_prompt_dropped",
            "idle",
            "turn_ended"
        ]
    );
    assert_eq!(
        rest[1].kind,
        EventKind::SteeredPromptDropped {
            prompt: rest[0].seq
        }
    );

    // The queue waits for a prompt, and its first prompt follows that turn.
    assert_eq!(info(&client, &session.id).await.unwrap().queue.len(), 1);
    client
        .prompt(&session.id, text("text \"sent\""))
        .await
        .unwrap();
    let sent = until(&mut events, turn_ended).await;
    assert_eq!(started_prompt(&sent[0]), text("text \"sent\""));
    let queued = until(&mut events, turn_ended).await;
    assert_eq!(started_prompt(&queued[0]), text("text \"queued\""));
    assert_eq!(chunks(&queued), ["queued"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_queue_survives_a_restart() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let (session, started) = first.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("wait")).await.unwrap();
    until(&mut events, running).await;
    let id = client
        .queue_add(&session.id, text("text \"queued\""))
        .await
        .unwrap();
    client.cancel(&session.id).await.unwrap();
    until(&mut events, turn_ended).await;
    let queue = info(&client, &session.id).await.unwrap().queue;
    drop(events);
    drop(client);
    first.stop().await;

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    assert_eq!(info(&client, &session.id).await.unwrap().queue, queue);
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    until(&mut events, agent_started).await;
    let loaded = until(&mut events, agent_started).await;
    let after = loaded.last().unwrap().seq;
    // Still held after the agent started: sending it now starts its turn.
    assert_eq!(info(&client, &session.id).await.unwrap().queue, queue);
    client.queue_send(&session.id, &id).await.unwrap();
    let turn = until_turn_ended(&client, &session.id, after).await;
    assert_eq!(started_prompt(&turn[0]), text("text \"queued\""));
    assert!(info(&client, &session.id).await.unwrap().queue.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_prompts_are_edited_and_removed_until_they_are_sent() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("wait")).await.unwrap();
    until(&mut events, running).await;
    let kept = client.queue_add(&session.id, text("a")).await.unwrap();
    let removed = client.queue_add(&session.id, text("b")).await.unwrap();
    client
        .queue_edit(&session.id, &kept, text("text \"edited\""))
        .await
        .unwrap();
    client.queue_remove(&session.id, &removed).await.unwrap();
    let queue = info(&client, &session.id).await.unwrap().queue;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].id, kept);
    assert_eq!(queue[0].prompt, text("text \"edited\""));
    let gone = client
        .queue_remove(&session.id, &removed)
        .await
        .unwrap_err();
    assert_eq!(remote_code(gone), code::NOT_FOUND);

    // Sending it now steers the running turn.
    client.queue_send(&session.id, &kept).await.unwrap();
    let steered = until(&mut events, |kind| {
        matches!(kind, EventKind::PromptSteered { .. })
    })
    .await;
    assert_eq!(
        steered.last().unwrap().kind,
        EventKind::PromptSteered {
            prompt: text("text \"edited\""),
            from: None,
        }
    );
    let edit = client
        .queue_edit(&session.id, &kept, text("late"))
        .await
        .unwrap_err();
    assert_eq!(remote_code(edit), code::NOT_FOUND);
    client.cancel(&session.id).await.unwrap();
    until(&mut events, turn_ended).await;
}

/// A client that reconnects during a turn receives the rest of it after the
/// last event it saw.
#[tokio::test(flavor = "multi_thread")]
async fn reattach_mid_turn() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    // The permission request holds the turn open until it is answered.
    client
        .prompt(&session.id, text("pid\nsay 20 0\npermission"))
        .await
        .unwrap();
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(next(&mut events).await);
    }
    drop(events);
    drop(client);

    let client = daemon.client().await;
    let mut events = client
        .subscribe(&session.id, seen.last().unwrap().seq)
        .await
        .unwrap();
    seen.extend(
        until(&mut events, |kind| {
            matches!(kind, EventKind::PermissionRequested { .. })
        })
        .await,
    );
    let EventKind::PermissionRequested { request_id, .. } = &seen.last().unwrap().kind else {
        unreachable!()
    };
    client
        .permit(&session.id, &request_id.clone(), "allow")
        .await
        .unwrap();
    seen.extend(until(&mut events, turn_ended).await);
    assert_contiguous(&seen, started + 1);
    assert!(matches!(
        seen.last().unwrap().kind,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::EndTurn),
            ..
        }
    ));
    let pid = chunk(&seen[3]).unwrap().to_owned();
    let expected: Vec<String> = (1..=20)
        .map(|i| format!("chunk-{i}"))
        .chain(["allowed".to_owned()])
        .collect();
    assert_eq!(chunks(&seen[4..]), expected);

    client.prompt(&session.id, text("pid")).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), [pid.as_str()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn permission_across_disconnect() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, _) = daemon.new_session(&client).await;
    client
        .prompt(&session.id, text("permission"))
        .await
        .unwrap();
    drop(client);

    let client = daemon.client().await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let requested = until(&mut events, |kind| {
        matches!(kind, EventKind::PermissionRequested { .. })
    })
    .await;
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::AwaitingPermission
    );
    let EventKind::PermissionRequested { request_id, .. } = &requested.last().unwrap().kind else {
        unreachable!()
    };
    client
        .permit(&session.id, request_id, "allow")
        .await
        .unwrap();

    let rest = until(&mut events, turn_ended).await;
    assert!(matches!(
        &rest[0].kind,
        EventKind::PermissionResolved {
            outcome: RequestPermissionOutcome::Selected(selected),
            ..
        } if &*selected.option_id.0 == "allow"
    ));
    assert_eq!(chunks(&rest), ["allowed"]);

    let error = client
        .permit(&session.id, request_id, "allow")
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_with_pending_permission() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("permission"))
        .await
        .unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::PermissionRequested { .. })
    })
    .await;
    client.cancel(&session.id).await.unwrap();

    let rest = until(&mut events, turn_ended).await;
    assert!(matches!(
        rest[0].kind,
        EventKind::PermissionResolved {
            outcome: RequestPermissionOutcome::Cancelled(_),
            ..
        }
    ));
    assert_eq!(chunks(&rest), ["cancelled"]);
    assert!(matches!(
        rest.last().unwrap().kind,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::Cancelled),
            ..
        }
    ));
}

/// Prompts the fake agent's `elicit` and returns the request ID of the
/// elicitation it opens, once the session awaits input.
async fn elicit(client: &Client, session_id: &str, events: &mut Subscription) -> String {
    client.prompt(session_id, text("elicit")).await.unwrap();
    let requested = until(events, |kind| {
        matches!(kind, EventKind::ElicitationRequested { .. })
    })
    .await;
    let EventKind::ElicitationRequested {
        request_id,
        message,
        requested_schema,
        ..
    } = &requested.last().unwrap().kind
    else {
        unreachable!()
    };
    assert_eq!(message, "Order");
    assert_eq!(requested_schema.required, Some(vec!["color".into()]));
    assert_eq!(
        info(client, session_id).await.unwrap().state,
        SessionState::AwaitingInput
    );
    request_id.clone()
}

fn accept(content: Value) -> ElicitationAction {
    ElicitationAction::Accept(
        ElicitationAcceptAction::new()
            .content(serde_json::from_value::<BTreeMap<_, _>>(content).unwrap()),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn elicitation_round_trip() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let request_id = elicit(&client, &session.id, &mut events).await;

    let answer = json!({"color": "blue", "extras": ["nuts"]});
    client
        .respond_elicitation(&session.id, &request_id, accept(answer.clone()))
        .await
        .unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert_eq!(
        rest[0].kind,
        EventKind::ElicitationResolved {
            request_id: request_id.clone(),
            action: accept(answer),
        }
    );
    assert_eq!(
        chunks(&rest),
        [r#"accepted {"color":"blue","extras":["nuts"]}"#]
    );

    let error = client
        .respond_elicitation(&session.id, &request_id, ElicitationAction::Decline)
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
}

/// A decline reaches the agent. `session/cancel` leaves the elicitation to
/// the agent, which withdraws it with `$/cancel_request`.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_withdraws_an_elicitation() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let request_id = elicit(&client, &session.id, &mut events).await;
    client
        .respond_elicitation(&session.id, &request_id, ElicitationAction::Decline)
        .await
        .unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert_eq!(
        rest[0].kind,
        EventKind::ElicitationResolved {
            request_id,
            action: ElicitationAction::Decline,
        }
    );
    assert_eq!(chunks(&rest), ["declined"]);

    let request_id = elicit(&client, &session.id, &mut events).await;
    client.cancel(&session.id).await.unwrap();

    let rest = until(&mut events, turn_ended).await;
    assert_eq!(
        rest[0].kind,
        EventKind::ElicitationResolved {
            request_id,
            action: ElicitationAction::Cancel,
        }
    );
    assert_eq!(chunks(&rest), ["withdrawn"]);
    assert!(matches!(
        rest.last().unwrap().kind,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::Cancelled),
            ..
        }
    ));
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
}

/// A client that vanishes mid-stream, leaving events unread in its socket,
/// does not disturb the daemon or the turn.
#[tokio::test(flavor = "multi_thread")]
async fn vanished_client() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();

    let mut raw = Raw::connect(&daemon).await;
    raw.send("session.subscribe", json!({"session_id": session.id}))
        .await;
    // The turn streams its chunks and stays open until it is cancelled.
    client
        .prompt(&session.id, text("say 200 0\nwait"))
        .await
        .unwrap();
    raw.events_until(|message| message["event"]["seq"] == 3)
        .await;
    let mut turn = until(&mut events, |kind| {
        matches!(
            kind,
            EventKind::Update(SessionUpdate::AgentMessageChunk(update))
                if matches!(&update.content, ContentBlock::Text(text) if text.text == "chunk-200")
        )
    })
    .await;
    drop(raw);

    client.cancel(&session.id).await.unwrap();
    turn.extend(until(&mut events, turn_ended).await);
    assert_contiguous(&turn, started + 1);
    assert_eq!(chunks(&turn).len(), 200);
    assert!(matches!(
        turn.last().unwrap().kind,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::Cancelled),
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_exit() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("exit 1")).await.unwrap();
    let history = until(&mut events, |kind| {
        matches!(kind, EventKind::AgentExited { .. })
    })
    .await;
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Exited
    );
    assert_contiguous(&history, started + 1);
    assert!(matches!(history[0].kind, EventKind::PromptStarted { .. }));
    assert!(turn_ended(&history[history.len() - 2].kind));
    let error = client.prompt(&session.id, text("pid")).await.unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);

    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    assert!(sessions(&client).await.is_empty());
    let error = client.subscribe(&session.id, 0).await.unwrap_err();
    assert_eq!(remote_code(error), code::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn lifetime() {
    let mut daemon = Daemon::start_with(Options {
        idle_exit_ms: Some(IDLE_EXIT_MS),
        ..Options::default()
    })
    .await;
    let client = daemon.client().await;
    let (session, _) = daemon.new_session(&client).await;
    // The live session keeps the daemon running without a client, which the
    // daemon's unit tests establish for every state.
    drop(client);
    let client = daemon.client().await;
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    drop(client);
    assert!(daemon.wait_for_exit().await.success());
    assert!(!daemon.socket.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn single_instance_and_on_demand_start() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("oxd.sock");
    let client = timeout(
        TIMEOUT,
        Client::connect_or_start_with(&socket, Path::new(OXD)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(sessions(&client).await.is_empty());

    let second = std::process::Command::new(OXD)
        .arg("--socket")
        .arg(&socket)
        .output()
        .unwrap();
    assert!(second.status.success());
    assert_eq!(second.stdout, b"ready\n");

    assert!(sessions(&client).await.is_empty());
    let other = Client::connect(&socket).await.unwrap();
    assert!(sessions(&other).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn version_mismatch() {
    let daemon = Daemon::start().await;
    let stream = UnixStream::connect(&daemon.socket).await.unwrap();
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut send = async |message: Value| {
        write
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
        let line = timeout(TIMEOUT, lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    };

    let response = send(json!({"id": 1, "method": "session.list"})).await;
    assert_eq!(response["error"]["code"], code::INVALID_REQUEST);
    let response =
        send(json!({"id": 2, "method": "hello", "params": {"protocol_version": 999}})).await;
    assert_eq!(response["id"], 2);
    assert_eq!(response["error"]["code"], code::PROTOCOL_MISMATCH);
}

#[tokio::test(flavor = "multi_thread")]
async fn subscriptions_to_one_session_are_independent() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut all = client.subscribe(&session.id, 0).await.unwrap();
    let mut live = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("say 3 0")).await.unwrap();
    let turn = until(&mut live, turn_ended).await;
    assert_contiguous(&turn, started + 1);
    let history = until(&mut all, turn_ended).await;
    assert_contiguous(&history, 1);
    assert_eq!(history[usize::try_from(started).unwrap()..], turn);

    let last_seq = turn.last().unwrap().seq;
    let error = client
        .subscribe(&session.id, last_seq + 1)
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn unsubscribe_ends_a_subscription() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut raw = Raw::connect(&daemon).await;
    let first = raw
        .send("session.subscribe", json!({"session_id": session.id}))
        .await;
    raw.events_until(|message| message["event"]["seq"] == started)
        .await;
    let unsubscribe = raw
        .send("unsubscribe", json!({"subscription": first}))
        .await;
    raw.answer(unsubscribe).await;
    let second = raw
        .send(
            "session.subscribe",
            json!({"session_id": session.id, "after_seq": started}),
        )
        .await;
    client.prompt(&session.id, text("say 3 0")).await.unwrap();
    let turn = raw.events_until(raw_turn_ended).await;
    // The prompt, its echo, `running`, three chunks, `idle`, and the turn's end.
    assert_eq!(turn.len(), 8);
    for message in turn {
        assert_eq!(message["subscription"], second);
    }
}

/// A subscriber that reads nothing while a session records more events than
/// the daemon and the socket buffer for it receives every event once it
/// reads.
#[tokio::test(flavor = "multi_thread")]
async fn slow_subscriber_receives_every_event() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut raw = Raw::connect(&daemon).await;
    raw.send(
        "session.subscribe",
        json!({"session_id": session.id, "after_seq": started}),
    )
    .await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("say 5000 0"))
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;

    let seqs: Vec<u64> = raw
        .events_until(raw_turn_ended)
        .await
        .iter()
        .map(|message| message["event"]["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, turn.iter().map(|event| event.seq).collect::<Vec<_>>());
}

/// A stopping daemon sends a client that has fallen behind the rest of each
/// session it follows, up to the session's close, before it exits.
#[tokio::test(flavor = "multi_thread")]
async fn a_stopping_daemon_sends_the_rest_of_each_subscription() {
    let mut daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut raw = Raw::connect(&daemon).await;
    raw.send(
        "session.subscribe",
        json!({"session_id": session.id, "after_seq": started}),
    )
    .await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("say 1000 0"))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;

    // The raw client has read nothing, so most of the turn is still to be
    // sent to it.
    daemon.stop_signal();
    let rest = raw
        .events_until(|message| message["event"]["kind"]["type"] == "session_closed")
        .await;
    assert_eq!(
        rest.iter()
            .filter(|message| raw_turn_ended(message))
            .count(),
        1
    );
    assert!(daemon.wait_for_exit().await.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_while_starting() {
    let mut daemon = Daemon::start_with(Options {
        idle_exit_ms: Some(IDLE_EXIT_MS),
        ..Options::default()
    })
    .await;
    let client = daemon.client().await;
    let mut agent = ExitWatch::new(daemon.dir.path());
    let mut args = vec!["--hang-initialize"];
    args.extend(agent.arg());
    let workspace = daemon.workspace(&client).await;
    let session = client
        .new_session(
            &workspace.id,
            fake_agent(&args),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(session.state, SessionState::Starting);
    // The agent runs and never answers `initialize`.
    agent.started().await;
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Starting
    );

    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let error = timeout(TIMEOUT, client.archive(&session.id, false, BTreeMap::new()))
        .await
        .expect("archive waited for the agent to start")
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Remote(ref body) if body.code == code::INVALID_STATE
    ));
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::SessionClosed
    ));
    agent.exited().await;
    drop(client);
    assert!(daemon.wait_for_exit().await.success());
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_ends_other_clients_subscriptions() {
    let daemon = Daemon::start().await;
    let watcher = daemon.client().await;
    let (session, started) = daemon.new_session(&watcher).await;
    let mut events = watcher.subscribe(&session.id, started).await.unwrap();
    let deleter = daemon.client().await;
    deleter
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    let last = next(&mut events).await;
    assert!(matches!(last.kind, EventKind::SessionClosed));
    assert_eq!(end(&mut events).await, End::SessionClosed);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_ends_the_turn_and_its_permission_request() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("permission"))
        .await
        .unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::PermissionRequested { .. })
    })
    .await;
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    let rest = until(&mut events, |kind| matches!(kind, EventKind::SessionClosed)).await;
    assert_eq!(rest.len(), 3, "{rest:?}");
    assert!(matches!(
        rest[0].kind,
        EventKind::PermissionResolved {
            outcome: RequestPermissionOutcome::Cancelled(_),
            ..
        }
    ));
    assert!(matches!(
        rest[1].kind,
        EventKind::TurnEnded {
            stop_reason: None,
            error: Some(_)
        }
    ));
    assert_eq!(end(&mut events).await, End::SessionClosed);
}

#[tokio::test(flavor = "multi_thread")]
async fn permission_after_turn_returns_session_to_idle() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("late-permission"))
        .await
        .unwrap();
    let history = until(&mut events, |kind| {
        matches!(kind, EventKind::PermissionRequested { .. })
    })
    .await;
    assert!(history.iter().any(|event| turn_ended(&event.kind)));
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::AwaitingPermission
    );
    let EventKind::PermissionRequested { request_id, .. } = &history.last().unwrap().kind else {
        unreachable!()
    };
    client
        .permit(&session.id, request_id, "allow")
        .await
        .unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::PermissionResolved { .. })
    })
    .await;
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
    client.prompt(&session.id, text("pid")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_agent_starts_runs_until_the_agent_is_idle() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("own-turn")).await.unwrap();
    let history = until(&mut events, |kind| {
        matches!(kind, EventKind::Update(SessionUpdate::AgentMessageChunk(_)))
    })
    .await;
    assert_eq!(chunks(&history), ["own-turn"]);
    assert_eq!(
        update_names(&history),
        [
            "prompt_started",
            "user_message",
            "running",
            "idle",
            "turn_ended",
            "running",
            "turn_started",
            "agent_message_chunk",
        ]
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Running
    );
    client.cancel(&session.id).await.unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert_eq!(update_names(&rest), ["idle", "turn_ended"], "{rest:?}");
    assert_eq!(
        rest[1].kind,
        EventKind::TurnEnded {
            stop_reason: Some(StopReason::Cancelled),
            error: None
        }
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
    client.prompt(&session.id, text("pid")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_ends_with_the_stop_reason_the_agent_reports() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("say 1 0\nfail the model failed"))
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    let EventKind::TurnEnded { stop_reason, error } = &turn.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(*error, None);
    let Some(StopReason::Error(details)) = stop_reason else {
        panic!("expected the stop reason error, got {stop_reason:?}");
    };
    assert_eq!(
        details.error.as_ref().map(|error| error.message.as_str()),
        Some("the model failed"),
        "{details:?}"
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_the_agent_rejects_ends_its_turn_with_the_error() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("reject not today"))
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(update_names(&turn), ["prompt_started", "turn_ended"]);
    assert!(
        matches!(&turn[1].kind, EventKind::TurnEnded { stop_reason: None, error: Some(error) } if error.contains("Invalid params")),
        "{turn:?}"
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
    client.prompt(&session.id, text("pid")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn exhausted_descriptors_do_not_stop_the_daemon() {
    let mut daemon = Daemon::start_with(Options {
        fd_limit: Some(64),
        log: true,
        ..Options::default()
    })
    .await;
    let mut streams = Vec::new();
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(&daemon.socket).await {
            streams.push(stream);
        }
    }
    daemon.wait_for_log("accept failed").await;
    drop(streams);
    let client = daemon.client().await;
    let (session, _) = daemon.new_session(&client).await;
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    assert!(daemon.child.try_wait().unwrap().is_none(), "oxd exited");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_rejection_does_not_end_the_next_prompt() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("deferred-reject"))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
    client.prompt(&session.id, text("pid\nwait")).await.unwrap();
    // The fake agent answers the first prompt with an error only after the
    // second arrives, then starts the second prompt and reports its PID.
    let second = until(&mut events, |kind| {
        matches!(kind, EventKind::Update(SessionUpdate::AgentMessageChunk(_)))
    })
    .await;
    assert!(
        !second.iter().any(|event| turn_ended(&event.kind)),
        "the second prompt ended before cancellation: {:?}",
        update_names(&second)
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Running
    );
    client.cancel(&session.id).await.unwrap();
    until(&mut events, turn_ended).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_after_the_resume_answer_is_live() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon
        .new_session_with(&client, &["--turn-after-resume"])
        .await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("exit 0")).await.unwrap();
    until(&mut events, agent_exited).await;
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let mut resumed = until(&mut events, agent_started).await;
    // A config update is a round-trip barrier after the resume handler has
    // sent all three live updates. No absence is checked before that barrier.
    client
        .set_config_option(&session.id, "mode", ConfigValue::Id("code".into()))
        .await
        .unwrap();
    resumed.extend(
        until(&mut events, |kind| {
            matches!(
                kind,
                EventKind::Update(SessionUpdate::ConfigOptionUpdate(_))
            )
        })
        .await,
    );
    assert!(
        resumed
            .iter()
            .any(|event| event.kind == EventKind::TurnStarted),
        "the live turn after the response was lost as replay: {:?}",
        update_names(&resumed)
    );
    assert!(resumed.iter().any(|event| turn_ended(&event.kind)));
}

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_stops_agents_and_removes_the_socket() {
    let mut daemon = Daemon::start().await;
    let client = daemon.client().await;
    let agent = ExitWatch::new(daemon.dir.path());
    daemon.new_session_with(&client, &agent.arg()).await;
    drop(client);
    // SAFETY: sends SIGTERM to the daemon this test started.
    unsafe { libc::kill(daemon.child.id().unwrap().cast_signed(), libc::SIGTERM) };
    assert!(daemon.wait_for_exit().await.success());
    assert!(!daemon.socket.exists());
    agent.exited().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_stops_the_agent() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let agent = ExitWatch::new(daemon.dir.path());
    let (session, _) = daemon.new_session_with(&client, &agent.arg()).await;
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    agent.exited().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let daemon = Daemon::start().await;
    let mode = std::fs::metadata(&daemon.socket)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loaded_agent_is_set_to_the_sessions_options() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .set_config_option(&session.id, "mode", ConfigValue::Id("code".into()))
        .await
        .unwrap();
    client.prompt(&session.id, text("exit 1")).await.unwrap();
    until(&mut events, agent_exited).await;

    // The new agent starts in its default mode.
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let started = until(&mut events, agent_started).await;
    let Some(EventKind::AgentStarted { config_options, .. }) =
        started.last().map(|event| &event.kind)
    else {
        panic!("{started:?}");
    };
    assert_eq!(current(config_options, "mode"), "ask");
    until(&mut events, |kind| {
        matches!(
            kind,
            EventKind::Update(SessionUpdate::ConfigOptionUpdate(_))
        )
    })
    .await;
    let options = info(&client, &session.id).await.unwrap().config_options;
    assert_eq!(current(&options, "mode"), "code");
}

#[tokio::test(flavor = "multi_thread")]
async fn config_options_are_listed_and_set() {
    // An agent that reports the change before its response, or not at all,
    // yields one update. One that reports it after its response yields its
    // own notification as well.
    for (flags, updates) in [
        (&[][..], 1),
        (&["--silent-config"][..], 1),
        (&["--late-config-update"][..], 2),
    ] {
        let daemon = Daemon::start().await;
        let client = daemon.client().await;
        let (session, started) = daemon.new_session_with(&client, flags).await;
        assert_eq!(current(&session.config_options, "mode"), "ask");
        let mut events = client.subscribe(&session.id, started).await.unwrap();

        let other = daemon.client().await;
        let options = other
            .set_config_option(&session.id, "mode", ConfigValue::Id("code".into()))
            .await
            .unwrap();
        assert_eq!(current(&options, "mode"), "code");
        let listed = sessions(&other).await;
        assert_eq!(current(&listed[0].config_options, "mode"), "code");

        let error = other
            .set_config_option(&session.id, "mode", ConfigValue::Id("fly".into()))
            .await
            .unwrap_err();
        assert_eq!(remote_code(error), code::AGENT);
        let listed = sessions(&other).await;
        assert_eq!(current(&listed[0].config_options, "mode"), "code");

        client
            .prompt(&session.id, text("config mode"))
            .await
            .unwrap();
        let turn = until(&mut events, turn_ended).await;
        let reported: Vec<String> = turn
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::Update(SessionUpdate::ConfigOptionUpdate(update)) => {
                    Some(current(&update.config_options, "mode"))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            reported,
            vec!["code"; updates],
            "fake agent flags {flags:?}"
        );
        assert_eq!(chunks(&turn), ["code"]);
    }
}

/// A prompt sent after a config option change, without waiting for the
/// change's answer, reaches the agent after the change.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_sent_after_a_config_change_runs_with_the_new_value() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    // The connection sends its requests in the order they are first polled.
    let (set, prompted) = tokio::join!(
        client.set_config_option(&session.id, "mode", ConfigValue::Id("code".into())),
        client.prompt(&session.id, text("config mode")),
    );
    set.unwrap();
    prompted.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["code"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_content_reaches_the_agent() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    assert!(session.prompt_capabilities.image.is_some());
    assert!(session.prompt_capabilities.embedded_context.is_some());
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let prompt = vec![
        ContentBlock::Text(TextContent::new("describe")),
        ContentBlock::Image(ImageContent::new("iVBORw0KGgo=", "image/png")),
        ContentBlock::ResourceLink(ResourceLink::new("a.rs", "file:///a.rs")),
        ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                "notes",
                "file:///b.md",
            )),
        )),
    ];
    client.prompt(&session.id, prompt.clone()).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(
        turn[0].kind,
        EventKind::PromptStarted { prompt, from: None }
    );
    assert_eq!(
        chunks(&turn),
        ["text image:image/png link:file:///a.rs resource:file:///b.md"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_content_the_agent_does_not_accept_is_rejected() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session_with(&client, &["--text-only"]).await;
    let text = ContentBlock::Text(TextContent::new("describe"));
    let rejected = [
        vec![],
        vec![
            text.clone(),
            ContentBlock::Image(ImageContent::new("iVBORw0KGgo=", "image/png")),
        ],
        vec![
            text.clone(),
            ContentBlock::Audio(AudioContent::new("UklGRg==", "audio/wav")),
        ],
        vec![ContentBlock::Resource(EmbeddedResource::new(
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(
                "notes",
                "file:///b.md",
            )),
        ))],
    ];
    for prompt in rejected {
        let error = client.prompt(&session.id, prompt).await.unwrap_err();
        assert_eq!(remote_code(error), code::INVALID_REQUEST);
    }
    let listed = sessions(&client).await;
    assert_eq!(listed[0].state, SessionState::Idle);

    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let link = ContentBlock::ResourceLink(ResourceLink::new("a.rs", "file:///a.rs"));
    let prompt = vec![text, link];
    client.prompt(&session.id, prompt.clone()).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(turn[0].seq, started + 1);
    assert_eq!(
        turn[0].kind,
        EventKind::PromptStarted { prompt, from: None }
    );
    assert_eq!(chunks(&turn), ["text link:file:///a.rs"]);
}

/// The user and agent text a session's replayed history holds.
fn replayed(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Update(SessionUpdate::UserMessage(message)) => {
                match message.content.clone().take().as_deref() {
                    Some([ContentBlock::Text(text)]) => Some(format!("user: {}", text.text)),
                    _ => None,
                }
            }
            _ => chunk(event).map(|text| format!("agent: {text}")),
        })
        .collect()
}

fn agent_started(kind: &EventKind) -> bool {
    matches!(kind, EventKind::AgentStarted { .. })
}

fn agent_exited(kind: &EventKind) -> bool {
    matches!(kind, EventKind::AgentExited { .. })
}

#[tokio::test(flavor = "multi_thread")]
async fn workspace_add_refuses_what_is_not_a_new_directory() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let file = daemon.cwd().join("file");
    std::fs::write(&file, "").unwrap();
    for path in [
        PathBuf::from("relative"),
        file,
        daemon.cwd().join("missing"),
    ] {
        let error = client.add_workspace(path.clone()).await.unwrap_err();
        assert_eq!(remote_code(error), code::INVALID_REQUEST, "{path:?}");
    }
    assert!(workspaces(&client).await.is_empty());

    let workspace = client.add_workspace(daemon.cwd()).await.unwrap();
    assert_eq!(workspace.path, daemon.cwd());
    let error = client.add_workspace(daemon.cwd()).await.unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_REQUEST);
    assert_eq!(workspaces(&client).await, [workspace]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_with_sessions_cannot_be_removed() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let workspace = daemon.workspace(&client).await;

    let error = client.remove_workspace(&workspace.id).await.unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
    let error = client.remove_workspace("unknown").await.unwrap_err();
    assert_eq!(remote_code(error), code::NOT_FOUND);

    // An exited session still belongs to its workspace.
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("exit 1")).await.unwrap();
    until(&mut events, agent_exited).await;
    let error = client.remove_workspace(&workspace.id).await.unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
    assert_eq!(workspaces(&client).await, std::slice::from_ref(&workspace));

    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    client.remove_workspace(&workspace.id).await.unwrap();
    assert!(workspaces(&client).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_runs_in_its_workspace() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, _) = daemon.new_session(&client).await;
    let workspace = daemon.workspace(&client).await;
    assert_eq!(session.workspace_id, workspace.id);
    assert_eq!(session.cwd, workspace.path);

    let error = client
        .new_session(
            "unknown",
            fake_agent(&[]),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::NOT_FOUND);
    let error = client
        .new_session(
            &workspace.id,
            Vec::new(),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn workspaces_and_sessions_survive_a_restart() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let workspace = first.workspace(&client).await;
    let (titled, started) = first.new_session(&client).await;
    let mut events = client.subscribe(&titled.id, started).await.unwrap();
    client
        .prompt(&titled.id, text("say 1 0\ntitle Fix the build"))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;
    let (plain, _) = first.new_session(&client).await;
    // This agent never answers `initialize`, so its session never starts.
    let unstarted = client
        .new_session(
            &workspace.id,
            fake_agent(&["--hang-initialize"]),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let titled = info(&client, &titled.id).await.unwrap();
    assert_eq!(titled.title.as_deref(), Some("Fix the build"));
    drop(events);
    drop(client);
    first.stop().await;

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    assert_eq!(workspaces(&client).await, [workspace]);
    let saved = |session: SessionInfo| SessionInfo {
        state: SessionState::Exited,
        prompt_capabilities: PromptCapabilities::default(),
        config_options: Vec::new(),
        available_commands: Vec::new(),
        ..session
    };
    assert_eq!(
        sessions(&client).await,
        [saved(titled), saved(plain)],
        "the session that never started ({}) is gone",
        unstarted.id
    );

    // Sessions created after the restart follow the saved ones.
    let (new, _) = second.new_session(&client).await;
    let ids: Vec<String> = sessions(&client)
        .await
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(ids[2], new.id);
}

#[tokio::test(flavor = "multi_thread")]
async fn loading_a_restored_session_keeps_its_stored_history() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let (session, started) = first
        .new_session_with(&client, &["--no-resume-replay"])
        .await;
    let agent_session_id = session.agent_session_id.clone().unwrap();
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("say 2 0\ntitle Replay me"))
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    let last_seq = turn.last().unwrap().seq;
    drop(events);
    drop(client);
    first.stop().await;

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    // The history comes from the database with its seqs, and the
    // subscription stays open for what follows.
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let mut history = Vec::new();
    while history.len() < usize::try_from(last_seq).unwrap() {
        history.push(next(&mut events).await);
    }
    assert_contiguous(&history, 1);
    assert_eq!(history[0].seq, 1);
    assert!(matches!(&history[0].kind, EventKind::AgentStarted { .. }));
    assert_eq!(
        replayed(&history),
        [
            "user: say 2 0\ntitle Replay me",
            "agent: chunk-1",
            "agent: chunk-2"
        ]
    );
    assert!(
        !history
            .iter()
            .any(|event| matches!(event.kind, EventKind::SessionClosed))
    );
    let mut middle = client.subscribe(&session.id, started).await.unwrap();
    assert_eq!(next(&mut middle).await.seq, started + 1);

    let loaded = client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(loaded.state, SessionState::Starting);
    // The fake agent only accepts a resume without a replay cursor.
    let restarted = next(&mut events).await;
    assert_eq!(restarted.seq, last_seq + 1);
    assert!(matches!(
        &restarted.kind,
        EventKind::AgentStarted { agent_session_id: id, .. } if *id == agent_session_id
    ));
    let loaded = info(&client, &loaded.id).await.unwrap();
    assert_eq!(loaded.state, SessionState::Idle);
    assert_eq!(loaded.title.as_deref(), Some("Replay me"));
    assert_eq!(loaded.config_options, session.config_options);
    assert_eq!(loaded.available_commands, session.available_commands);

    client.prompt(&loaded.id, text("say 1 0")).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["chunk-1"]);
    assert_contiguous(&turn, last_seq + 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn loading_a_restored_session_recovers_missing_history_from_the_agent() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let (session, started) = first.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("say 1 0")).await.unwrap();
    until(&mut events, turn_ended).await;
    drop(events);
    drop(client);
    first.stop().await;

    let database = rusqlite::Connection::open(first.dir.path().join("oxd.sock.db")).unwrap();
    database
        .execute("DELETE FROM events WHERE session_id = ?1", [&session.id])
        .unwrap();
    drop(database);

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let recovered = until(&mut events, agent_started).await;
    assert_eq!(replayed(&recovered), ["user: say 1 0", "agent: chunk-1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_daemon_stores_the_end_of_the_turn_it_interrupted() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let (session, started) = first.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("permission"))
        .await
        .unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::PermissionRequested { .. })
    })
    .await;
    first.stop_signal();
    // The daemon records the end of the turn as it closes the session, and
    // the subscriber receives it and the session's close before oxd exits.
    let ended = until(&mut events, turn_ended).await.pop().unwrap();
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::SessionClosed
    ));
    assert!(first.wait_for_exit().await.success());
    drop(events);
    drop(client);

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let history = until(&mut events, turn_ended).await;
    let [.., resolved, stored] = history.as_slice() else {
        panic!("{history:?}");
    };
    assert_eq!(stored, &ended);
    assert!(
        matches!(
            &resolved.kind,
            EventKind::PermissionResolved {
                outcome: RequestPermissionOutcome::Cancelled(_),
                ..
            }
        ),
        "{resolved:?}"
    );
    assert!(
        matches!(
            &stored.kind,
            EventKind::TurnEnded { error: Some(error), .. } if error == "the session was closed"
        ),
        "{stored:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_history_that_a_crash_left_open_ends_its_turn() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let (session, started) = first.new_session(&client).await;
    drop(client);
    first.stop().await;
    // What a daemon killed mid-turn leaves in the database: a turn that
    // started, and a permission request and an elicitation nobody answered.
    let database = rusqlite::Connection::open(first.dir.path().join("oxd.sock.db")).unwrap();
    for (seq, kind) in [
        (started + 1, EventKind::TurnStarted),
        (
            started + 2,
            EventKind::PermissionRequested {
                request_id: "1".into(),
                title: "Tool".into(),
                description: None,
                subject: None,
                options: Vec::new(),
            },
        ),
        (
            started + 3,
            serde_json::from_value(json!({
                "type": "elicitation_requested",
                "data": {
                    "request_id": "2",
                    "message": "Order",
                    "tool_call_id": null,
                    "requested_schema": {"type": "object", "properties": {}}
                }
            }))
            .unwrap(),
        ),
    ] {
        database
            .execute(
                "INSERT INTO events (session_id, seq, recorded_at, kind) VALUES (?1, ?2, 0, ?3)",
                (
                    &session.id,
                    i64::try_from(seq).unwrap(),
                    serde_json::to_string(&kind).unwrap(),
                ),
            )
            .unwrap();
    }
    drop(database);

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let rest = until(&mut events, turn_ended).await;
    assert_contiguous(&rest, started + 1);
    assert!(matches!(&rest[0].kind, EventKind::TurnStarted));
    assert!(matches!(
        &rest[3].kind,
        EventKind::PermissionResolved {
            outcome: RequestPermissionOutcome::Cancelled(_),
            ..
        }
    ));
    assert_eq!(
        rest[4].kind,
        EventKind::ElicitationResolved {
            request_id: "2".into(),
            action: ElicitationAction::Cancel,
        }
    );
    assert!(matches!(
        &rest.last().unwrap().kind,
        EventKind::TurnEnded { error: Some(error), .. } if error == "oxd stopped during the turn"
    ));

    // The next events continue the seqs.
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let restarted = next(&mut events).await;
    assert_eq!(restarted.seq, rest.last().unwrap().seq + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_session_loses_its_history() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let (session, _) = first.new_session(&client).await;
    let (kept, _) = first.new_session(&client).await;
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    drop(client);
    first.stop().await;

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    let ids: Vec<String> = sessions(&client).await.into_iter().map(|s| s.id).collect();
    assert_eq!(ids, [kept.id.as_str()]);
    let error = client.subscribe(&session.id, 0).await.unwrap_err();
    assert_eq!(remote_code(error), code::NOT_FOUND);
    let mut events = client.subscribe(&kept.id, 0).await.unwrap();
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::AgentStarted { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn loading_a_session_whose_agent_exited_keeps_its_history() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("say 1 0")).await.unwrap();
    until(&mut events, turn_ended).await;
    client.prompt(&session.id, text("exit 1")).await.unwrap();
    let exited = until(&mut events, agent_exited).await.pop().unwrap();

    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let restarted = next(&mut events).await;
    assert!(agent_started(&restarted.kind), "{restarted:?}");
    // The replay is not recorded: nothing comes between the exit and the
    // second start.
    assert_eq!(restarted.seq, exited.seq + 1);

    let mut all = client.subscribe(&session.id, 0).await.unwrap();
    let mut history = Vec::new();
    while history.len() < usize::try_from(restarted.seq).unwrap() {
        history.push(next(&mut all).await);
    }
    // The history holds the echo of the prompt and its reply once.
    assert_eq!(replayed(&history), ["user: say 1 0", "agent: chunk-1"]);
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );

    client.prompt(&session.id, text("say 1 0")).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["chunk-1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loaded_agent_gets_new_permission_request_ids() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let requested = |kind: &EventKind| matches!(kind, EventKind::PermissionRequested { .. });

    client
        .prompt(&session.id, text("permission"))
        .await
        .unwrap();
    let history = until(&mut events, requested).await;
    let EventKind::PermissionRequested {
        request_id: old_id, ..
    } = &history.last().unwrap().kind
    else {
        unreachable!()
    };
    client.permit(&session.id, old_id, "allow").await.unwrap();
    until(&mut events, turn_ended).await;
    client.prompt(&session.id, text("exit 1")).await.unwrap();
    until(&mut events, agent_exited).await;

    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    until(&mut events, agent_started).await;
    client
        .prompt(&session.id, text("permission"))
        .await
        .unwrap();
    let history = until(&mut events, requested).await;
    let EventKind::PermissionRequested { request_id, .. } = &history.last().unwrap().kind else {
        unreachable!()
    };
    assert_ne!(request_id, old_id);

    // An answer to the first agent's request does not answer the second's.
    let error = client
        .permit(&session.id, old_id, "reject")
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_exited_session_that_started_can_be_loaded() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let error = client
        .load_session("unknown", BTreeMap::new())
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::NOT_FOUND);

    let (running, _) = daemon.new_session(&client).await;
    let error = client
        .load_session(&running.id, BTreeMap::new())
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
    assert_eq!(
        info(&client, &running.id).await.unwrap().state,
        SessionState::Idle
    );

    let workspace = daemon.workspace(&client).await;
    let unstarted = client
        .new_session(
            &workspace.id,
            vec![daemon.cwd().join("missing").to_string_lossy().into()],
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let mut events = client.subscribe(&unstarted.id, 0).await.unwrap();
    let exited = next(&mut events).await;
    assert!(
        matches!(exited.kind, EventKind::AgentExited { error: Some(_) }),
        "{exited:?}"
    );
    assert_eq!(
        info(&client, &unstarted.id).await.unwrap().state,
        SessionState::Exited
    );
    let error = client
        .load_session(&unstarted.id, BTreeMap::new())
        .await
        .unwrap_err();
    assert_eq!(remote_code(error), code::INVALID_STATE);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_without_session_capabilities_cannot_run_sessions() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let workspace = daemon.workspace(&client).await;
    let session = client
        .new_session(
            &workspace.id,
            fake_agent(&["--no-sessions"]),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let exited = next(&mut events).await;
    assert!(
        matches!(&exited.kind, EventKind::AgentExited { error: Some(error) } if error.contains("does not support sessions")),
        "{exited:?}"
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Exited
    );
}

impl Daemon {
    /// Creates a session of the fake agent with `args`, given its first
    /// prompt and option values.
    async fn new_session_prompted(
        &self,
        client: &Client,
        args: &[&str],
        config: Vec<ConfigSetting>,
        prompt: Vec<ContentBlock>,
    ) -> Result<SessionInfo, Error> {
        let workspace = self.workspace(client).await;
        client
            .new_session(
                &workspace.id,
                fake_agent(args),
                BTreeMap::new(),
                Checkout::Workspace,
                config,
                Some(prompt),
            )
            .await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_sessions_first_prompt_shows_before_its_agent_starts() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    // The agent blocks opening the FIFO for writing until the test opens it
    // for reading.
    let fifo = daemon.dir.path().join("start");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    let session = daemon
        .new_session_prompted(
            &client,
            &["--hold", fifo.to_str().unwrap()],
            Vec::new(),
            text("text \"hello\"\n  text   \"!\""),
        )
        .await
        .unwrap();
    assert_eq!(session.state, SessionState::Starting);
    assert_eq!(session.title.as_deref(), Some("text \"hello\" text \"!\""));
    let listed = sessions(&client).await;
    assert_eq!(
        listed[0].title.as_deref(),
        Some("text \"hello\" text \"!\"")
    );
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let first = next(&mut events).await;
    assert_eq!(
        first.kind,
        EventKind::PromptStarted {
            prompt: text("text \"hello\"\n  text   \"!\""),
            from: None,
        }
    );
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Starting
    );

    let reader = tokio::task::spawn_blocking(move || {
        let mut held = std::fs::File::open(fifo).unwrap();
        std::io::Read::read_to_end(&mut held, &mut Vec::new()).unwrap();
    });
    assert!(agent_started(&next(&mut events).await.kind));
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn).concat(), "hello!");
    assert!(matches!(
        turn.last().unwrap().kind,
        EventKind::TurnEnded { error: None, .. }
    ));
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();
    reader.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_session_sets_its_options_before_its_first_prompt() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = daemon
        .new_session_prompted(
            &client,
            &[],
            vec![ConfigSetting {
                config_id: "mode".into(),
                value: ConfigValue::Id("code".into()),
            }],
            text("config mode"),
        )
        .await
        .unwrap();
    let turn = until_turn_ended(&client, &session.id, 0).await;
    assert_eq!(chunks(&turn), ["code"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_first_prompt_ends_when_the_agent_does_not_start() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let session = daemon
        .new_session_prompted(&client, &["--no-sessions"], Vec::new(), text("hello"))
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    assert!(matches!(
        next(&mut events).await.kind,
        EventKind::PromptStarted { .. }
    ));
    let ended = next(&mut events).await;
    assert!(
        matches!(&ended.kind, EventKind::TurnEnded { error: Some(error), .. } if error.contains("does not support sessions")),
        "{ended:?}"
    );
    assert!(agent_exited(&next(&mut events).await.kind));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_first_prompt_with_content_beyond_text_and_links_is_refused() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let refused = refusal(
        daemon
            .new_session_prompted(
                &client,
                &[],
                Vec::new(),
                vec![
                    ContentBlock::Text(TextContent::new("describe")),
                    ContentBlock::Image(ImageContent::new("iVBORw0KGgo=", "image/png")),
                ],
            )
            .await,
    );
    assert_eq!(refused.code, code::INVALID_REQUEST);
    assert!(sessions(&client).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_resumed_while_its_agent_is_working_runs_until_the_agent_is_idle() {
    let mut first = Daemon::start().await;
    let client = first.client().await;
    let workspace = first.workspace(&client).await;
    let session = client
        .new_session(
            &workspace.id,
            fake_agent(&["--busy-on-resume"]),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    until(&mut events, agent_started).await;
    client.prompt(&session.id, text("pid")).await.unwrap();
    until(&mut events, turn_ended).await;
    drop(events);
    drop(client);
    first.stop().await;

    let second = first.again(Options::default()).await;
    let client = second.client().await;
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    // The history holds the first agent's start; the second follows it.
    until(&mut events, agent_started).await;
    let started_seq = until(&mut events, agent_started).await.last().unwrap().seq;
    // The agent reported `running` on resume: the session is running, and no
    // turn began that the history saw.
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Running
    );
    client.cancel(&session.id).await.unwrap();
    let rest = until(&mut events, |kind| {
        matches!(
            kind,
            EventKind::Update(SessionUpdate::StateUpdate(StateUpdate::Idle(_)))
        )
    })
    .await;
    assert_eq!(update_names(&rest), ["idle"]);
    assert_eq!(
        info(&client, &session.id).await.unwrap().state,
        SessionState::Idle
    );
    let mut all = client.subscribe(&session.id, 0).await.unwrap();
    let mut history = Vec::new();
    while history.len() < usize::try_from(started_seq).unwrap() + 1 {
        history.push(next(&mut all).await);
    }
    assert!(
        !history
            .iter()
            .any(|event| event.kind == EventKind::TurnStarted)
    );
    client.prompt(&session.id, text("pid")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn exited_sessions_do_not_keep_the_daemon_alive() {
    let mut daemon = Daemon::start_with(Options {
        idle_exit_ms: Some(IDLE_EXIT_MS),
        ..Options::default()
    })
    .await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client.prompt(&session.id, text("exit 0")).await.unwrap();
    until(&mut events, agent_exited).await;
    drop(events);
    drop(client);
    assert!(daemon.wait_for_exit().await.success());

    // The session was saved, so the next daemon lists it.
    let daemon = daemon.again(Options::default()).await;
    let client = daemon.client().await;
    let sessions = sessions(&client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session.id);
    assert_eq!(sessions[0].state, SessionState::Exited);
}
/// A watch's first message lists the workspaces and sessions. After it, a
/// watch sends a session only when its info changed, and never the sessions
/// that did not change.
#[tokio::test(flavor = "multi_thread")]
async fn watch_sends_only_sessions_whose_info_changed() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (quiet, _) = daemon.new_session(&client).await;
    let (busy, started) = daemon.new_session(&client).await;
    let workspace = daemon.workspace(&client).await;
    let mut watch = client.watch().await.unwrap();
    let mut listing = Listing::default();
    for change in next_changes(&mut watch).await {
        apply(&mut listing, change);
    }
    assert_eq!(listing.workspaces, [workspace]);
    assert_eq!(listing.sessions, [quiet, busy.clone()]);

    let mut events = client.subscribe(&busy.id, started).await.unwrap();
    client
        .prompt(&busy.id, text("title One\nsay 200 0\ntitle Two"))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;
    client
        .delete(&busy.id, false, BTreeMap::new())
        .await
        .unwrap();
    let mut last = busy.clone();
    loop {
        for change in next_changes(&mut watch).await {
            match change {
                ListingChange::Session { session } => {
                    assert_eq!(session.id, busy.id);
                    assert_ne!(*session, last, "an unchanged session was sent");
                    last = *session;
                }
                ListingChange::SessionRemoved { session_id } => {
                    assert_eq!(session_id, busy.id);
                    assert_eq!(last.title.as_deref(), Some("Two"));
                    return;
                }
                change => panic!("unexpected change {change:?}"),
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn session_info_updates_set_the_title() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    assert_eq!(session.title, None);
    let mut watch = listings(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    for (script, title) in [
        ("title Fix the build", Some("Fix the build")),
        ("touch", Some("Fix the build")),
        ("title-null", None),
    ] {
        client.prompt(&session.id, text(script)).await.unwrap();
        until(&mut events, turn_ended).await;
        assert_eq!(
            info(&client, &session.id).await.unwrap().title.as_deref(),
            title,
            "{script}"
        );
        watch
            .until(|listing| {
                listing.sessions[0].title.as_deref() == title
                    && listing.sessions[0].state == SessionState::Idle
            })
            .await;
    }
}

/// An agent's `set_title` call on the `ox` MCP server oxd gives it retitles
/// its session. Needs `ox-mcp` built beside `oxd`.
#[tokio::test(flavor = "multi_thread")]
async fn agent_sets_title_through_ox_mcp_server() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(
            &session.id,
            text(r#"mcp set_title {"title": "Renamed  by\tagent"}"#),
        )
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["Title set."]);
    assert_eq!(
        info(&client, &session.id).await.unwrap().title.as_deref(),
        Some("Renamed by agent")
    );
}

/// An agent gets Ox's instructions in its system prompt when it takes one,
/// and otherwise before its first prompt.
#[tokio::test(flavor = "multi_thread")]
async fn agent_gets_ox_instructions() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    for args in [&[][..], &["--no-system-prompt"]] {
        let (session, started) = daemon.new_session_with(&client, args).await;
        let mut events = client.subscribe(&session.id, started).await.unwrap();
        client
            .prompt(&session.id, text("instructions"))
            .await
            .unwrap();
        let turn = until(&mut events, turn_ended).await;
        let instructions = chunks(&turn).concat();
        assert!(
            instructions.starts_with("You are running in Ox")
                && instructions.contains("`set_title`"),
            "{args:?}: {instructions}"
        );
    }
}

/// After a watch's `unsubscribe` is answered, the connection receives no more
/// listings.
#[tokio::test(flavor = "multi_thread")]
async fn unsubscribe_ends_a_watch() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut raw = Raw::connect(&daemon).await;
    let watch = raw.send("watch", Value::Null).await;
    raw.answer(watch).await;
    let first = raw.read().await;
    assert_eq!(first["subscription"], watch);
    assert_eq!(first["changes"][1]["session"]["id"], session.id.as_str());
    let unsubscribe = raw
        .send("unsubscribe", json!({"subscription": watch}))
        .await;
    raw.answer(unsubscribe).await;
    raw.send(
        "session.subscribe",
        json!({"session_id": session.id, "after_seq": started}),
    )
    .await;

    // A turn held open changes the session's state long enough for a watch
    // to send the list.
    let mut running = listings(&client).await;
    client.prompt(&session.id, text("wait")).await.unwrap();
    running
        .until(|listing| listing.sessions[0].state == SessionState::Running)
        .await;
    client.cancel(&session.id).await.unwrap();
    let messages = raw.messages_until(raw_turn_ended).await;
    assert!(
        messages
            .iter()
            .all(|message| message.get("changes").is_none()),
        "{messages:?}"
    );
}

/// A session whose fake agent reports background work.
async fn parking_session(daemon: &Daemon, client: &Client, args: &[&str]) -> (SessionInfo, u64) {
    let args: Vec<&str> = ["--background"]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    daemon.new_session_with(client, &args).await
}

/// Waits until the watch lists session `id` as `exited`.
async fn parked(watch: &mut Listings, id: &str) {
    watch
        .until(|listing| state_in(&listing.sessions, id) == Some(SessionState::Exited))
        .await;
}

/// The message chunks in a session's first `seq` events.
async fn chunks_before(client: &Client, id: &str, seq: u64) -> Vec<String> {
    let mut events = client.subscribe(id, 0).await.unwrap();
    let mut history = Vec::new();
    while history.len() < usize::try_from(seq).unwrap() {
        history.push(next(&mut events).await);
    }
    chunks(&history).into_iter().map(String::from).collect()
}

/// The seq of the `agent_parked` event in a session's history.
async fn parked_seq(client: &Client, id: &str) -> u64 {
    let mut events = client.subscribe(id, 0).await.unwrap();
    until(&mut events, |kind| matches!(kind, EventKind::AgentParked))
        .await
        .pop()
        .unwrap()
        .seq
}

/// A daemon that parks a session as soon as it is quiet and unviewed. The
/// park delay itself is established by the daemon's unit tests.
async fn parking_daemon(idle_exit_ms: Option<u64>) -> Daemon {
    Daemon::start_with(Options {
        idle_exit_ms,
        park_after_ms: Some(0),
        ..Options::default()
    })
    .await
}

/// Creates a session whose fake agent reports background work, with `args`,
/// and waits until the daemon has parked it. Nothing views the session, so
/// it is parked as soon as its agent reports that no work is running.
async fn parked_session(
    daemon: &Daemon,
    client: &Client,
    watch: &mut Listings,
    args: &[&str],
) -> SessionInfo {
    let workspace = daemon.workspace(client).await;
    let args: Vec<&str> = ["--background"]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    let session = client
        .new_session(
            &workspace.id,
            fake_agent(&args),
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    parked(watch, &session.id).await;
    parked_seq(client, &session.id).await;
    session
}

/// The fake agent's arguments for a session whose background work `job`
/// runs until a prompt of `background-end job`, which keeps it from being
/// parked until then.
const BUSY: [&str; 2] = ["--background-running", "job"];

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_unviewed_session_is_parked_and_its_agent_exits() {
    let daemon = parking_daemon(None).await;
    let client = daemon.client().await;
    let mut watch = listings(&client).await;
    let agent = ExitWatch::new(daemon.dir.path());
    parked_session(&daemon, &client, &mut watch, &agent.arg()).await;
    agent.exited().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_parked_session_loads_again() {
    let daemon = parking_daemon(None).await;
    let client = daemon.client().await;
    let mut watch = listings(&client).await;
    let (session, started) = parking_session(&daemon, &client, &BUSY).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("say 1 0\nbackground-end job"))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;
    drop(events);
    parked(&mut watch, &session.id).await;
    let seq = parked_seq(&client, &session.id).await;

    // The parked session loads like any exited one, and keeps its history.
    assert_eq!(chunks_before(&client, &session.id, seq).await, ["chunk-1"]);
    let mut events = client.subscribe(&session.id, seq).await.unwrap();
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let restarted = next(&mut events).await;
    assert!(agent_started(&restarted.kind), "{restarted:?}");
    assert_eq!(restarted.seq, seq + 1);
    client.prompt(&session.id, text("say 1 0")).await.unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["chunk-1"]);
}

/// The sessions a test expects to stay unparked become quiet before a
/// control session does, so a wrongful park of one comes before the
/// control's park.
#[tokio::test(flavor = "multi_thread")]
async fn a_viewed_session_is_not_parked() {
    let daemon = parking_daemon(None).await;
    let client = daemon.client().await;
    let mut watch = listings(&client).await;
    let (viewed, started) = parking_session(&daemon, &client, &BUSY).await;
    let mut view = client.subscribe(&viewed.id, started).await.unwrap();
    client
        .prompt(&viewed.id, text("background-end job"))
        .await
        .unwrap();
    until(&mut view, turn_ended).await;
    parked_session(&daemon, &client, &mut watch, &[]).await;
    assert_eq!(
        state_in(&watch.current.sessions, &viewed.id),
        Some(SessionState::Idle)
    );

    drop(view);
    parked(&mut watch, &viewed.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_background_work_is_not_parked() {
    let daemon = parking_daemon(None).await;
    let client = daemon.client().await;
    let mut watch = listings(&client).await;
    let (busy, _) = parking_session(&daemon, &client, &BUSY).await;
    parked_session(&daemon, &client, &mut watch, &[]).await;
    assert_eq!(
        state_in(&watch.current.sessions, &busy.id),
        Some(SessionState::Idle)
    );

    let mut events = client.subscribe(&busy.id, 0).await.unwrap();
    client
        .prompt(&busy.id, text("background-end job"))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;
    drop(events);
    parked(&mut watch, &busy.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_whose_agent_reports_no_background_work_is_not_parked() {
    let daemon = parking_daemon(None).await;
    let client = daemon.client().await;
    let mut watch = listings(&client).await;
    let (silent, _) = daemon.new_session(&client).await;
    parked_session(&daemon, &client, &mut watch, &[]).await;
    assert_eq!(
        state_in(&watch.current.sessions, &silent.id),
        Some(SessionState::Idle)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_whose_sessions_are_parked_exits() {
    let mut daemon = parking_daemon(Some(IDLE_EXIT_MS)).await;
    let client = daemon.client().await;
    let mut watch = listings(&client).await;
    parked_session(&daemon, &client, &mut watch, &[]).await;
    drop(watch);
    drop(client);
    assert!(daemon.wait_for_exit().await.success());
    assert!(!daemon.socket.exists());
}

/// Waits for a directory watch to list exactly `expected`, as name and kind
/// pairs.
async fn entries_until(watch: &mut Stream<Vec<DirEntry>>, expected: &[(&str, EntryKind)]) {
    let expected: Vec<DirEntry> = expected
        .iter()
        .map(|&(name, kind)| DirEntry {
            name: name.into(),
            kind,
        })
        .collect();
    loop {
        let entries = timeout(TIMEOUT, watch.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {expected:?}"))
            .expect("the directory watch ended");
        if entries == expected {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn directory_watch_follows_changes() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".DS_Store"), "").unwrap();
    let mut watch = client.watch_directory(root).await.unwrap();
    let first = timeout(TIMEOUT, watch.next()).await.unwrap().unwrap();
    assert_eq!(
        first,
        [
            DirEntry {
                name: "a.txt".into(),
                kind: EntryKind::File,
            },
            DirEntry {
                name: "sub".into(),
                kind: EntryKind::Directory,
            },
        ]
    );

    std::fs::write(root.join("b.txt"), "b").unwrap();
    let file = EntryKind::File;
    let directory = EntryKind::Directory;
    entries_until(
        &mut watch,
        &[("a.txt", file), ("b.txt", file), ("sub", directory)],
    )
    .await;
    std::fs::rename(root.join("a.txt"), root.join("c.txt")).unwrap();
    entries_until(
        &mut watch,
        &[("b.txt", file), ("c.txt", file), ("sub", directory)],
    )
    .await;
    std::fs::remove_dir(root.join("sub")).unwrap();
    entries_until(&mut watch, &[("b.txt", file), ("c.txt", file)]).await;
}

/// A directory watch on a path that does not exist yet, such as the worktree
/// of a session that is starting, lists the directory once it is created.
#[tokio::test(flavor = "multi_thread")]
async fn directory_watch_lists_a_directory_once_it_exists() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("parent");
    let path = parent.join("worktree");
    let mut watch = client.watch_directory(&path).await.unwrap();
    let first = timeout(TIMEOUT, watch.next()).await.unwrap().unwrap();
    assert_eq!(first, []);

    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("a.txt"), "a").unwrap();
    entries_until(&mut watch, &[("a.txt", EntryKind::File)]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn file_watch_follows_edits_replacement_and_recreation() {
    use ox_protocol::FileContent;
    async fn text(watch: &mut Stream<ox_protocol::FileContent>, expected: &str) {
        timeout(TIMEOUT, async {
            loop {
                if watch.next().await.unwrap()
                    == (FileContent::Text {
                        text: expected.into(),
                    })
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, "first").unwrap();
    let mut watch = client.watch_file(&path).await.unwrap();
    text(&mut watch, "first").await;
    std::fs::write(&path, "edited").unwrap();
    text(&mut watch, "edited").await;
    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, "replacement").unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    text(&mut watch, "replacement").await;
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        timeout(TIMEOUT, watch.next()).await.unwrap().unwrap(),
        FileContent::Error { .. }
    ));
    std::fs::write(&path, "recreated").unwrap();
    text(&mut watch, "recreated").await;
    for bytes in [vec![0xff], vec![0]] {
        // A whole replacement leaves no empty file between the two contents
        // for the watch to read.
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(matches!(
            timeout(TIMEOUT, watch.next()).await.unwrap().unwrap(),
            FileContent::Error { .. }
        ));
    }
    let fifo = dir.path().join("fifo");
    assert!(
        tokio::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .await
            .unwrap()
            .success()
    );
    let mut special = client.watch_file(&fifo).await.unwrap();
    assert!(matches!(
        timeout(TIMEOUT, special.next()).await.unwrap().unwrap(),
        FileContent::Error { .. }
    ));
    let mut directory = client.watch_file(dir.path()).await.unwrap();
    assert!(matches!(
        timeout(TIMEOUT, directory.next()).await.unwrap().unwrap(),
        FileContent::Error { .. }
    ));
    assert!(
        client
            .watch_file(std::path::Path::new("relative"))
            .await
            .is_err()
    );
}

/// The listed session's unseen outcome.
async fn unseen(client: &Client, session_id: &str) -> Option<Unseen> {
    info(client, session_id).await.unwrap().unseen
}

/// Prompts `script` and returns once its turn has ended.
async fn turn(client: &Client, events: &mut Subscription, session_id: &str, script: &str) {
    client.prompt(session_id, text(script)).await.unwrap();
    until(events, turn_ended).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_ends_out_of_focus_is_unseen_until_the_session_is_focused() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    assert_eq!(unseen(&client, &session.id).await, None);
    turn(&client, &mut events, &session.id, "say 1 0").await;
    assert_eq!(unseen(&client, &session.id).await, Some(Unseen::Finished));

    client.focus(Some(&session.id)).await.unwrap();
    assert_eq!(unseen(&client, &session.id).await, None);
    turn(&client, &mut events, &session.id, "say 1 0").await;
    assert_eq!(unseen(&client, &session.id).await, None);

    client.focus(None).await.unwrap();
    turn(&client, &mut events, &session.id, "say 1 0").await;
    assert_eq!(unseen(&client, &session.id).await, Some(Unseen::Finished));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_turn_is_not_unseen_and_an_error_outlasts_a_finish() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    client
        .prompt(&session.id, text("say 1 0\nwait"))
        .await
        .unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::Update(SessionUpdate::AgentMessageChunk(_)))
    })
    .await;
    client.cancel(&session.id).await.unwrap();
    until(&mut events, turn_ended).await;
    assert_eq!(unseen(&client, &session.id).await, None);

    turn(&client, &mut events, &session.id, "reject not today").await;
    assert_eq!(unseen(&client, &session.id).await, Some(Unseen::Error));
    turn(&client, &mut events, &session.id, "say 1 0").await;
    assert_eq!(unseen(&client, &session.id).await, Some(Unseen::Error));

    let (exiting, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&exiting.id, started).await.unwrap();
    client.prompt(&exiting.id, text("exit 1")).await.unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::AgentExited { .. })
    })
    .await;
    assert_eq!(unseen(&client, &exiting.id).await, Some(Unseen::Error));
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_focusing_connection_ends_its_focus() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (session, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&session.id, started).await.unwrap();
    let mut watch = client.watch().await.unwrap();
    next_changes(&mut watch).await;
    let focusing = daemon.client().await;
    focusing.focus(Some(&session.id)).await.unwrap();
    turn(&client, &mut events, &session.id, "say 1 0").await;
    assert_eq!(unseen(&client, &session.id).await, None);

    // The daemon lists the workspaces again once a client has disconnected,
    // so the workspace turning into a Git working tree shows when it has.
    init(&daemon.cwd());
    drop(focusing);
    loop {
        let changes = next_changes(&mut watch).await;
        if changes.iter().any(|change| {
            matches!(change, ListingChange::Workspace { workspace } if workspace.git_working_tree)
        }) {
            break;
        }
    }
    turn(&client, &mut events, &session.id, "say 1 0").await;
    assert_eq!(unseen(&client, &session.id).await, Some(Unseen::Finished));
}

#[tokio::test(flavor = "multi_thread")]
async fn unseen_outcomes_survive_a_restart() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let (finished, started) = daemon.new_session(&client).await;
    let mut events = client.subscribe(&finished.id, started).await.unwrap();
    turn(&client, &mut events, &finished.id, "say 1 0").await;
    // A turn still open when oxd stops ends with an error.
    let (cut_off, _) = daemon.new_session(&client).await;
    client.prompt(&cut_off.id, text("wait")).await.unwrap();
    drop(events);
    drop(client);

    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    assert_eq!(unseen(&client, &finished.id).await, Some(Unseen::Finished));
    assert_eq!(unseen(&client, &cut_off.id).await, Some(Unseen::Error));
}

#[tokio::test(flavor = "multi_thread")]
async fn focusing_an_unknown_session_is_refused() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let error = refusal(client.focus(Some("missing")).await);
    assert_eq!(error.code, code::NOT_FOUND);
}
