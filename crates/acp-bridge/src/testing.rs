//! Helpers for tests that drive a bridge binary over its stdio.

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdout},
    time::{Duration, timeout},
};

pub async fn send(child: &mut Child, message: Value) {
    let stdin = child.stdin.as_mut().unwrap();
    stdin
        .write_all(message.to_string().as_bytes())
        .await
        .unwrap();
    stdin.write_all(b"\n").await.unwrap();
}

pub async fn receive(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("timed out waiting for ACP response")
        .unwrap();
    assert!(!line.is_empty(), "ACP process closed stdout");
    serde_json::from_str(&line).unwrap()
}

/// Reads until the response to request `id`. Returns it and what was read
/// before it.
pub async fn response(reader: &mut BufReader<ChildStdout>, id: i64) -> (Value, Vec<Value>) {
    let mut notifications = Vec::new();
    loop {
        let value = receive(reader).await;
        if value["id"] == id {
            return (value, notifications);
        }
        notifications.push(value);
    }
}

/// Reads until the client gets a request or notification of `method`.
/// Returns it and what was read before it.
pub async fn until_method(
    reader: &mut BufReader<ChildStdout>,
    method: &str,
) -> (Value, Vec<Value>) {
    let mut seen = Vec::new();
    loop {
        let value = receive(reader).await;
        if value["method"] == method {
            return (value, seen);
        }
        seen.push(value);
    }
}

/// The `session/update` update of a notification.
pub fn update(notification: &Value) -> &Value {
    &notification["params"]["update"]
}

pub fn is_state(notification: &Value, state: &str) -> bool {
    notification["method"] == "session/update"
        && update(notification)["sessionUpdate"] == "state_update"
        && update(notification)["state"] == state
}

/// Reads until the prompt `id` is answered and its turn goes idle, with
/// what was `seen` already, and checks that the response names the user
/// message the bridge sent. Returns the idle update, and the notifications
/// from `seen` and after.
pub async fn finish(
    reader: &mut BufReader<ChildStdout>,
    id: i64,
    mut seen: Vec<Value>,
) -> (Value, Vec<Value>) {
    let idle = loop {
        let answered = seen.iter().position(|value| value["id"] == id);
        if let Some(answered) = answered
            && let Some(idle) = seen[answered..].iter().find(|v| is_state(v, "idle"))
        {
            break update(idle).clone();
        }
        seen.push(receive(reader).await);
    };
    let response = seen.iter().find(|value| value["id"] == id).unwrap();
    let message_id = &response["result"]["messageId"];
    assert!(message_id.is_string(), "{response}");
    assert!(
        seen.iter().any(|value| {
            update(value)["sessionUpdate"] == "user_message"
                && update(value)["messageId"] == *message_id
        }),
        "no user message {message_id} in {seen:?}"
    );
    seen.retain(|value| value["method"] == "session/update");
    (idle, seen)
}
