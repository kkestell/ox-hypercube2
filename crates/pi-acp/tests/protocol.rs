#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdout, Command},
    time::{Duration, Instant, sleep, timeout},
};

/// What every fake pi does: it records how it was started, and reads
/// commands from stdin, which stays open for the session as the real one's
/// does.
const FAKE_PI: &str = r#"
printf '%s\n' "$*" >> calls
pwd >> calls
# Reads the next command into $command. Pi ends when its stdin closes.
next_command() {
  IFS= read -r command || { : > closed; exit 0; }
}
# Exits unless $command is of type $1.
expect() {
  case "$command" in *"\"type\":\"$1\""*) ;; *) printf 'expected %s, got %s\n' "$1" "$command" >&2; exit 20;; esac
}
# Answers $command, a $1 command, with success and the fields in $2.
answer() {
  id=$(printf '%s\n' "$command" | sed 's/.*"id":"\([^"]*\)".*/\1/')
  printf '{"id":"%s","type":"response","command":"%s","success":true%s}\n' "$id" "$1" "$2"
}
# Reads a prompt and records it in `prompts`.
next_prompt() {
  next_command
  expect prompt
  printf '%s\n' "$command" >> prompts
}
# Accepts the prompt in $command with disposition $1, `started` by default.
accept() {
  answer prompt ",\"data\":{\"disposition\":\"${1:-started}\"}"
}
# Reads an abort and answers it.
aborted() {
  next_command
  expect abort
  answer abort
}
emit() {
  printf '%s\n' "$1"
}
# An assistant message that is not streamed.
assistant() {
  emit '{"type":"message_start","message":{"role":"assistant","content":[]}}'
  emit "{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"content\":$1,\"stopReason\":\"${2:-stop}\"}}"
}
# Ends the run, and with it the turn, aborted when $1 is `true`.
settled() {
  emit '{"type":"agent_end","messages":[],"willRetry":false}'
  emit "{\"type\":\"agent_settled\",\"aborted\":${1:-false}}"
}
"#;

fn fake_pi(dir: &Path, script: &str) {
    let path = dir.join("pi");
    fs::write(
        &path,
        format!("#!/bin/sh\n{FAKE_PI}\n{script}\nwhile :; do next_command; done\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn dirs(temp: &Path) -> (PathBuf, PathBuf) {
    let bin = temp.join("bin");
    let project = temp.join("project");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&project).unwrap();
    (bin, project.canonicalize().unwrap())
}

fn start(bin: &Path) -> (Child, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pi-acp"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let reader = BufReader::new(child.stdout.take().unwrap());
    (child, reader)
}

async fn send(child: &mut Child, message: Value) {
    let stdin = child.stdin.as_mut().unwrap();
    stdin
        .write_all(message.to_string().as_bytes())
        .await
        .unwrap();
    stdin.write_all(b"\n").await.unwrap();
}

async fn receive(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .expect("timed out waiting for ACP response")
        .unwrap();
    assert!(!line.is_empty(), "ACP process closed stdout");
    serde_json::from_str(&line).unwrap()
}

async fn response(reader: &mut BufReader<ChildStdout>, id: i64) -> (Value, Vec<Value>) {
    let mut notifications = Vec::new();
    loop {
        let value = receive(reader).await;
        if value["id"] == id {
            return (value, notifications);
        }
        notifications.push(value);
    }
}

/// The `session/update` update of a notification.
fn update(notification: &Value) -> &Value {
    &notification["params"]["update"]
}

fn kind(notification: &Value) -> &Value {
    &update(notification)["sessionUpdate"]
}

fn is_state(notification: &Value, state: &str) -> bool {
    kind(notification) == "state_update" && update(notification)["state"] == state
}

/// Reads until the prompt `id` is answered and its turn goes idle, with
/// what was `seen` already, and checks that the response names the user
/// message the bridge sent. Returns the idle update, and the notifications
/// from `seen` and after.
async fn finish(
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
            kind(value) == "user_message" && update(value)["messageId"] == *message_id
        }),
        "no user message {message_id} in {seen:?}"
    );
    seen.retain(|value| value["method"] == "session/update");
    (idle, seen)
}

async fn initialize(child: &mut Child, reader: &mut BufReader<ChildStdout>) {
    send(
        child,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":2,"capabilities":{},"info":{"name":"test","version":"1"}
        }}),
    )
    .await;
    let (value, _) = response(reader, 1).await;
    assert_eq!(value["result"]["protocolVersion"], 2);
    assert!(
        value["result"]["capabilities"]["session"]["prompt"]["image"].is_object(),
        "{value}"
    );
}

/// Opens a session, which reports itself idle with nothing running.
async fn new_session(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    request_id: i64,
    project: &Path,
) -> Value {
    send(child, json!({"jsonrpc":"2.0","id":request_id,"method":"session/new","params":{"cwd":project,"mcpServers":[]}})).await;
    let (new, _) = response(reader, request_id).await;
    let session = new["result"]["sessionId"].clone();
    assert!(session.is_string(), "{new}");
    let snapshot = receive(reader).await;
    assert!(is_state(&snapshot, "idle"), "{snapshot}");
    assert_eq!(update(&snapshot)["background"], json!([]), "{snapshot}");
    assert_eq!(snapshot["params"]["sessionId"], session, "{snapshot}");
    session
}

/// Starts the bridge with a fake pi running `script`, and opens a session.
async fn session(
    script: &str,
) -> (
    tempfile::TempDir,
    PathBuf,
    Child,
    BufReader<ChildStdout>,
    Value,
) {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi(&bin, script);
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    (temp, project, child, reader, session)
}

fn prompt_request(id: i64, session: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})
}

fn cancel_notification(session: &Value) -> Value {
    json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}})
}

/// Sends a prompt and reads until its turn ends.
async fn prompt(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    id: i64,
    session: &Value,
) -> (Value, Vec<Value>) {
    send(child, prompt_request(id, session)).await;
    finish(reader, id, Vec::new()).await
}

/// The lines a fake pi recorded in `project`'s `name` file, once it has
/// `count` of them.
async fn lines(project: &Path, name: &str, count: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let lines: Vec<String> = fs::read_to_string(project.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        if lines.len() >= count {
            return lines;
        }
        assert!(Instant::now() < deadline, "{name} has {lines:?}");
        sleep(Duration::from_millis(10)).await;
    }
}

/// The message of the error an idle update ends its turn with.
fn error_message(idle: &Value) -> Option<&str> {
    (idle["stopReason"] == "error").then(|| idle["error"]["message"].as_str().unwrap())
}

/// The updates of tool call `id`.
fn tool_updates<'a>(updates: &'a [Value], id: &str) -> Vec<&'a Value> {
    updates
        .iter()
        .map(update)
        .filter(|update| {
            update["sessionUpdate"] == "tool_call_update" && update["toolCallId"] == id
        })
        .collect()
}

fn statuses(updates: &[&Value]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| update["status"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_new_session_starts_pi_and_reports_idle() {
    let (_temp, project, mut child, _reader, _session) = session("").await;
    let calls = lines(&project, "calls", 2).await;
    let session_id = calls[0]
        .strip_prefix("--mode rpc --approve --session-id ")
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert_eq!(session_id.len(), 36, "{calls:?}");
    assert_eq!(session_id.matches('-').count(), 4, "{calls:?}");
    assert_eq!(calls[1], project.display().to_string());
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_prompt_is_answered_when_pi_accepts_it_and_ends_when_pi_settles() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"message_start","message":{"role":"assistant","content":[]}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":0}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"Let me "}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"think"}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":1}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"Hello "}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"there"}}'
emit '{"type":"message_end","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Let me think"},{"type":"text","text":"Hello there"}],"stopReason":"error","errorMessage":"529 overloaded"}}'
emit '{"type":"agent_end","messages":[],"willRetry":true}'
emit '{"type":"auto_retry_start","attempt":1,"maxAttempts":3,"delayMs":0,"errorMessage":"529 overloaded"}'
emit '{"type":"agent_start"}'
emit '{"type":"message_start","message":{"role":"assistant","content":[]}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Retried"}}'
emit '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"Retried"}],"stopReason":"stop"}}'
settled
"#,
    )
    .await;
    let (idle, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    assert_eq!(idle["background"], json!([]), "{idle}");

    let chunks = |name: &str| -> Vec<(String, String)> {
        updates
            .iter()
            .filter(|value| kind(value) == name)
            .map(|value| {
                (
                    update(value)["messageId"].as_str().unwrap().to_owned(),
                    update(value)["content"]["text"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                )
            })
            .collect()
    };
    let thoughts = chunks("agent_thought_chunk");
    let texts = chunks("agent_message_chunk");
    assert_eq!(
        thoughts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        ["Let me ", "think"]
    );
    assert_eq!(
        texts
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        ["Hello ", "there", "Retried"]
    );
    assert_eq!(thoughts[0].0, thoughts[1].0);
    assert_eq!(texts[0].0, texts[1].0);
    assert_ne!(texts[0].0, thoughts[0].0);
    assert_ne!(texts[2].0, texts[0].0);
    assert!(
        !updates
            .iter()
            .any(|value| kind(value) == "agent_message" || kind(value) == "agent_thought"),
        "streamed blocks are not sent again: {updates:?}"
    );

    let states: Vec<&Value> = updates
        .iter()
        .filter(|value| kind(value) == "state_update")
        .collect();
    assert_eq!(states.len(), 2, "one running and one idle: {states:?}");
    assert!(is_state(states[0], "running"), "{states:?}");
    let retried = updates
        .iter()
        .position(|value| update(value)["content"]["text"] == "Retried")
        .unwrap();
    let idle_at = updates
        .iter()
        .position(|value| is_state(value, "idle"))
        .unwrap();
    assert!(retried < idle_at);

    let prompts = lines(&project, "prompts", 1).await;
    let sent: Value = serde_json::from_str(&prompts[0]).unwrap();
    assert_eq!(sent["message"], "hello");
    assert_eq!(sent["streamingBehavior"], "followUp");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn tool_calls_report_kinds_locations_and_diffs() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
assistant '[{"type":"toolCall","id":"e1","name":"edit","arguments":{"path":"src/a.txt","edits":[{"oldText":"two","newText":"three"}]}},{"type":"toolCall","id":"w1","name":"write","arguments":{"path":"new.txt","content":"one\ntwo\n"}},{"type":"toolCall","id":"b1","name":"bash","arguments":{"command":"false"}}]' toolUse
emit '{"type":"tool_execution_start","toolCallId":"e1","toolName":"edit","args":{}}'
emit '{"type":"tool_execution_end","toolCallId":"e1","toolName":"edit","result":{"content":[{"type":"text","text":"Edited src/a.txt"}],"details":{"patch":"--- src/a.txt\n+++ src/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+three\n"}},"isError":false}'
emit '{"type":"tool_execution_start","toolCallId":"w1","toolName":"write","args":{}}'
emit '{"type":"tool_execution_end","toolCallId":"w1","toolName":"write","result":{"content":[{"type":"text","text":"Wrote new.txt"}]},"isError":false}'
emit '{"type":"tool_execution_start","toolCallId":"b1","toolName":"bash","args":{}}'
emit '{"type":"tool_execution_end","toolCallId":"b1","toolName":"bash","result":{"content":[{"type":"text","text":"exit code 1"}]},"isError":true}'
assistant '[{"type":"text","text":"done"}]'
settled
"#,
    )
    .await;
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(project.join("src/a.txt"), "one\ntwo\n").unwrap();
    let (idle, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    let edited = project.join("src/a.txt").display().to_string();
    let written = project.join("new.txt").display().to_string();

    let edit = tool_updates(&updates, "e1");
    assert_eq!(edit[0]["kind"], "edit", "{edit:?}");
    assert_eq!(edit[0]["title"], "Edit `src/a.txt`", "{edit:?}");
    assert_eq!(edit[0]["locations"], json!([{"path": edited}]), "{edit:?}");
    assert_eq!(statuses(&edit), ["pending", "in_progress", "completed"]);
    let diff = &edit.last().unwrap()["content"][0];
    assert_eq!(diff["type"], "diff", "{diff}");
    assert_eq!(
        diff["changes"],
        json!([{"operation": "modify", "path": edited}])
    );
    assert_eq!(
        diff["patch"]["text"],
        format!(
            "diff --git {edited} {edited}\n--- {edited}\n+++ {edited}\n@@ -1,2 +1,2 @@\n one\n-two\n+three\n"
        )
    );
    assert_eq!(
        edit.last().unwrap()["content"][1]["content"]["text"],
        "Edited src/a.txt"
    );

    let write = tool_updates(&updates, "w1");
    assert_eq!(write[0]["kind"], "edit", "{write:?}");
    let diff = &write.last().unwrap()["content"][0];
    assert_eq!(
        diff["changes"],
        json!([{"operation": "add", "path": written}]),
        "{diff}"
    );
    assert_eq!(
        diff["patch"]["text"],
        format!(
            "diff --git {written} {written}\nnew file mode 100644\n--- /dev/null\n+++ {written}\n@@ -0,0 +1,2 @@\n+one\n+two\n"
        )
    );

    let bash = tool_updates(&updates, "b1");
    assert_eq!(bash[0]["kind"], "execute", "{bash:?}");
    assert_eq!(bash[0]["title"], "Bash `false`", "{bash:?}");
    assert_eq!(statuses(&bash), ["pending", "in_progress", "failed"]);
    assert_eq!(
        bash.last().unwrap()["content"],
        json!([{"type": "content", "content": {"type": "text", "text": "exit code 1"}}])
    );

    assert!(
        updates
            .iter()
            .any(|value| kind(value) == "agent_message"
                && update(value)["content"][0]["text"] == "done"),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_prompt_pi_rejects_fails_with_its_error() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r#"
next_prompt
id=$(printf '%s\n' "$command" | sed 's/.*"id":"\([^"]*\)".*/\1/')
printf '{"id":"%s","type":"response","command":"prompt","success":false,"error":"No model selected"}\n' "$id"
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    let (result, updates) = response(&mut reader, 3).await;
    assert_eq!(result["error"]["code"], -32603, "{result}");
    assert_eq!(result["error"]["message"], "No model selected", "{result}");
    assert!(
        !updates.iter().any(|value| kind(value) == "user_message"),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_failed_response_ends_the_turn_with_its_error() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"message_start","message":{"role":"assistant","content":[]}}'
emit '{"type":"message_end","message":{"role":"assistant","content":[],"stopReason":"error","errorMessage":"rate limited"}}'
settled
"#,
    )
    .await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(error_message(&idle), Some("rate limited"), "{idle}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn cancel_aborts_the_turn() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"message_start","message":{"role":"assistant","content":[]}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"working"}}'
aborted
emit '{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"working"}],"stopReason":"aborted"}}'
settled true
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    let mut seen = Vec::new();
    while update(seen.last().unwrap_or(&Value::Null))["content"]["text"] != "working" {
        seen.push(receive(&mut reader).await);
    }
    send(&mut child, cancel_notification(&session)).await;
    let (idle, _) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "cancelled", "{idle}");
    child.kill().await.unwrap();
}

/// A cancel that reaches Pi before Pi answers the prompt does not stop the
/// run that prompt starts, so the bridge aborts again once Pi starts it.
#[tokio::test]
async fn cancel_sent_right_after_prompt_cancels_it() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r#"
next_prompt
prompt=$command
aborted
command=$prompt
accept
aborted
emit '{"type":"agent_start"}'
settled true
"#,
    )
    .await;
    let stdin = child.stdin.as_mut().unwrap();
    stdin
        .write_all(
            format!(
                "{}\n{}\n",
                prompt_request(3, &session),
                cancel_notification(&session)
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let (idle, _) = finish(&mut reader, 3, Vec::new()).await;
    assert_eq!(idle["stopReason"], "cancelled", "{idle}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_pi_that_exits_fails_the_turn_and_the_next_prompt_starts_it_again() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
if [ "$(wc -l < calls)" -le 2 ]; then
  next_prompt
  accept
  emit '{"type":"agent_start"}'
  echo 'model crashed' >&2
  exit 3
fi
next_prompt
accept
emit '{"type":"agent_start"}'
settled
"#,
    )
    .await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    let error = error_message(&idle).unwrap_or_else(|| panic!("{idle}"));
    assert!(error.contains("model crashed"), "{error}");
    let (idle, _) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    let calls = lines(&project, "calls", 4).await;
    assert_eq!(calls[0], calls[2], "the same Pi session: {calls:?}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn closing_a_session_stops_pi() {
    let (_temp, project, mut child, mut reader, session) = session("").await;
    lines(&project, "calls", 2).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":3,"method":"session/close","params":{"sessionId":session}}),
    )
    .await;
    let (closed, _) = response(&mut reader, 3).await;
    assert!(closed["result"].is_object(), "{closed}");
    assert!(project.join("closed").exists(), "pi saw its stdin close");
    send(&mut child, prompt_request(4, &session)).await;
    let (result, _) = response(&mut reader, 4).await;
    assert_eq!(result["error"]["code"], -32002, "{result}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn extension_dialogs_are_dismissed() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"extension_ui_request","id":"note-1","method":"notify","message":"Checking"}'
emit '{"type":"extension_ui_request","id":"ask-1","method":"confirm","title":"Run it?","message":"rm -rf build"}'
next_command
printf '%s\n' "$command" > dialog
settled
"#,
    )
    .await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    let dialog: Value = serde_json::from_str(&lines(&project, "dialog", 1).await[0]).unwrap();
    assert_eq!(
        dialog,
        json!({"type": "extension_ui_response", "id": "ask-1", "cancelled": true})
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_handled_prompt_ends_its_turn() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r"
next_prompt
accept handled
",
    )
    .await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    child.kill().await.unwrap();
}
