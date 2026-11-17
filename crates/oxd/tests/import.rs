//! Importing Claude Code sessions: the daemon runs the fake agent as the
//! Claude bridge, whose `--sessions` and `--transcript` read
//! `fake-transcripts.json` in the workspace directory.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::Path;

use ox_protocol::schema::{ContentBlock, SessionUpdate};
use ox_protocol::{
    Client, ConfigSetting, ConfigValue, Event, EventKind, SessionInfo, SessionState,
};
use serde_json::{Value, json};

mod common;

use common::*;

/// A session of the fixture: started at `started_at`, with a prompt at
/// `started_at + 1` and a reply at `started_at + 2`.
fn claude_session(id: &str, started_at: u64) -> Value {
    json!({
        "sessionId": id,
        "startedAt": started_at,
        "title": format!("Session {id}"),
        "updates": [
            {"at": started_at + 1, "update": {"sessionUpdate": "user_message", "messageId": "m1",
                "content": [{"type": "text", "text": format!("Prompt {id}")}]}},
            {"at": started_at + 2, "update": {"sessionUpdate": "agent_message", "messageId": "m2",
                "content": [{"type": "text", "text": format!("Reply {id}")}]}},
        ],
    })
}

fn write_fixture(workspace: &Path, sessions: &[Value]) {
    std::fs::write(
        workspace.join("fake-transcripts.json"),
        Value::from(sessions.to_vec()).to_string(),
    )
    .unwrap();
}

/// A daemon that runs the fake agent as the Claude bridge, logging, and a
/// workspace in its directory.
async fn daemon() -> (Daemon, Client, String) {
    let daemon = Daemon::start_with(Options {
        bridges: true,
        log: true,
        ..Options::default()
    })
    .await;
    let client = daemon.client().await;
    let workspace = workspace_id(&client, &daemon.cwd()).await;
    (daemon, client, workspace)
}

async fn import(client: &Client, workspace: &str) -> u64 {
    client
        .import_claude_sessions(workspace, BTreeMap::new())
        .await
        .unwrap()
        .imported
}

/// The listed session of Claude session `id`.
async fn imported(client: &Client, id: &str) -> Option<SessionInfo> {
    sessions(client)
        .await
        .into_iter()
        .find(|session| session.agent_session_id.as_deref() == Some(id))
}

/// The session's first `count` stored events.
async fn history(client: &Client, session_id: &str, count: usize) -> Vec<Event> {
    let mut events = client.subscribe(session_id, 0).await.unwrap();
    let mut history = Vec::new();
    while history.len() < count {
        history.push(next(&mut events).await);
    }
    history
}

/// Each event's seq, time and what it shows.
fn described(history: &[Event]) -> Vec<(u64, u64, String)> {
    history
        .iter()
        .map(|event| {
            let text = |content: &[ContentBlock]| match content {
                [ContentBlock::Text(text)] => text.text.clone(),
                _ => panic!("unexpected content {content:?}"),
            };
            let kind = match &event.kind {
                EventKind::PromptStarted { prompt, .. } => format!("prompt: {}", text(prompt)),
                EventKind::Update(SessionUpdate::AgentMessage(message)) => {
                    format!("agent: {}", text(message.content.value().unwrap()))
                }
                EventKind::TurnEnded { .. } => "turn ended".into(),
                kind => panic!("unexpected event {kind:?}"),
            };
            (event.seq, event.recorded_at, kind)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn importing_adds_each_session_archived_with_its_history() {
    let (daemon, client, workspace) = daemon().await;
    write_fixture(
        &daemon.cwd(),
        &[claude_session("b", 2000), claude_session("a", 1000)],
    );
    assert_eq!(import(&client, &workspace).await, 2);

    let session = imported(&client, "a").await.unwrap();
    assert!(session.archived);
    assert_eq!(session.state, SessionState::Exited);
    assert_eq!(session.title.as_deref(), Some("Session a"));
    assert_eq!(session.created_at, 1000);
    assert_eq!(session.workspace_id, workspace);
    assert_eq!(session.cwd, daemon.cwd());
    assert_eq!(
        Path::new(&session.command[0]).file_name().unwrap(),
        "claude-code-acp"
    );
    assert_eq!(
        described(&history(&client, &session.id, 3).await),
        [
            (1, 1001, "prompt: Prompt a".to_owned()),
            (2, 1002, "agent: Reply a".to_owned()),
            (3, 1002, "turn ended".to_owned()),
        ]
    );
    assert!(imported(&client, "b").await.unwrap().archived);
}

#[tokio::test(flavor = "multi_thread")]
async fn importing_again_adds_only_new_sessions() {
    let (daemon, client, workspace) = daemon().await;
    write_fixture(&daemon.cwd(), &[claude_session("a", 1000)]);
    assert_eq!(import(&client, &workspace).await, 1);
    assert_eq!(import(&client, &workspace).await, 0);

    write_fixture(
        &daemon.cwd(),
        &[claude_session("b", 2000), claude_session("a", 1000)],
    );
    assert_eq!(import(&client, &workspace).await, 1);
    let ids: Vec<_> = sessions(&client)
        .await
        .into_iter()
        .filter_map(|session| session.agent_session_id)
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids.contains(&"a".to_owned()) && ids.contains(&"b".to_owned()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_imported_session_is_not_imported_again() {
    let (daemon, client, workspace) = daemon().await;
    write_fixture(&daemon.cwd(), &[claude_session("a", 1000)]);
    assert_eq!(import(&client, &workspace).await, 1);
    let session = imported(&client, "a").await.unwrap();
    client
        .delete(&session.id, false, BTreeMap::new())
        .await
        .unwrap();

    assert_eq!(import(&client, &workspace).await, 0);
    assert!(imported(&client, "a").await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transcript_that_fails_is_skipped() {
    let (mut daemon, client, workspace) = daemon().await;
    let mut failing = claude_session("bad", 3000);
    failing["fail"] = json!(true);
    write_fixture(
        &daemon.cwd(),
        &[
            failing,
            claude_session("b", 2000),
            claude_session("a", 1000),
        ],
    );
    assert_eq!(import(&client, &workspace).await, 2);
    daemon
        .wait_for_log("importing Claude Code session bad: transcript bad is unreadable")
        .await;
    assert!(imported(&client, "bad").await.is_none());
    assert!(imported(&client, "a").await.is_some());
    assert!(imported(&client, "b").await.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unarchived_imported_session_resumes_without_replay() {
    let (daemon, client, workspace) = daemon().await;
    write_fixture(&daemon.cwd(), &[claude_session("a", 1000)]);
    // The fake agent replays its log when it resumes with replay.
    std::fs::write(
        daemon.cwd().join("fake-agent-a.jsonl"),
        json!({"role": "user", "text": "replayed"}).to_string() + "\n",
    )
    .unwrap();
    assert_eq!(import(&client, &workspace).await, 1);
    let session = imported(&client, "a").await.unwrap();
    assert!(session.config.is_empty());
    client
        .unarchive(&session.id, BTreeMap::new())
        .await
        .unwrap();
    let mut events = client.subscribe(&session.id, 3).await.unwrap();
    client
        .load_session(&session.id, BTreeMap::new())
        .await
        .unwrap();
    // A replay precedes the agent's start.
    let resumed = until(&mut events, |kind| {
        matches!(
            kind,
            EventKind::AgentStarted { .. } | EventKind::AgentExited { .. }
        )
    })
    .await;
    assert!(
        matches!(resumed.last().unwrap().kind, EventKind::AgentStarted { .. }),
        "{resumed:?}"
    );
    assert!(
        !resumed
            .iter()
            .any(|event| matches!(event.kind, EventKind::Update(SessionUpdate::UserMessage(_)))),
        "{resumed:?}"
    );
    // A session without a config takes the one its agent reports.
    let setting = |config_id: &str, value: &str| ConfigSetting {
        config_id: config_id.into(),
        value: ConfigValue::Id(value.into()),
    };
    assert_eq!(
        imported(&client, "a").await.unwrap().config,
        [
            setting("model", "smart"),
            setting("effort", "medium"),
            setting("mode", "ask"),
        ]
    );
    assert_eq!(
        described(&history(&client, &session.id, 3).await),
        [
            (1, 1001, "prompt: Prompt a".to_owned()),
            (2, 1002, "agent: Reply a".to_owned()),
            (3, 1002, "turn ended".to_owned()),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_workspace_of_archived_sessions_lets_them_be_imported_again() {
    let (daemon, client, workspace) = daemon().await;
    write_fixture(&daemon.cwd(), &[claude_session("a", 1000)]);
    assert_eq!(import(&client, &workspace).await, 1);

    client.remove_workspace(&workspace).await.unwrap();
    assert!(workspaces(&client).await.is_empty());
    assert!(sessions(&client).await.is_empty());

    let workspace = workspace_id(&client, &daemon.cwd()).await;
    assert_eq!(import(&client, &workspace).await, 1);
    assert!(imported(&client, "a").await.unwrap().archived);
}
