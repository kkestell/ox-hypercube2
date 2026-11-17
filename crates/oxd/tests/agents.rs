//! The `ox` tools agents use to work with other sessions: messaging them,
//! reading their transcripts, listing them, and asking the user to start
//! one. The fake agent calls each tool through `ox-mcp` with its
//! `mcp <tool> <JSON>` directive and reports the result as a chunk. Needs
//! `ox-mcp` built beside `oxd`.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::Path;

use ox_protocol::schema::{ContentBlock, SessionUpdate, TextContent, UserMessage};
use ox_protocol::{
    Checkout, Client, ConfigSetting, ConfigValue, Event, EventKind, Harness, ListingChange,
    RequestedCheckout, Sender, SessionInfo, SessionRequest, SessionState, Subscription,
};
use serde_json::json;
use tempfile::TempDir;

mod common;

use common::*;

/// Creates a session of the fake agent with `args` in `workspace`, with
/// `env` for its agent, waits for the agent to start, and returns the
/// session and a subscription to it after its `agent_started` event.
async fn new_session(
    client: &Client,
    workspace: &str,
    args: &[&str],
    env: BTreeMap<String, String>,
) -> (SessionInfo, Subscription) {
    let command = std::iter::once(FAKE_AGENT)
        .chain(args.iter().copied())
        .map(String::from)
        .collect();
    let session = client
        .new_session(
            workspace,
            command,
            env,
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 0).await.unwrap();
    let started = next(&mut events).await;
    assert!(
        matches!(started.kind, EventKind::AgentStarted { .. }),
        "{started:?}"
    );
    (info(client, &session.id).await.unwrap(), events)
}

/// The prompt of the directive that calls `ox` tool `tool` with `arguments`.
fn call(tool: &str, arguments: &serde_json::Value) -> Vec<ContentBlock> {
    text(&format!("mcp {tool} {arguments}"))
}

fn chunks(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Update(SessionUpdate::AgentMessageChunk(chunk)) => match &chunk.content {
                ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Prompts `session` to call `ox` tool `tool` and returns the tool's
/// answer, once the turn has ended.
async fn tool(
    client: &Client,
    session: &str,
    events: &mut Subscription,
    tool: &str,
    arguments: serde_json::Value,
) -> String {
    client
        .prompt(session, call(tool, &arguments))
        .await
        .unwrap();
    let turn = until(events, turn_ended).await;
    chunks(&turn).concat()
}

/// The text of the user message the agent echoed among `events`.
fn echoed(events: &[Event]) -> String {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::Update(SessionUpdate::UserMessage(UserMessage { content, .. })) => {
                let ox_protocol::MaybeUndefined::Value(content) = content else {
                    return None;
                };
                match &content[0] {
                    ContentBlock::Text(TextContent { text, .. }) => Some(text.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .expect("the agent echoed the prompt")
}

/// Waits until session `id` is listed in a state `done` accepts.
async fn until_state(client: &Client, id: &str, done: impl Fn(SessionState) -> bool) {
    let mut watch = client.watch().await.unwrap();
    loop {
        for change in next_changes(&mut watch).await {
            if let ListingChange::Session { session } = change
                && session.id == id
                && done(session.state)
            {
                return;
            }
        }
    }
}

/// A sender titled `Sender`, whose agent gets `sender_env`, and a recipient
/// whose fake agent gets `recipient_args`, both idle in the daemon's
/// directory, with the subscriptions after their `agent_started`.
async fn pair(
    daemon: &Daemon,
    client: &Client,
    sender_env: BTreeMap<String, String>,
    recipient_args: &[&str],
) -> ((SessionInfo, Subscription), (SessionInfo, Subscription)) {
    let workspace = workspace_id(client, &daemon.cwd()).await;
    let sender = new_session(client, &workspace, &[], sender_env).await;
    client.set_title(&sender.0.id, "Sender").await.unwrap();
    let recipient = new_session(client, &workspace, recipient_args, BTreeMap::new()).await;
    (sender, recipient)
}

fn from_sender(sender: &SessionInfo) -> Sender {
    Sender {
        session_id: sender.id.clone(),
        title: Some("Sender".into()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_to_an_idle_session_starts_a_turn_with_its_sender() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let ((sender, mut sender_events), (recipient, mut events)) =
        pair(&daemon, &client, BTreeMap::new(), &[]).await;
    let answer = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "send_message",
        json!({"to": recipient.id, "text": r#"text "pong""#}),
    )
    .await;
    assert_eq!(
        answer,
        format!("Sent. Session {} started a turn with it.", recipient.id)
    );
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(
        turn[0].kind,
        EventKind::PromptStarted {
            prompt: text(r#"text "pong""#),
            from: Some(from_sender(&sender)),
        }
    );
    let header = format!(
        "Message from Ox session {} (\"Sender\"). Your reply in this conversation does not \
         reach it; answer with the ox tool send_message.\n\ntext \"pong\"",
        sender.id
    );
    assert_eq!(echoed(&turn), header);
    assert_eq!(chunks(&turn), ["pong"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_to_a_busy_session_steers_its_turn() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let ((sender, mut sender_events), (recipient, mut events)) =
        pair(&daemon, &client, BTreeMap::new(), &[]).await;
    client.prompt(&recipient.id, text("wait")).await.unwrap();
    let answer = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "send_message",
        json!({"to": recipient.id, "text": "are you done?"}),
    )
    .await;
    assert_eq!(
        answer,
        format!(
            "Sent. Session {} takes it into its current turn.",
            recipient.id
        )
    );
    let steered = until(&mut events, |kind| {
        matches!(kind, EventKind::PromptSteered { .. })
    })
    .await;
    assert_eq!(
        steered.last().unwrap().kind,
        EventKind::PromptSteered {
            prompt: text("are you done?"),
            from: Some(from_sender(&sender)),
        }
    );
    client.cancel(&recipient.id).await.unwrap();
    until(&mut events, turn_ended).await;
}

/// A parked recipient is loaded with the sender's environment.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_wakes_a_parked_session_with_the_senders_environment() {
    let daemon = Daemon::start_with(Options {
        park_after_ms: Some(0),
        ..Options::default()
    })
    .await;
    let client = daemon.client().await;
    let env = BTreeMap::from([("OX_PEER".to_owned(), "from-sender".to_owned())]);
    // The sender's subscription keeps it from parking. The recipient's
    // agent reports that no work runs, so it parks once unviewed.
    let ((sender, mut sender_events), (recipient, mut events)) =
        pair(&daemon, &client, env, &["--background"]).await;
    // The fake agent resumes a session that has had a prompt.
    client.prompt(&recipient.id, text("say 1 0")).await.unwrap();
    until(&mut events, turn_ended).await;
    drop(events);
    until_state(&client, &recipient.id, |state| {
        state == SessionState::Exited
    })
    .await;

    let answer = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "send_message",
        json!({"to": recipient.id, "text": r#"shell "printf %s \"$OX_PEER\"""#}),
    )
    .await;
    assert_eq!(
        answer,
        format!("Sent. Session {} started a turn with it.", recipient.id)
    );
    let mut events = client.subscribe(&recipient.id, 0).await.unwrap();
    let woken = until(&mut events, |kind| {
        matches!(kind, EventKind::PromptStarted { from: Some(_), .. })
    })
    .await;
    assert!(
        matches!(&woken.last().unwrap().kind, EventKind::PromptStarted { from, .. } if *from == Some(from_sender(&sender))),
        "{woken:?}"
    );
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn).concat(), "from-sender");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_loads_a_session_restored_after_a_restart() {
    let daemon = Daemon::start().await;
    // The fake agent resumes a session that has had a prompt.
    let (sender, sender_seq, recipient) = {
        let client = daemon.client().await;
        let ((sender, mut sender_events), (recipient, mut events)) =
            pair(&daemon, &client, BTreeMap::new(), &[]).await;
        client.prompt(&sender.id, text("say 1 0")).await.unwrap();
        let sender_seq = until(&mut sender_events, turn_ended)
            .await
            .pop()
            .unwrap()
            .seq;
        client.prompt(&recipient.id, text("say 1 0")).await.unwrap();
        until(&mut events, turn_ended).await;
        (sender, sender_seq, recipient)
    };
    let daemon = daemon.restart().await;
    let client = daemon.client().await;
    load(&client, &sender.id).await;
    let mut sender_events = client.subscribe(&sender.id, sender_seq).await.unwrap();
    until(&mut sender_events, |kind| {
        matches!(kind, EventKind::AgentStarted { .. })
    })
    .await;

    let answer = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "send_message",
        json!({"to": recipient.id, "text": r#"text "back""#}),
    )
    .await;
    assert_eq!(
        answer,
        format!("Sent. Session {} started a turn with it.", recipient.id)
    );
    let mut events = client.subscribe(&recipient.id, 0).await.unwrap();
    until(&mut events, |kind| {
        matches!(kind, EventKind::PromptStarted { from: Some(_), .. })
    })
    .await;
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["back"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_to_sessions_that_cannot_take_them_are_refused() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let ((sender, mut sender_events), (archived, _)) =
        pair(&daemon, &client, BTreeMap::new(), &[]).await;
    client
        .archive(&archived.id, false, BTreeMap::new())
        .await
        .unwrap();
    let workspace = workspace_id(&client, &daemon.cwd()).await;
    let unstarted = client
        .new_session(
            &workspace,
            vec![FAKE_AGENT.to_owned(), "--no-sessions".to_owned()],
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    until_state(&client, &unstarted.id, |state| {
        state == SessionState::Exited
    })
    .await;

    for (to, refusal) in [
        (
            archived.id.clone(),
            format!("session {} is archived", archived.id),
        ),
        (
            unstarted.id.clone(),
            "the session never started, so it has nothing to load".to_owned(),
        ),
        (
            sender.id.clone(),
            "a session cannot message itself".to_owned(),
        ),
        ("nobody".to_owned(), "no session nobody".to_owned()),
    ] {
        let answer = tool(
            &client,
            &sender.id,
            &mut sender_events,
            "send_message",
            json!({"to": to, "text": r#"text "hi""#}),
        )
        .await;
        assert_eq!(answer, refusal);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn read_transcript_pages_through_any_sessions_transcript() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let ((sender, mut sender_events), (recipient, mut events)) =
        pair(&daemon, &client, BTreeMap::new(), &[]).await;
    client
        .prompt(&recipient.id, text(r#"text "hi""#))
        .await
        .unwrap();
    until(&mut events, turn_ended).await;
    let read = |before: Option<u64>| {
        let mut arguments = json!({"session": recipient.id});
        if let Some(before) = before {
            arguments["before"] = json!(before);
        }
        arguments
    };
    let answer = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "read_transcript",
        read(None),
    )
    .await;
    assert_eq!(
        answer,
        "User: text \"hi\"\n\nAssistant: hi\n\n(turn finished)"
    );

    // Two turns of 30,000 characters each fill more than a page.
    let long = format!("stream 30 0 \"{}\"", "x".repeat(1000));
    for _ in 0..2 {
        client.prompt(&recipient.id, text(&long)).await.unwrap();
        until(&mut events, turn_ended).await;
    }
    client
        .archive(&recipient.id, false, BTreeMap::new())
        .await
        .unwrap();
    let latest = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "read_transcript",
        read(None),
    )
    .await;
    let rest = latest
        .strip_prefix("Earlier entries remain: call read_transcript with before ")
        .unwrap_or_else(|| panic!("{}", &latest[..100]));
    let (before, page) = rest.split_once(" to read them.\n\n").unwrap();
    assert!(
        page.starts_with("(turn finished)\n\nUser: stream"),
        "{}",
        &page[..100]
    );
    assert!(page.ends_with("x\n\n(turn finished)"));

    let earlier = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "read_transcript",
        read(Some(before.parse().unwrap())),
    )
    .await;
    assert!(
        earlier.starts_with("User: text \"hi\""),
        "{}",
        &earlier[..100]
    );
    assert!(earlier.ends_with('x'));
}

#[tokio::test(flavor = "multi_thread")]
async fn list_sessions_lists_the_sessions_one_can_message() {
    let daemon = Daemon::start().await;
    let client = daemon.client().await;
    let ((sender, mut sender_events), (recipient, _)) =
        pair(&daemon, &client, BTreeMap::new(), &[]).await;
    let workspace = workspace_id(&client, &daemon.cwd()).await;
    let (archived, _) = new_session(&client, &workspace, &[], BTreeMap::new()).await;
    client
        .archive(&archived.id, false, BTreeMap::new())
        .await
        .unwrap();
    let unstarted = client
        .new_session(
            &workspace,
            vec![FAKE_AGENT.to_owned(), "--no-sessions".to_owned()],
            BTreeMap::new(),
            Checkout::Workspace,
            Vec::new(),
            None,
        )
        .await
        .unwrap();
    until_state(&client, &unstarted.id, |state| {
        state == SessionState::Exited
    })
    .await;

    let listed = tool(
        &client,
        &sender.id,
        &mut sender_events,
        "list_sessions",
        json!({}),
    )
    .await;
    assert_eq!(
        listed,
        format!(
            "Workspace {}\n- id: {}, state: idle, model: smart, effort: medium, directory: {}\n",
            daemon.cwd().display(),
            recipient.id,
            daemon.cwd().display()
        )
    );
}

/// A repository on `main` that ignores the fake agent's logs, with a second
/// local branch `topic`.
fn repo() -> TempDir {
    let repo = repo_with(".gitignore", "fake-agent-*.jsonl\n");
    git(repo.path(), &["branch", "topic"]);
    repo
}

/// Ox's config for the session request tests: `claude` offers `fast`, at
/// Low, Medium and High, and `smart`, which has no effort levels, in modes
/// Ask and Code; `pi` offers `fast`, with no effort levels and no modes.
const CONFIG: &str = r#"{
  "harnesses": {
    "claude": {
      "models": [
        {
          "id": "fast",
          "name": "Fast",
          "efforts": [
            { "id": "low", "name": "Low" },
            { "id": "medium", "name": "Medium" },
            { "id": "high", "name": "High" }
          ]
        },
        { "id": "smart", "name": "Smart" }
      ],
      "modes": [{ "id": "ask", "name": "Ask" }, { "id": "code", "name": "Code" }]
    },
    "pi": {
      "models": [{ "id": "fast", "name": "Fast" }]
    }
  }
}"#;

/// A daemon that finds the fake agent as each harness's bridge, with
/// [`CONFIG`] as Ox's config, a requester in a repository's workspace, and
/// its subscription.
async fn requester(repo: &Path) -> (Daemon, Client, SessionInfo, Subscription) {
    let daemon = Daemon::start_with(Options {
        bridges: true,
        ..Options::default()
    })
    .await;
    let config = daemon.dir.path().join("ox");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.json"), CONFIG).unwrap();
    let client = daemon.client().await;
    let workspace = workspace_id(&client, repo).await;
    let (session, events) = new_session(&client, &workspace, &[], BTreeMap::new()).await;
    (daemon, client, session, events)
}

/// Prompts the requester to call `start_session` with `arguments` and
/// returns its permission request's ID, title and requested session.
async fn request(
    client: &Client,
    session: &str,
    events: &mut Subscription,
    arguments: &serde_json::Value,
) -> (String, String, SessionRequest) {
    client
        .prompt(session, call("start_session", arguments))
        .await
        .unwrap();
    let requested = until(events, |kind| {
        matches!(kind, EventKind::PermissionRequested { .. })
    })
    .await;
    let EventKind::PermissionRequested {
        request_id,
        title,
        description,
        session_request,
        ..
    } = requested.last().unwrap().kind.clone()
    else {
        unreachable!("until stopped at the request");
    };
    assert_eq!(description, None);
    (request_id, title, session_request.unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_approved_request_starts_the_session_it_describes() {
    let repo = repo();
    let (_daemon, client, requester, mut events) = requester(repo.path()).await;
    let (request_id, title, asked) = request(
        &client,
        &requester.id,
        &mut events,
        &json!({
            "prompt": r#"text "started""#,
            "worktree": true,
            "branch": "topic",
            "harness": "claude",
            "model": "fast",
            "effort": "low",
            "mode": "code",
        }),
    )
    .await;
    let name = repo.path().file_name().unwrap().to_string_lossy();
    assert_eq!(title, format!("Start a session in {name}?"));
    assert_eq!(
        asked,
        SessionRequest {
            harness: Harness::Claude,
            model: "Fast".into(),
            provider: None,
            effort: Some("Low".into()),
            mode: Some("Code".into()),
            checkout: RequestedCheckout::Worktree {
                branch: Some("topic".into()),
                base: "main".into(),
            },
            prompt: r#"text "started""#.into(),
        }
    );
    client
        .permit(&requester.id, &request_id, "start")
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    let answer = chunks(&turn).concat();
    let id = answer
        .strip_prefix("The user started session ")
        .and_then(|rest| rest.strip_suffix('.'))
        .unwrap_or_else(|| panic!("{answer}"));

    let turn = until_turn_ended(&client, id, 0).await;
    assert!(
        turn.iter().any(|event| event.kind
            == EventKind::PromptStarted {
                prompt: text(r#"text "started""#),
                from: None,
            }),
        "{turn:?}"
    );
    assert_eq!(chunks(&turn), ["started"]);
    let listing = listing(&client).await;
    let session = listing.sessions.iter().find(|s| s.id == id).unwrap();
    assert_eq!(session.title.as_deref(), Some(r#"text "started""#));
    let setting = |config_id: &str, value: &str| ConfigSetting {
        config_id: config_id.into(),
        value: ConfigValue::Id(value.into()),
    };
    assert_eq!(
        session.config,
        [
            setting("model", "fast"),
            setting("effort", "low"),
            setting("mode", "code"),
        ]
    );
    let worktree = listing
        .worktrees
        .iter()
        .find(|worktree| Some(&worktree.id) == session.worktree_id.as_ref())
        .unwrap();
    assert_eq!(worktree.branch, "topic");
    assert!(!worktree.created_branch);
    assert_eq!(worktree.compare.as_deref(), Some("main"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_in_the_workspaces_checkout_names_its_directory() {
    let repo = repo();
    let (_daemon, client, requester, mut events) = requester(repo.path()).await;
    let (_, _, asked) = request(
        &client,
        &requester.id,
        &mut events,
        &json!({"prompt": "hello", "worktree": false, "harness": "pi", "model": "fast"}),
    )
    .await;
    assert_eq!(
        asked.checkout,
        RequestedCheckout::Workspace {
            path: repo.path().to_owned()
        }
    );
    assert_eq!((asked.effort, asked.mode), (None, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_declined_request_creates_nothing() {
    let repo = repo();
    let (daemon, client, requester, mut events) = requester(repo.path()).await;
    let (request_id, _, _) = request(
        &client,
        &requester.id,
        &mut events,
        &json!({"prompt": "hello", "worktree": true, "harness": "pi", "model": "fast"}),
    )
    .await;
    client
        .permit(&requester.id, &request_id, "decline")
        .await
        .unwrap();
    let turn = until(&mut events, turn_ended).await;
    assert_eq!(chunks(&turn), ["The user did not start the session."]);
    let listing = listing(&client).await;
    assert_eq!(listing.sessions.len(), 1);
    assert!(listing.worktrees.is_empty());
    assert!(!daemon.worktrees().exists());
    assert_eq!(git(repo.path(), &["branch", "--list", "ox/*"]), "");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_cannot_be_started_is_refused_without_asking() {
    let repo = repo();
    let (_daemon, client, requester, mut events) = requester(repo.path()).await;
    let claude = |fields: serde_json::Value| {
        let mut arguments = json!({
            "prompt": "hi", "worktree": false, "harness": "claude", "model": "fast",
            "effort": "low", "mode": "ask",
        });
        for (key, value) in fields.as_object().unwrap() {
            if value.is_null() {
                arguments.as_object_mut().unwrap().remove(key);
            } else {
                arguments[key] = value.clone();
            }
        }
        arguments
    };
    for (arguments, refusal) in [
        (
            claude(json!({"worktree": true, "branch": "nope"})),
            "nope is not a local branch",
        ),
        (
            claude(json!({"branch": "topic"})),
            "the workspace's checkout is on main, not topic",
        ),
        (
            claude(json!({"harness": "codex"})),
            "codex is not a harness Ox offers; it offers claude, pi",
        ),
        (
            claude(json!({"model": "huge"})),
            "huge is not a claude model Ox offers; it offers fast, smart",
        ),
        (
            claude(json!({"effort": null})),
            "fast needs an effort; Ox offers low, medium, high",
        ),
        (
            claude(json!({"effort": "max"})),
            "max is not an effort Ox offers for fast; it offers low, medium, high",
        ),
        (
            claude(json!({"model": "smart"})),
            "low is not an effort Ox offers for smart; it has no effort levels",
        ),
        (
            claude(json!({"mode": null})),
            "claude needs a mode; Ox offers ask, code",
        ),
        (
            claude(json!({"mode": "plan"})),
            "plan is not a claude mode Ox offers; it offers ask, code",
        ),
        (
            json!({"prompt": "hi", "worktree": false, "harness": "pi", "model": "fast", "mode": "ask"}),
            "ask is not a pi mode Ox offers; it has no modes",
        ),
    ] {
        client
            .prompt(&requester.id, call("start_session", &arguments))
            .await
            .unwrap();
        let turn = until(&mut events, turn_ended).await;
        assert_eq!(chunks(&turn), [refusal], "{arguments}");
        assert!(
            !turn
                .iter()
                .any(|event| matches!(event.kind, EventKind::PermissionRequested { .. })),
            "{arguments}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn list_harnesses_lists_the_config() {
    let repo = repo();
    let (_daemon, client, requester, mut events) = requester(repo.path()).await;
    let listed = tool(
        &client,
        &requester.id,
        &mut events,
        "list_harnesses",
        json!({}),
    )
    .await;
    assert_eq!(
        listed,
        "claude\n\
         modes: ask, code\n\
         - fast (Fast), effort: low, medium, high\n\
         - smart (Smart), no effort levels\n\
         \n\
         pi\n\
         - fast (Fast), no effort levels\n"
    );
}
