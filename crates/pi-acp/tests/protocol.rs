#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
};

use acp_bridge::testing::{
    TIMEOUT, finish, is_state, receive, response, send, until_method, update,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdout, Command},
};

/// What every fake pi does: it records how it was started and the MCP
/// servers it was given, and reads commands from stdin, which stays open for
/// the session as the real one's does.
const FAKE_PI: &str = r#"
printf '%s\n' "$*" >> calls
pwd >> calls
if [ -n "${PI_ACP_MCP_SERVERS-}" ]; then printf '%s\n' "$PI_ACP_MCP_SERVERS" >> servers; fi
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
# Reads all four description queries before answering them out of order.
describe() {
  for query in 1 2 3 4; do
    next_command
    printf '%s\n' "$command" >> queries
    case "$command" in
      *'"type":"get_state"'*) state_command=$command;;
      *'"type":"get_available_models"'*) models_command=$command;;
      *'"type":"get_available_thinking_levels"'*) levels_command=$command;;
      *'"type":"get_commands"'*) commands_command=$command;;
      *) printf 'unexpected query: %s\n' "$command" >&2; exit 21;;
    esac
  done
  command=$commands_command
  answer get_commands ',"data":{"commands":[{"name":"template","description":"A template","source":"prompt"},{"name":"extension","source":"extension"},{"name":"skill:review","description":"Review","source":"skill"}]}'
  command=$levels_command
  answer get_available_thinking_levels ',"data":{"levels":["off","low","high"]}'
  command=$models_command
  answer get_available_models ',"data":{"models":[{"provider":"first","id":"one","name":"One"},{"provider":"second","id":"org/two","name":"Two"}]}'
  command=$state_command
  model=${current_model:-'{"provider":"first","id":"one","name":"One"}'}
  answer get_state ",\"data\":{\"sessionFile\":\"$PWD/history.jsonl\",\"model\":$model,\"thinkingLevel\":\"${thinking:-high}\"}"
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
# Reads the clear_queue and the abort a cancel sends, in that order, and
# answers them.
aborted() {
  next_command
  expect clear_queue
  answer clear_queue ',"data":{"steering":[],"followUp":[]}'
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
    fake_pi_script(dir, &format!("describe\n{script}"));
}

/// Writes the fake pi through a rename, so a fake pi of an earlier bridge
/// that is still reading its script keeps the whole old one.
fn fake_pi_script(dir: &Path, script: &str) {
    let staging = dir.join("pi.new");
    fs::write(
        &staging,
        format!("#!/bin/sh\n{FAKE_PI}\n{script}\nwhile :; do next_command; done\n"),
    )
    .unwrap();
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&staging, dir.join("pi")).unwrap();
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

async fn answer_form(child: &mut Child, request: &Value, action: Value) {
    send(
        child,
        json!({"jsonrpc":"2.0", "id":request["id"], "result": action}),
    )
    .await;
}

fn kind(notification: &Value) -> &Value {
    &update(notification)["sessionUpdate"]
}

async fn initialize(child: &mut Child, reader: &mut BufReader<ChildStdout>, capabilities: Value) {
    send(
        child,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":2,"capabilities":capabilities,"info":{"name":"test","version":"1"}
        }}),
    )
    .await;
    let (value, _) = response(reader, 1).await;
    assert_eq!(value["result"]["protocolVersion"], 2);
    assert!(
        value["result"]["capabilities"]["session"]["prompt"]["image"].is_object(),
        "{value}"
    );
    assert!(
        value["result"]["capabilities"]["session"]["mcp"]["stdio"].is_object(),
        "{value}"
    );
}

/// Opens a session, which reports itself idle with nothing running, and
/// returns the `session/new` result.
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
    new["result"].clone()
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
    initialize(&mut child, &mut reader, json!({"elicitation":{"form":{}}})).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await["sessionId"].clone();
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

/// The `count` lines a fake pi recorded in `project`'s `name` file. The fake
/// writes them before output the test has already read.
fn lines(project: &Path, name: &str, count: usize) -> Vec<String> {
    let lines: Vec<String> = fs::read_to_string(project.join(name))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), count, "{name} has {lines:?}");
    lines
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
async fn a_new_session_starts_pi_and_answers_with_its_options_and_commands() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi(&bin, "");
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader, json!({"elicitation":{"form":{}}})).await;
    let opened = new_session(&mut child, &mut reader, 2, &project).await;
    assert_eq!(opened["configOptions"][0]["category"], "model", "{opened}");
    assert_eq!(opened["configOptions"][0]["currentValue"], "first/one");
    assert_eq!(opened["configOptions"][1]["category"], "thought_level");
    assert_eq!(opened["configOptions"][1]["currentValue"], "high");
    assert_eq!(
        opened["availableCommands"],
        json!([
            {"type":"command","name":"template","description":"A template"},
            {"type":"command","name":"extension","description":""},
            {"type":"command","name":"skill:review","description":"Review"}
        ])
    );
    let calls = lines(&project, "calls", 2);
    let session_id = calls[0]
        .strip_prefix("--mode rpc --approve --session-id ")
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert_eq!(session_id.len(), 36, "{calls:?}");
    assert_eq!(session_id.matches('-').count(), 4, "{calls:?}");
    assert_eq!(calls[1], project.display().to_string());
    child.kill().await.unwrap();
}

/// Runs the bridge's `--describe` in `project`, with the pi in `bin`.
async fn describe(bin: &Path, project: &Path) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_pi-acp"))
        .arg("--describe")
        .arg(project)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(TIMEOUT, output)
        .await
        .expect("timed out waiting for --describe")
        .unwrap()
}

#[tokio::test]
async fn describe_prints_the_options_and_commands_pi_reports() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi(&bin, "");
    let output = describe(&bin, &project).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let description: Value = serde_json::from_slice(&output.stdout).unwrap();
    let options = &description["configOptions"];
    assert_eq!(options[0]["category"], "model", "{description}");
    assert_eq!(options[0]["currentValue"], "first/one", "{description}");
    assert_eq!(options[1]["category"], "thought_level", "{description}");
    assert_eq!(options[1]["currentValue"], "high", "{description}");
    assert_eq!(
        description["availableCommands"],
        json!([
            {"type":"command","name":"template","description":"A template"},
            {"type":"command","name":"extension","description":""},
            {"type":"command","name":"skill:review","description":"Review"}
        ])
    );
    let calls = lines(&project, "calls", 2);
    assert_eq!(calls[0], "--mode rpc --approve --no-session");
    assert_eq!(calls[1], project.display().to_string());
}

#[tokio::test]
async fn describe_fails_with_the_error_of_a_pi_that_does_not_start() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi_script(&bin, "echo 'no models configured' >&2\nexit 3");
    let output = describe(&bin, &project).await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("no models configured"), "{stderr}");
    assert!(output.stdout.is_empty());
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

    let prompts = lines(&project, "prompts", 1);
    let sent: Value = serde_json::from_str(&prompts[0]).unwrap();
    assert_eq!(sent["message"], "hello");
    assert_eq!(sent["streamingBehavior"], "steer");
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
next_command
expect set_thinking_level
answer set_thinking_level
describe
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    let (result, mut updates) = response(&mut reader, 3).await;
    assert_eq!(result["error"]["code"], -32603, "{result}");
    assert_eq!(result["error"]["message"], "No model selected", "{result}");
    // The bridge handles Pi's answers in order, so what it sends for the
    // prompt's answer precedes the answer to a later config change.
    send(&mut child, config_request(4, &session, "thinking", "high")).await;
    let (set, more) = response(&mut reader, 4).await;
    assert!(set["result"].is_object(), "{set}");
    updates.extend(more);
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

fn steer_request(id: i64, session: &Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"steer"}]}})
}

/// Starts prompt 3, then sends prompt 4 once Pi works on it. Returns what
/// was read until the update the fake emits after it queues prompt 4, which
/// does not answer prompt 4.
async fn steer(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    session: &Value,
) -> Vec<Value> {
    send(child, prompt_request(3, session)).await;
    let mut seen = Vec::new();
    while update(seen.last().unwrap_or(&Value::Null))["content"]["text"] != "working" {
        seen.push(receive(reader).await);
    }
    send(child, steer_request(4, session)).await;
    while update(seen.last().unwrap())["content"]["text"] != "queued" {
        seen.push(receive(reader).await);
    }
    assert!(!seen.iter().any(|value| value["id"] == 4), "{seen:?}");
    seen
}

/// The fake's run of prompt 3, up to queueing prompt 4.
const STEERED_RUN: &str = r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}'
emit '{"type":"message_end","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}'
emit '{"type":"message_start","message":{"role":"assistant","content":[]}}'
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"working"}}'
next_prompt
accept queued
emit '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"queued"}}'
"#;

#[tokio::test]
async fn a_steering_prompt_is_answered_when_pi_takes_it_in() {
    let (_temp, project, mut child, mut reader, session) = session(&format!(
        r#"{STEERED_RUN}
emit '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"workingqueued"}}],"stopReason":"stop"}}}}'
emit '{{"type":"message_start","message":{{"role":"user","content":[{{"type":"text","text":"steer"}}]}}}}'
emit '{{"type":"message_end","message":{{"role":"user","content":[{{"type":"text","text":"steer"}}]}}}}'
assistant '[{{"type":"text","text":"steered"}}]'
settled
"#
    ))
    .await;
    let mut seen = steer(&mut child, &mut reader, &session).await;
    let (answer, more) = response(&mut reader, 4).await;
    let echo = receive(&mut reader).await;
    assert_eq!(kind(&echo), "user_message", "{echo}");
    assert_eq!(
        update(&echo)["messageId"],
        answer["result"]["messageId"],
        "{echo}"
    );
    assert_eq!(update(&echo)["content"][0]["text"], "steer", "{echo}");
    seen.extend(more);
    seen.extend([answer, echo]);
    let (idle, updates) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    let user_messages = updates
        .iter()
        .filter(|value| kind(value) == "user_message")
        .count();
    assert_eq!(user_messages, 2, "{updates:?}");
    let prompts = lines(&project, "prompts", 2);
    let sent: Value = serde_json::from_str(&prompts[1]).unwrap();
    assert_eq!(sent["message"], "steer");
    assert_eq!(sent["streamingBehavior"], "steer");
    child.kill().await.unwrap();
}

/// Pi starts a run before it answers the prompt that starts it, so it can
/// queue a steer first. The user message of the started prompt still comes
/// first, and does not answer the steer.
#[tokio::test]
async fn a_steer_queued_before_its_run_is_answered_waits_for_its_own_message() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r#"
next_prompt
started=$command
next_prompt
accept queued
command=$started
accept
emit '{"type":"agent_start"}'
emit '{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}'
assistant '[{"type":"text","text":"first"}]'
emit '{"type":"message_start","message":{"role":"user","content":[{"type":"text","text":"steer"}]}}'
assistant '[{"type":"text","text":"second"}]'
settled
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    send(&mut child, steer_request(4, &session)).await;
    let (answer, seen) = response(&mut reader, 4).await;
    assert!(answer["result"]["messageId"].is_string(), "{answer}");
    assert!(
        seen.iter()
            .any(|value| update(value)["content"][0]["text"] == "first"),
        "{seen:?}"
    );
    finish(&mut reader, 3, seen).await;
    child.kill().await.unwrap();
}

/// Pi's abort goes on to deliver the steering queue, so cancel clears it
/// first; the fake's `aborted` expects the two in that order.
#[tokio::test]
async fn cancel_clears_the_steering_queue_and_fails_its_prompts() {
    let (_temp, _project, mut child, mut reader, session) = session(&format!(
        r#"{STEERED_RUN}
aborted
emit '{{"type":"message_end","message":{{"role":"assistant","content":[{{"type":"text","text":"workingqueued"}}],"stopReason":"aborted"}}}}'
settled true
"#
    ))
    .await;
    let mut seen = steer(&mut child, &mut reader, &session).await;
    send(&mut child, cancel_notification(&session)).await;
    let (answer, more) = response(&mut reader, 4).await;
    assert_eq!(answer["error"]["code"], -32800, "{answer}");
    seen.extend(more);
    let (idle, updates) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "cancelled", "{idle}");
    assert!(
        !updates
            .iter()
            .any(|value| update(value)["content"][0]["text"] == "steer"),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_pi_that_exits_fails_its_steering_prompts() {
    let (_temp, _project, mut child, mut reader, session) = session(&format!(
        "{STEERED_RUN}
echo 'model crashed' >&2
exit 3
"
    ))
    .await;
    steer(&mut child, &mut reader, &session).await;
    let (answer, _) = response(&mut reader, 4).await;
    assert_eq!(answer["error"]["code"], -32603, "{answer}");
    let error = answer["error"]["message"].as_str().unwrap();
    assert!(error.contains("model crashed"), "{error}");
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
    let (idle, mut updates) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    while !updates
        .iter()
        .any(|value| kind(value) == "available_commands_update")
    {
        updates.push(receive(&mut reader).await);
    }
    assert!(
        updates
            .iter()
            .any(|value| kind(value) == "config_option_update")
    );
    let calls = lines(&project, "calls", 4);
    assert_eq!(calls[0], calls[2], "the same Pi session: {calls:?}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn extension_dialogs_become_forms() {
    let (_temp, project, mut child, mut reader, session) = session(r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"extension_ui_request","id":"note-1","method":"notify","message":"Checking"}'
emit '{"type":"extension_ui_request","id":"select-1","method":"select","title":"Choose","options":["Allow","Block"]}'
next_command
printf '%s\n' "$command" >> dialogs
emit '{"type":"extension_ui_request","id":"confirm-1","method":"confirm","title":"Run?","message":"Delete build"}'
next_command
printf '%s\n' "$command" >> dialogs
emit '{"type":"extension_ui_request","id":"input-1","method":"input","title":"Name"}'
next_command
printf '%s\n' "$command" >> dialogs
emit '{"type":"extension_ui_request","id":"editor-1","method":"editor","title":"Edit","prefill":"first\nsecond"}'
next_command
printf '%s\n' "$command" >> dialogs
settled
"#).await;
    send(&mut child, prompt_request(3, &session)).await;
    let mut seen = Vec::new();
    for content in [
        json!({"answer":"Block"}),
        json!({"answer":"no"}),
        json!({}),
        json!({"answer":"edited\nsecond"}),
    ] {
        let (request, more) = until_method(&mut reader, "elicitation/create").await;
        seen.extend(more);
        assert_eq!(request["params"]["mode"], "form", "{request}");
        assert_eq!(request["params"]["sessionId"], session, "{request}");
        answer_form(
            &mut child,
            &request,
            json!({"action":"accept", "content":content}),
        )
        .await;
    }
    let (idle, seen) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "end_turn");
    let states: Vec<_> = seen
        .iter()
        .filter(|v| kind(v) == "state_update")
        .map(|v| update(v)["state"].as_str().unwrap())
        .collect();
    assert!(
        states
            .windows(3)
            .any(|s| s == ["running", "requires_action", "running"]),
        "{states:?}"
    );
    let dialogs: Vec<Value> = lines(&project, "dialogs", 4)
        .iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(
        dialogs,
        vec![
            json!({"type":"extension_ui_response","id":"select-1","value":"Block"}),
            json!({"type":"extension_ui_response","id":"confirm-1","confirmed":false}),
            json!({"type":"extension_ui_response","id":"input-1","value":""}),
            json!({"type":"extension_ui_response","id":"editor-1","value":"edited\nsecond"}),
        ]
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn declined_and_cancelled_dialogs_answer_cancelled() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
for id in ask-1 ask-2; do
  emit "{\"type\":\"extension_ui_request\",\"id\":\"$id\",\"method\":\"input\",\"title\":\"Name\"}"
  next_command
  printf '%s\n' "$command" >> dialogs
done
settled
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    let mut seen = Vec::new();
    for action in ["decline", "cancel"] {
        let (request, more) = until_method(&mut reader, "elicitation/create").await;
        seen.extend(more);
        answer_form(&mut child, &request, json!({"action":action})).await;
    }
    finish(&mut reader, 3, seen).await;
    for (index, line) in lines(&project, "dialogs", 2).iter().enumerate() {
        assert_eq!(
            serde_json::from_str::<Value>(line).unwrap(),
            json!({"type":"extension_ui_response","id":format!("ask-{}", index + 1),"cancelled":true})
        );
    }
    child.kill().await.unwrap();
}

#[tokio::test]
async fn cancel_withdraws_open_dialogs() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"extension_ui_request","id":"ask-1","method":"confirm","title":"Run?"}'
next_command
printf '%s\n' "$command" > dialog
aborted
settled true
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    let (request, mut seen) = until_method(&mut reader, "elicitation/create").await;
    send(&mut child, cancel_notification(&session)).await;
    let (cancel, more) = until_method(&mut reader, "$/cancel_request").await;
    seen.extend(more);
    assert_eq!(cancel["params"]["requestId"], request["id"]);
    let (idle, _) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "cancelled");
    assert_eq!(
        serde_json::from_str::<Value>(&lines(&project, "dialog", 1)[0]).unwrap(),
        json!({"type":"extension_ui_response","id":"ask-1","cancelled":true})
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_dialog_with_a_timeout_is_withdrawn() {
    let (_temp, project, mut child, mut reader, session) = session(r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"extension_ui_request","id":"ask-1","method":"select","title":"Choose","options":["Allow","Block"],"timeout":50}'
next_command
printf '%s\n' "$command" > dialog
settled
"#).await;
    send(&mut child, prompt_request(3, &session)).await;
    let (request, mut seen) = until_method(&mut reader, "elicitation/create").await;
    let (cancel, more) = until_method(&mut reader, "$/cancel_request").await;
    seen.extend(more);
    assert_eq!(cancel["params"]["requestId"], request["id"]);
    finish(&mut reader, 3, seen).await;
    assert_eq!(
        serde_json::from_str::<Value>(&lines(&project, "dialog", 1)[0]).unwrap(),
        json!({"type":"extension_ui_response","id":"ask-1","cancelled":true})
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_zero_timeout_keeps_the_dialog_open() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
emit '{"type":"extension_ui_request","id":"ask-1","method":"input","title":"Name","timeout":0}'
next_command
printf '%s\n' "$command" > dialog
settled
"#,
    )
    .await;
    send(&mut child, prompt_request(3, &session)).await;
    let (request, mut seen) = until_method(&mut reader, "elicitation/create").await;
    answer_form(
        &mut child,
        &request,
        json!({"action":"accept", "content":{"answer":"Ada"}}),
    )
    .await;
    // The script settles only after the dialog's answer reaches it, so
    // everything the bridge sends about the dialog precedes the turn's idle.
    loop {
        let value = receive(&mut reader).await;
        assert_ne!(
            value["method"], "$/cancel_request",
            "a zero timeout withdrew the dialog"
        );
        let idle = is_state(&value, "idle");
        seen.push(value);
        if idle {
            break;
        }
    }
    finish(&mut reader, 3, seen).await;
    assert_eq!(
        serde_json::from_str::<Value>(&lines(&project, "dialog", 1)[0]).unwrap(),
        json!({"type":"extension_ui_response","id":"ask-1","value":"Ada"})
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn an_aborted_run_and_pi_exit_withdraw_dialogs() {
    for ending in ["settled true", "exit 3"] {
        let script = format!(
            r#"
next_prompt
accept
emit '{{"type":"agent_start"}}'
emit '{{"type":"extension_ui_request","id":"ask-1","method":"input","title":"Name"}}'
# Wait for the client to observe the form, then trigger the boundary.
next_command
expect set_thinking_level
{ending}
"#
        );
        let (_temp, _project, mut child, mut reader, session) = session(&script).await;
        send(&mut child, prompt_request(3, &session)).await;
        let (request, _) = until_method(&mut reader, "elicitation/create").await;
        send(&mut child, config_request(4, &session, "thinking", "high")).await;
        let (cancel, _) = until_method(&mut reader, "$/cancel_request").await;
        assert_eq!(cancel["params"]["requestId"], request["id"], "{ending}");
        child.kill().await.unwrap();
    }
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

fn resume_request(id: i64, session: &Value, project: &Path, replay: Option<Value>) -> Value {
    let mut request = json!({"jsonrpc":"2.0","id":id,"method":"session/resume","params":{"sessionId":session,"cwd":project,"mcpServers":[]}});
    if let Some(replay) = replay {
        request["params"]["replayFrom"] = replay;
    }
    request
}

/// Starts a bridge beside `project` and asks it to resume `session`. Returns
/// the bridge, the response and the notifications before it.
async fn resume_in_new_bridge(
    project: &Path,
    session: &Value,
    id: i64,
    replay: Option<Value>,
) -> (Child, BufReader<ChildStdout>, Value, Vec<Value>) {
    let (mut child, mut reader) = start(&project.parent().unwrap().join("bin"));
    initialize(&mut child, &mut reader, json!({"elicitation":{"form":{}}})).await;
    send(&mut child, resume_request(id, session, project, replay)).await;
    let (result, notifications) = response(&mut reader, id).await;
    (child, reader, result, notifications)
}

#[tokio::test]
async fn a_replacement_bridge_resumes_the_pi_session_and_takes_its_options_again() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_prompt
accept
emit '{"type":"agent_start"}'
settled
"#,
    )
    .await;
    let (_, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert!(
        updates.iter().any(|v| kind(v) == "session_info_update"),
        "the first prompt titles the session: {updates:?}"
    );
    child.kill().await.unwrap();
    // Pi saved the prompt in its session file.
    fs::write(
        project.join("history.jsonl"),
        json!({"type":"message","id":"u1","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}).to_string(),
    )
    .unwrap();

    // The client sets the session's model and thinking level again on the
    // replacement Pi.
    fake_pi(
        &project.parent().unwrap().join("bin"),
        r#"
next_command
expect set_model
printf '%s\n' "$command" >> changes
current_model='{"provider":"second","id":"org/two","name":"Two"}'
answer set_model
describe
next_command
expect set_thinking_level
printf '%s\n' "$command" >> changes
thinking=low
answer set_thinking_level
describe
next_prompt
accept
emit '{"type":"agent_start"}'
settled
"#,
    );
    let (mut child, mut reader, resumed, notifications) =
        resume_in_new_bridge(&project, &session, 2, None).await;
    assert!(resumed["result"].is_object(), "{resumed}");
    assert_eq!(notifications.len(), 1, "{notifications:?}");
    assert!(is_state(&notifications[0], "idle"));
    send(
        &mut child,
        config_request(4, &session, "model", "second/org/two"),
    )
    .await;
    let (set, _) = response(&mut reader, 4).await;
    assert_eq!(
        set["result"]["configOptions"][0]["currentValue"],
        "second/org/two"
    );
    send(&mut child, config_request(5, &session, "thinking", "low")).await;
    let (set, _) = response(&mut reader, 5).await;
    assert_eq!(set["result"]["configOptions"][1]["currentValue"], "low");
    let (_, updates) = prompt(&mut child, &mut reader, 6, &session).await;
    assert!(
        !updates.iter().any(|v| kind(v) == "session_info_update"),
        "a resumed session keeps its title: {updates:?}"
    );

    let calls = lines(&project, "calls", 4);
    assert_eq!(
        calls[0],
        format!(
            "--mode rpc --approve --session-id {}",
            session.as_str().unwrap()
        )
    );
    assert_eq!(calls[0], calls[2]);
    let changes = lines(&project, "changes", 2);
    let model: Value = serde_json::from_str(&changes[0]).unwrap();
    assert_eq!(model["modelId"], "org/two");
    let thinking: Value = serde_json::from_str(&changes[1]).unwrap();
    assert_eq!(thinking["level"], "low");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn replay_recovers_recorded_messages_tools_and_patches_before_state_and_response() {
    let (_temp, project, mut child, _reader, session) = session(
        r"
while :; do describe; done
",
    )
    .await;
    let entries = [
        json!({"type":"session","id":"native"}),
        json!({"type":"message","id":"u1","message":{"role":"user","content":[{"type":"text","text":"old question"},{"type":"image","data":"aW1n","mimeType":"image/png"}]}}),
        json!({"type":"compaction","summary":"summary","firstKeptEntryId":"a1"}),
        json!({"type":"message","id":"a1","message":{"role":"assistant","content":[{"type":"thinking","thinking":"thinking"},{"type":"text","text":"answer"},{"type":"toolCall","id":"edit-1","name":"edit","arguments":{"path":"a.txt","edits":[{"oldText":"old","newText":"new"}]}},{"type":"toolCall","id":"write-1","name":"write","arguments":{"path":"missing.txt","content":"historical"}}]}}),
        json!({"type":"message","id":"r1","message":{"role":"toolResult","toolCallId":"edit-1","isError":false,"content":[{"type":"text","text":"edited"}],"details":{"patch":"--- a.txt\n+++ a.txt\n@@ -1 +1 @@\n-old\n+new\n"}}}),
        json!({"type":"message","id":"r2","message":{"role":"toolResult","toolCallId":"write-1","isError":true,"content":[{"type":"text","text":"failed"}]}}),
    ];
    fs::write(
        project.join("history.jsonl"),
        entries
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    child.kill().await.unwrap();
    let (mut child, _reader, resumed, notifications) =
        resume_in_new_bridge(&project, &session, 3, Some(json!({"type":"start"}))).await;
    assert!(resumed["result"].is_object(), "{resumed}");
    assert!(is_state(notifications.last().unwrap(), "idle"));
    assert_eq!(notifications.len(), 8, "{notifications:?}");
    assert_eq!(
        resumed["result"]["configOptions"][0]["currentValue"],
        "first/one"
    );
    assert_eq!(
        resumed["result"]["availableCommands"][2]["name"],
        "skill:review"
    );
    let queries = lines(&project, "queries", 8);
    let states = queries
        .iter()
        .filter(|line| line.contains("\"type\":\"get_state\""))
        .count();
    assert_eq!(
        states, 2,
        "one get_state at startup and one at resume: {queries:?}"
    );
    assert_eq!(update(&notifications[0])["messageId"], "u1");
    assert_eq!(update(&notifications[0])["content"][1]["data"], "aW1n");
    assert_eq!(update(&notifications[1])["messageId"], "a1:0");
    assert_eq!(kind(&notifications[1]), "agent_thought");
    assert_eq!(update(&notifications[2])["messageId"], "a1:1");
    let edit = tool_updates(&notifications, "edit-1");
    assert_eq!(statuses(&edit), ["pending", "completed"]);
    assert!(
        edit[1]["content"][0]["patch"]["text"]
            .as_str()
            .unwrap()
            .contains("-old\n+new")
    );
    let write = tool_updates(&notifications, "write-1");
    assert_eq!(statuses(&write), ["pending", "failed"]);
    assert_eq!(write[0]["rawInput"]["content"], "historical");
    assert_eq!(write[1]["content"][0]["type"], "content");
    // Replaying again must not infer that a past write created today's file.
    let mut entries = entries;
    entries[5]["message"]["isError"] = json!(false);
    fs::write(
        project.join("history.jsonl"),
        entries
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    child.kill().await.unwrap();
    let (mut child, _reader, _, notifications) =
        resume_in_new_bridge(&project, &session, 4, Some(json!({"type":"start"}))).await;
    let write = tool_updates(&notifications, "write-1");
    assert_eq!(statuses(&write), ["pending", "completed"]);
    assert_eq!(write[1]["content"][0]["type"], "content");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn replay_empty_and_invalid_history_and_exited_pi() {
    let (_temp, project, mut child, _reader, session) = session("").await;
    child.kill().await.unwrap();
    let replay = || Some(json!({"type":"start"}));

    let (mut child, _reader, result, notifications) =
        resume_in_new_bridge(&project, &session, 3, replay()).await;
    assert!(result["result"].is_object(), "{result}");
    assert_eq!(notifications.len(), 1);
    assert!(is_state(&notifications[0], "idle"));
    child.kill().await.unwrap();

    fs::write(project.join("history.jsonl"), "broken json").unwrap();
    let (mut child, _reader, result, notifications) =
        resume_in_new_bridge(&project, &session, 4, replay()).await;
    assert_eq!(result["error"]["code"], -32603, "{result}");
    assert!(notifications.is_empty());
    child.kill().await.unwrap();

    fake_pi_script(
        &project.parent().unwrap().join("bin"),
        "next_command\nexpect get_state\nexit 3",
    );
    let (mut child, _reader, result, _) =
        resume_in_new_bridge(&project, &session, 5, replay()).await;
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pi exited"),
        "{result}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn mcp_servers_reach_every_pi_through_the_extension() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi(&bin, "");
    let servers = json!([{"type":"stdio","name":"ox","command":"/usr/local/bin/ox-mcp",
        "args":["--session","s1"],"env":[{"name":"KEY","value":"value"}]}]);
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader, json!({})).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":project,"mcpServers":servers}}),
    )
    .await;
    let (new, _) = response(&mut reader, 2).await;
    let session = new["result"]["sessionId"].clone();
    assert!(session.is_string(), "{new}");
    let snapshot = receive(&mut reader).await;
    assert!(is_state(&snapshot, "idle"), "{snapshot}");
    child.kill().await.unwrap();

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader, json!({})).await;
    let mut resume = resume_request(3, &session, &project, None);
    resume["params"]["mcpServers"] = servers;
    send(&mut child, resume).await;
    let (resumed, _) = response(&mut reader, 3).await;
    assert!(resumed["result"].is_object(), "{resumed}");
    child.kill().await.unwrap();

    let calls = lines(&project, "calls", 4);
    let extension = calls[0]
        .strip_prefix(&format!(
            "--mode rpc --approve --session-id {} -e ",
            session.as_str().unwrap()
        ))
        .unwrap_or_else(|| panic!("{calls:?}"));
    assert_eq!(
        fs::read_to_string(extension).unwrap(),
        include_str!("../src/mcp-servers.js")
    );
    assert_eq!(calls[0], calls[2]);
    let given = lines(&project, "servers", 2);
    let expected = json!({"ox":{"command":"/usr/local/bin/ox-mcp","args":["--session","s1"],
        "env":{"KEY":"value"},"exposure":"direct"}});
    assert_eq!(serde_json::from_str::<Value>(&given[0]).unwrap(), expected);
    assert_eq!(given[0], given[1]);
}

#[tokio::test]
async fn invalid_mcp_servers_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi(&bin, "");
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader, json!({})).await;
    let stdio = |name: &str, command: &str| json!({"type":"stdio","name":name,"command":command,"args":[],"env":[]});
    let refused = [
        json!([{"type":"http","name":"context","url":"https://example.com/mcp","headers":[]}]),
        json!([stdio("x", "node")]),
        json!([stdio("o x", "/usr/bin/node")]),
        json!([stdio("a-b", "/usr/bin/node"), stdio("a_b", "/usr/bin/node")]),
    ];
    for (id, servers) in (2..).zip(refused) {
        send(
            &mut child,
            json!({"jsonrpc":"2.0","id":id,"method":"session/new","params":{"cwd":project,"mcpServers":servers}}),
        )
        .await;
        let (answer, _) = response(&mut reader, id).await;
        assert_eq!(answer["error"]["code"], -32602, "{servers}: {answer}");
    }
    let mut resume = resume_request(
        6,
        &json!("8a4f2c1e-0b7d-4e5a-9c3f-1d2e3f4a5b6c"),
        &project,
        None,
    );
    resume["params"]["mcpServers"] = json!([stdio("x", "node")]);
    send(&mut child, resume).await;
    let (answer, _) = response(&mut reader, 6).await;
    assert_eq!(answer["error"]["code"], -32602, "{answer}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn resume_rejects_unknown_ids_and_cursors() {
    let (_temp, project, mut child, _reader, session) = session("").await;
    child.kill().await.unwrap();
    let (mut child, mut reader, result, _) =
        resume_in_new_bridge(&project, &json!("unknown"), 3, None).await;
    assert_eq!(result["error"]["code"], -32002, "{result}");
    send(
        &mut child,
        resume_request(
            4,
            &session,
            &project,
            Some(json!({"type":"cursor","cursor":"a"})),
        ),
    )
    .await;
    let (result, _) = response(&mut reader, 4).await;
    assert_eq!(result["error"]["code"], -32602, "{result}");
    child.kill().await.unwrap();
}

fn config_request(id: i64, session: &Value, config: &str, value: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"session/set_config_option","params":{
        "sessionId":session,"configId":config,"type":"id","value":value
    }})
}

#[tokio::test]
async fn a_config_change_restarts_pi_and_refreshes_its_commands() {
    let (_temp, project, mut child, mut reader, session) =
        session("next_prompt\naccept\nexit 3").await;
    let (exited, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(exited["stopReason"], "error", "{exited}");
    fake_pi_script(
        &project.parent().unwrap().join("bin"),
        r#"
while :; do
  next_command
  case "$command" in
    *'"type":"set_thinking_level"'*) answer set_thinking_level;;
    *'"type":"get_state"'*) answer get_state ',"data":{"model":{"provider":"first","id":"one"},"thinkingLevel":"high"}';;
    *'"type":"get_available_models"'*) answer get_available_models ',"data":{"models":[{"provider":"first","id":"one","name":"One"}]}';;
    *'"type":"get_available_thinking_levels"'*) answer get_available_thinking_levels ',"data":{"levels":["off","high"]}';;
    *'"type":"get_commands"'*) answer get_commands ',"data":{"commands":[{"name":"skill:new","description":"New skill"}]}';;
    *) exit 20;;
  esac
done
"#,
    );
    send(&mut child, config_request(4, &session, "thinking", "high")).await;
    let (result, mut notifications) = response(&mut reader, 4).await;
    assert!(result["result"].is_object(), "{result}");
    while !notifications
        .iter()
        .any(|value| kind(value) == "available_commands_update")
    {
        notifications.push(receive(&mut reader).await);
    }
    let catalog = notifications
        .iter()
        .find(|value| kind(value) == "available_commands_update")
        .unwrap();
    assert_eq!(update(catalog)["availableCommands"][0]["name"], "skill:new");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn config_changes_use_pi_values_and_publish_the_resulting_options() {
    let (_temp, project, mut child, mut reader, session) = session(
        r#"
next_command
expect set_model
printf '%s\n' "$command" >> changes
current_model='{"provider":"second","id":"org/two","name":"Two"}'
thinking=low
answer set_model
describe
next_command
expect set_thinking_level
printf '%s\n' "$command" >> changes
thinking=high
answer set_thinking_level
describe
"#,
    )
    .await;
    for (id, config, value, level) in [
        (3, "model", "second/org/two", "low"),
        (4, "thinking", "high", "high"),
    ] {
        send(&mut child, config_request(id, &session, config, value)).await;
        let (result, notifications) = response(&mut reader, id).await;
        assert!(result["result"].is_object(), "{result}");
        let options = &result["result"]["configOptions"];
        assert_eq!(options[0]["currentValue"], "second/org/two");
        assert_eq!(options[1]["currentValue"], level);
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        assert_eq!(kind(&notifications[0]), "config_option_update");
        assert_eq!(update(&notifications[0])["configOptions"], *options);
    }
    let changes = lines(&project, "changes", 2);
    let model: Value = serde_json::from_str(&changes[0]).unwrap();
    assert_eq!(model["provider"], "second");
    assert_eq!(model["modelId"], "org/two");
    let thinking: Value = serde_json::from_str(&changes[1]).unwrap();
    assert_eq!(thinking["level"], "high");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn config_changes_reject_invalid_values_and_report_pi_errors() {
    let (_temp, _project, mut child, mut reader, session) = session(
        r#"
next_command
expect set_model
id=$(printf '%s\n' "$command" | sed 's/.*"id":"\([^"]*\)".*/\1/')
printf '{"id":"%s","type":"response","command":"set_model","success":false,"error":"Model not found: first/missing"}\n' "$id"
"#,
    )
    .await;
    for (id, request) in [
        (3, config_request(3, &session, "unknown", "high")),
        (4, config_request(4, &session, "thinking", "huge")),
        (5, config_request(5, &session, "model", "no-provider")),
        (
            6,
            json!({"jsonrpc":"2.0","id":6,"method":"session/set_config_option","params":{
                "sessionId":session,"configId":"thinking","type":"boolean","value":true
            }}),
        ),
    ] {
        send(&mut child, request).await;
        let (result, notifications) = response(&mut reader, id).await;
        assert_eq!(result["error"]["code"], -32602, "{result}");
        assert!(notifications.is_empty(), "{notifications:?}");
    }
    send(
        &mut child,
        config_request(7, &json!("unknown"), "thinking", "high"),
    )
    .await;
    let (result, _) = response(&mut reader, 7).await;
    assert_eq!(result["error"]["code"], -32002, "{result}");
    send(
        &mut child,
        config_request(8, &session, "model", "first/missing"),
    )
    .await;
    let (result, notifications) = response(&mut reader, 8).await;
    assert_eq!(result["error"]["code"], -32603, "{result}");
    assert_eq!(result["error"]["message"], "Model not found: first/missing");
    assert!(notifications.is_empty(), "{notifications:?}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn skill_commands_reach_pi_with_their_native_name() {
    let (_temp, project, mut child, mut reader, session) =
        session("next_prompt\naccept handled").await;
    let mut request = prompt_request(3, &session);
    request["params"]["prompt"][0]["text"] = json!("/skill:review src/main.rs");
    send(&mut child, request).await;
    finish(&mut reader, 3, Vec::new()).await;
    let sent: Value = serde_json::from_str(&lines(&project, "prompts", 1)[0]).unwrap();
    assert_eq!(sent["message"], "/skill:review src/main.rs");
    child.kill().await.unwrap();
}

/// A client's system prompt is appended to Pi's.
#[tokio::test]
async fn appends_the_client_system_prompt_to_pi() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_pi_script(&bin, "printf '%s\\n' \"$@\" > args.txt\ndescribe");
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader, json!({})).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":project,"mcpServers":[],
            "systemPrompt":{"mode":"append","content":[{"type":"text","text":"Be brief."}]}
        }}),
    )
    .await;
    let (new, _) = response(&mut reader, 2).await;
    assert!(new["result"]["sessionId"].is_string(), "{new}");
    let snapshot = receive(&mut reader).await;
    assert!(is_state(&snapshot, "idle"), "{snapshot}");
    child.kill().await.unwrap();
    let args = fs::read_to_string(project.join("args.txt")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let appended = args
        .iter()
        .position(|arg| *arg == "--append-system-prompt")
        .expect("--append-system-prompt");
    assert_eq!(args[appended + 1], "Be brief.");
}
