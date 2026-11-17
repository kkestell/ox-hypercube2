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

/// What every fake claude does with its stdin, which stays open across
/// turns as the real one's does.
const FAKE_CLAUDE: &str = r#"
# Reads the next message into $message. Claude ends when its stdin closes.
next_message() {
  IFS= read -r message || exit 0
}
# Reports the turn of the prompt in $message as started.
started() {
  uuid=$(printf '%s\n' "$message" | sed 's/.*"uuid":"\([^"]*\)".*/\1/')
  printf '{"type":"command_lifecycle","command_uuid":"%s","state":"started"}\n' "$uuid"
}
# Reads a prompt and starts its turn.
next_prompt() {
  while :; do
    next_message
    case "$message" in
      *'"type":"user"'*) break;;
      *'"type":"control_request"'*) accept;;
      *) exit 20;;
    esac
  done
  started
}
# Waits for an interrupt and ends the turn it stops.
interrupted() {
  next_message
  case "$message" in *'"subtype":"interrupt"'*) ;; *) exit 21;; esac
  printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}'
  printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_tools"}'
}
# Says claude is running or idle.
state() {
  printf '{"type":"system","subtype":"session_state_changed","state":"%s"}\n' "$1"
}
# Echoes the prompt in $message as taken into a turn, as claude does with
# --replay-user-messages.
echoed() {
  uuid=$(printf '%s\n' "$message" | sed 's/.*"uuid":"\([^"]*\)".*/\1/')
  printf '{"type":"user","uuid":"%s","isReplay":true,"message":{"role":"user","content":[{"type":"text","text":"hello"}]}}\n' "$uuid"
}
# Accepts the control request in $message, and notes it in `controls`.
accept() {
  printf '%s\n' "$message" >> controls
  request=$(printf '%s\n' "$message" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
  printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s"}}\n' "$request"
}
"#;

fn fake_claude(dir: &Path, script: &str) {
    fake_claude_with_init(
        dir,
        r#"
next_message
case "$message" in *'"subtype":"initialize"'*) ;; *) exit 19;; esac
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"commands":[]}}}'
"#,
        script,
    );
}

fn fake_claude_with_init(dir: &Path, initialization: &str, script: &str) {
    let path = dir.join("claude");
    fs::write(
        &path,
        format!("#!/bin/sh\n{FAKE_CLAUDE}\n{initialization}\n{script}\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn start(dir: &Path) -> (Child, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_claude-code-acp"))
        .env(
            "PATH",
            format!("{}:{}", dir.display(), std::env::var("PATH").unwrap()),
        )
        .env(
            "CLAUDE_CONFIG_DIR",
            dir.parent().unwrap().join("empty-claude"),
        )
        .env("ANTHROPIC_API_KEY", "must-be-removed")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let reader = BufReader::new(child.stdout.take().unwrap());
    (child, reader)
}

/// The text an update shows: a chunk's, or the first block of a whole
/// message.
fn shown_text(notification: &Value) -> Option<&str> {
    let content = &update(notification)["content"];
    content["text"]
        .as_str()
        .or_else(|| content[0]["text"].as_str())
}

/// Reads until the prompt `id` is answered and its turn goes idle.
async fn finished(reader: &mut BufReader<ChildStdout>, id: i64) -> (Value, Vec<Value>) {
    finish(reader, id, Vec::new()).await
}

/// Reads until the client gets a permission request. Returns it and what
/// was read before it.
async fn until_permission(reader: &mut BufReader<ChildStdout>) -> (Value, Vec<Value>) {
    let mut seen = Vec::new();
    loop {
        let value = receive(reader).await;
        if value["method"] == "session/request_permission" {
            return (value, seen);
        }
        seen.push(value);
    }
}

async fn initialize(child: &mut Child, reader: &mut BufReader<ChildStdout>) {
    initialize_with(child, reader, json!({})).await;
}

/// Initializes the bridge as a client with `capabilities`.
async fn initialize_with(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    capabilities: Value,
) {
    send(
        child,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":2,"capabilities":capabilities,"info":{"name":"test","version":"1"}
        }}),
    )
    .await;
    let (value, _) = response(reader, 1).await;
    assert_eq!(value["result"]["protocolVersion"], 2);
    let session = &value["result"]["capabilities"]["session"];
    assert!(session["prompt"]["image"].is_object(), "{value}");
    assert!(session["prompt"]["embeddedContext"].is_object(), "{value}");
    assert!(session["mcp"]["stdio"].is_object(), "{value}");
}

#[tokio::test]
async fn commands_are_discovered_before_a_prompt_and_replaced_live() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude_with_init(
        &bin,
        r#"
printf '%s\n' "$*" >> calls
next_message
case "$message" in *'"subtype":"initialize"'*) ;; *) exit 19;; esac
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"commands":[{"name":"review","description":"Review a file","argumentHint":"file"},{"name":"clear","description":"Clear context","builtin":true}]}}}'
"#,
        r#"
next_prompt
printf '%s\n' "$message" > captured-prompt.json
printf '%s\n' '{"type":"system","subtype":"commands_changed","commands":[{"name":"plugin:check","description":"Check","argumentHint":"target"}]}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"reviewed"}]}}'
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2"}'
next_prompt
printf '%s\n' '{"type":"system","subtype":"commands_changed","commands":[]}'
printf '%s\n' '{"type":"result","local_command":"clear","num_turns":0}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":project}}),
    )
    .await;
    let (new, _) = response(&mut reader, 2).await;
    let session = new["result"]["sessionId"].clone();
    assert_eq!(
        new["result"]["configOptions"].as_array().unwrap().len(),
        3,
        "{new}"
    );
    let snapshot = receive(&mut reader).await;
    assert!(is_state(&snapshot, "idle"), "{snapshot}");
    let initial = json!([
        {"type":"command","name":"review","description":"Review a file","input":{"type":"text","hint":"file"}},
        {"type":"command","name":"clear","description":"Clear context"}
    ]);
    let discovered = receive(&mut reader).await;
    assert_eq!(
        update(&discovered)["sessionUpdate"],
        "available_commands_update",
        "{discovered}"
    );
    assert_eq!(
        update(&discovered)["availableCommands"],
        initial,
        "{discovered}"
    );
    assert_eq!(
        calls(&project).len(),
        1,
        "discovery starts exactly one process"
    );

    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"/review src/main.rs"}]}})).await;
    let (_, updates) = finished(&mut reader, 3).await;
    assert!(
        updates
            .iter()
            .any(|value| shown_text(value) == Some("reviewed")),
        "{updates:?}"
    );
    let captured: Value =
        serde_json::from_slice(&fs::read(project.join("captured-prompt.json")).unwrap()).unwrap();
    assert_eq!(
        captured["message"]["content"],
        json!([
            {"type":"text", "text":"/review src/main.rs"}
        ])
    );
    let changed = updates
        .iter()
        .find(|value| update(value)["sessionUpdate"] == "available_commands_update")
        .unwrap();
    assert_eq!(
        update(changed)["availableCommands"][0]["name"],
        "plugin:check"
    );

    send(&mut child, json!({"jsonrpc":"2.0","id":5,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"/clear"}]}})).await;
    let (idle, updates) = finished(&mut reader, 5).await;
    assert_eq!(idle["stopReason"], "end_turn");
    assert!(
        updates.iter().any(
            |value| update(value)["sessionUpdate"] == "available_commands_update"
                && update(value)["availableCommands"] == json!([])
        ),
        "{updates:?}"
    );
    child.kill().await.unwrap();

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/resume","params":{"sessionId":session,"cwd":project,"replayFrom":{"type":"start"}}})).await;
    // Claude reports its commands from its own output, on either side of the
    // response.
    let (_, mut seen) = response(&mut reader, 2).await;
    let is_commands =
        |message: &Value| update(message)["sessionUpdate"] == "available_commands_update";
    while !seen.iter().any(is_commands) {
        seen.push(receive(&mut reader).await);
    }
    let discovered = seen.iter().find(|message| is_commands(message)).unwrap();
    assert_eq!(update(discovered)["availableCommands"], initial);
    assert_eq!(calls(&project).len(), 2);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn native_initialization_failure_fails_the_first_prompt() {
    for (initialization, expected) in [
        (
            r#"next_message
printf '%s\n' '{"type":"control_response","response":{"subtype":"error","request_id":"initialize","error":"initialization refused"}}'
next_message"#,
            "initialization refused",
        ),
        (
            "printf '%s\\n' 'startup diagnostic' >&2\nexit 23",
            "startup diagnostic",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (bin, project) = dirs(temp.path());
        fake_claude_with_init(&bin, initialization, "");
        let (mut child, mut reader) = start(&bin);
        initialize(&mut child, &mut reader).await;
        send(
            &mut child,
            json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":project}}),
        )
        .await;
        let (new, _) = response(&mut reader, 2).await;
        let session = new["result"]["sessionId"].clone();
        assert!(session.is_string(), "{new}");
        send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})).await;
        let (failed, _) = response(&mut reader, 3).await;
        assert_eq!(failed["error"]["code"], -32603, "{failed}");
        assert!(
            failed["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{failed}"
        );
        child.kill().await.unwrap();
    }
}

#[tokio::test]
async fn streams_and_resumes_a_persisted_session() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    let project = temp.path().join("project");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&project).unwrap();
    fake_claude(
        &bin,
        r#"
if [ -n "$ANTHROPIC_API_KEY" ] || [ -n "$ANTHROPIC_AUTH_TOKEN" ]; then exit 11; fi
mode=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--permission-mode' ]; then mode="$2"; break; fi
  shift
done
printf '%s\n' "$mode" >> launch-modes
next_prompt
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello "}}}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"},{"type":"tool_use","name":"Read","input":{"file_path":"x"}}]}}'
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2","result":"hello world"}'
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":project,"mcpServers":[]
        }}),
    )
    .await;
    let (new, _) = response(&mut reader, 2).await;
    let id = new["result"]["sessionId"].as_str().unwrap().to_owned();
    let categories: Vec<_> = new["result"]["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|option| (option["configId"].clone(), option["category"].clone()))
        .collect();
    assert_eq!(
        categories,
        [
            (json!("model"), json!("model")),
            (json!("effort"), json!("thought_level")),
            (json!("mode"), json!("mode")),
        ]
    );
    // Each option offers only its current value.
    for (id, value) in [
        ("model", "claude-opus-5-5"),
        ("effort", "high"),
        ("mode", "manual"),
    ] {
        let option = option(&new, id);
        assert_eq!(option["currentValue"], value);
        assert_eq!(option["options"], json!([{"value": value, "name": value}]));
    }

    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":7,"method":"session/set_config_option","params":{
            "sessionId":id,"configId":"mode","type":"id","value":"plan"
        }}),
    )
    .await;
    let (set_mode, mode_updates) = response(&mut reader, 7).await;
    assert_eq!(option(&set_mode, "mode")["currentValue"], "plan");
    assert!(
        mode_updates.iter().any(|update| {
            update["params"]["update"]["sessionUpdate"] == "config_option_update"
        })
    );

    let set = set_config(
        &mut child,
        &mut reader,
        3,
        &json!(id),
        "model",
        "claude-haiku-5-5",
    )
    .await;
    assert_eq!(option(&set, "model")["currentValue"], "claude-haiku-5-5");

    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{
            "sessionId":id,"prompt":[{"type":"text","text":"say hello"}]
        }}),
    )
    .await;
    let (idle, updates) = finished(&mut reader, 4).await;
    assert_eq!(idle["stopReason"], "end_turn");
    let texts: Vec<_> = updates
        .iter()
        .filter(|v| v["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .map(|v| v["params"]["update"]["content"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["hello "]);
    assert!(
        updates
            .iter()
            .any(|v| v["params"]["update"]["sessionUpdate"] == "tool_call_update")
    );
    child.kill().await.unwrap();
    fs::remove_file(project.join("controls")).unwrap();

    // The client keeps the session's options and sets them again when a new
    // bridge resumes the session.
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":5,"method":"session/resume","params":{
            "sessionId":id,"cwd":project,"mcpServers":[]
        }}),
    )
    .await;
    response(&mut reader, 5).await;
    let set = set_config(&mut child, &mut reader, 8, &json!(id), "mode", "plan").await;
    assert_eq!(option(&set, "mode")["currentValue"], "plan");
    let set = set_config(
        &mut child,
        &mut reader,
        9,
        &json!(id),
        "model",
        "claude-haiku-5-5",
    )
    .await;
    assert_eq!(option(&set, "model")["currentValue"], "claude-haiku-5-5");
    prompt(&mut child, &mut reader, 6, &json!(id)).await;
    let controls = fs::read_to_string(project.join("controls")).unwrap();
    assert!(controls.contains(r#""mode":"plan""#), "{controls}");
    assert!(
        controls.contains(r#""model":"claude-haiku-5-5""#),
        "{controls}"
    );
    child.kill().await.unwrap();
}

/// The config option `id` in a response's `configOptions`.
fn option<'a>(response: &'a Value, id: &str) -> &'a Value {
    response["result"]["configOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("no config options: {response}"))
        .iter()
        .find(|option| option["configId"] == id)
        .unwrap_or_else(|| panic!("no {id} option: {response}"))
}

/// Sets a config option and returns the response.
async fn set_config(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    request_id: i64,
    session: &Value,
    config: &str,
    value: &str,
) -> Value {
    send(child, json!({"jsonrpc":"2.0","id":request_id,"method":"session/set_config_option","params":{"sessionId":session,"configId":config,"type":"id","value":value}})).await;
    response(reader, request_id).await.0
}

#[tokio::test]
async fn config_changes_reach_the_running_claude() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // Each turn says the mode, model and effort claude has: those it started
    // with, then those it was asked to change to. It refuses Sonnet.
    fake_claude(
        &bin,
        r#"
printf '%s\n' "$*" >> calls
while [ "$#" -gt 0 ]; do
  case "$1" in
    --permission-mode) mode="$2";;
    --model) model="$2";;
    --effort) effort="$2";;
  esac
  shift
done
while :; do
  next_message
  case "$message" in
    *'"subtype":"set_model"'*'sonnet'*)
      request=$(printf '%s\n' "$message" | sed 's/.*"request_id":"\([^"]*\)".*/\1/')
      printf '{"type":"control_response","response":{"subtype":"error","request_id":"%s","error":"no such model"}}\n' "$request";;
    *'"subtype":"set_permission_mode"'*)
      mode=$(printf '%s\n' "$message" | sed 's/.*"mode":"\([^"]*\)".*/\1/')
      accept;;
    *'"subtype":"set_model"'*)
      model=$(printf '%s\n' "$message" | sed 's/.*"model":"\([^"]*\)".*/\1/')
      accept;;
    *'"subtype":"apply_flag_settings"'*)
      effort=$(printf '%s\n' "$message" | sed 's/.*"effortLevel":"\([^"]*\)".*/\1/')
      accept;;
    *)
      started
      printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s %s %s"}]}}\n' "$mode" "$model" "$effort"
      printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2"}';;
  esac
done
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (_, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert!(
        updates
            .iter()
            .any(|v| shown_text(v) == Some("manual claude-opus-5-5 high")),
        "{updates:?}"
    );

    // The bridge passes any value to claude.
    let set = set_config(&mut child, &mut reader, 4, &session, "mode", "dontAsk").await;
    assert_eq!(option(&set, "mode")["currentValue"], "dontAsk");
    let model = "claude-opus-6";
    let set = set_config(&mut child, &mut reader, 5, &session, "model", model).await;
    assert_eq!(option(&set, "model")["currentValue"], model);
    let set = set_config(&mut child, &mut reader, 9, &session, "effort", "max").await;
    assert_eq!(option(&set, "effort")["currentValue"], "max");
    let set = set_config(&mut child, &mut reader, 10, &session, "speed", "fast").await;
    assert_eq!(set["error"]["code"], -32602, "{set}");
    let refused = "claude-sonnet-5-5";
    let set = set_config(&mut child, &mut reader, 6, &session, "model", refused).await;
    assert_eq!(set["error"]["message"], "no such model", "{set}");

    let (_, updates) = prompt(&mut child, &mut reader, 7, &session).await;
    assert!(
        updates
            .iter()
            .any(|v| shown_text(v) == Some("dontAsk claude-opus-6 max")),
        "{updates:?}"
    );
    // A bridge serves one session.
    send(&mut child, json!({"jsonrpc":"2.0","id":8,"method":"session/new","params":{"cwd":project,"mcpServers":[]}})).await;
    let (refused, _) = response(&mut reader, 8).await;
    assert_eq!(refused["error"]["code"], -32600, "{refused}");
    assert_eq!(calls(&project).len(), 1);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn cancellation_interrupts_the_turn_and_keeps_claude() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
printf '%s\n' "$*" >> calls
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"started"}]}}'
interrupted
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"again"}]}}'
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"wait"}]}})).await;
    let seen = until_text(&mut reader, "started").await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await;
    let (result, updates) = finish(&mut reader, 3, seen).await;
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    // Claude's note that it was interrupted is not something the user said:
    // the prompt is the only user message.
    let user_messages = updates
        .iter()
        .filter(|v| update(v)["sessionUpdate"] == "user_message")
        .count();
    assert_eq!(user_messages, 1, "{updates:?}");

    let (result, updates) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        updates.iter().any(|v| shown_text(v) == Some("again")),
        "{updates:?}"
    );
    assert_eq!(calls(&project).len(), 1);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn permission_request_round_trips_through_client() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    let project = temp.path().join("project");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&project).unwrap();
    fake_claude(
        &bin,
        r#"
next_prompt
case "$message" in *'"run touch"'*) ;; *) exit 11;; esac
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"touch /tmp/claude-approval-test"}}]}}'
printf '%s\n' '{"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":{"command":"touch /tmp/claude-approval-test"}}}'
IFS= read -r answer || exit 12
for part in '"type":"control_response"' '"request_id":"permission-1"' '"behavior":"allow"' '"toolUseID":"tool-1"'; do
  case "$answer" in *"$part"*) ;; *) exit 13;; esac
done
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":""}]}}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"approved"}]}}'
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2","result":"done"}'
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let id = new_session(&mut child, &mut reader, 2, &project).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{
            "sessionId":id,"prompt":[{"type":"text","text":"run touch"}]
        }}),
    )
    .await;
    let (permission, before) = until_permission(&mut reader).await;
    // The turn requires action while the request is open.
    let states: Vec<&str> = before
        .iter()
        .filter(|v| update(v)["sessionUpdate"] == "state_update")
        .map(|v| update(v)["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["running", "requires_action"], "{before:?}");
    let tool_call = &permission["params"]["subject"]["toolCall"];
    assert_eq!(tool_call["toolCallId"], "tool-1", "{permission}");
    assert_eq!(
        permission["params"]["title"],
        "Bash `touch /tmp/claude-approval-test`"
    );
    assert_eq!(
        tool_call["rawInput"]["command"],
        "touch /tmp/claude-approval-test"
    );
    // The input is the tool call's only description of the request.
    assert!(tool_call["content"].is_null(), "{tool_call}");
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":permission["id"],"result":{
            "outcome":{"outcome":"selected","optionId":"allow_once"}
        }}),
    )
    .await;
    let (idle, updates) = finish(&mut reader, 3, before).await;
    assert_eq!(idle["stopReason"], "end_turn");
    assert!(updates.iter().any(|v| shown_text(v) == Some("approved")));
    let statuses: Vec<&Value> = updates
        .iter()
        .filter(|v| update(v)["toolCallId"] == "tool-1")
        .map(|v| &update(v)["status"])
        .filter(|status| !status.is_null())
        .collect();
    assert_eq!(statuses, ["pending", "pending", "in_progress", "completed"]);
    // The turn runs again once the request is answered.
    let states: Vec<&str> = updates
        .iter()
        .filter(|v| update(v)["sessionUpdate"] == "state_update")
        .map(|v| update(v)["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["running", "requires_action", "running", "idle"]);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn persistent_approval_and_denial_reach_claude() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    let project = temp.path().join("project");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&project).unwrap();
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":{"command":"pwd"},"permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"pwd"}],"behavior":"allow","destination":"session"}]}}'
IFS= read -r answer || exit 11
case "$answer" in *'"behavior":"allow"'*) ;; *) exit 12;; esac
case "$answer" in *'"updatedPermissions"'*) ;; *) exit 13;; esac
printf '%s\n' '{"type":"control_request","request_id":"permission-2","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-2","input":{"command":"whoami"}}}'
IFS= read -r answer || exit 14
case "$answer" in *'"behavior":"deny"'*) ;; *) exit 15;; esac
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2","result":"done"}'
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":project,"mcpServers":[]}})).await;
    let (new, _) = response(&mut reader, 2).await;
    let id = new["result"]["sessionId"].as_str().unwrap().to_owned();
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":id,"prompt":[{"type":"text","text":"test permissions"}]}})).await;
    let (first, mut seen) = until_permission(&mut reader).await;
    assert!(
        first["params"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|option| option["optionId"] == "allow_always")
    );
    send(&mut child, json!({"jsonrpc":"2.0","id":first["id"],"result":{"outcome":{"outcome":"selected","optionId":"allow_always"}}})).await;
    let (second, more) = until_permission(&mut reader).await;
    seen.extend(more);
    assert!(
        !second["params"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|option| option["optionId"] == "allow_always")
    );
    send(&mut child, json!({"jsonrpc":"2.0","id":second["id"],"result":{"outcome":{"outcome":"selected","optionId":"reject_once"}}})).await;
    let (idle, _) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_permission_without_an_argument_is_titled_with_its_tool_name() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"mcp__x__act","tool_use_id":"tool-1","input":{},"title":"Claude wants to use `act`"}}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"act"}]}})).await;
    let (permission, _) = until_permission(&mut reader).await;
    assert_eq!(permission["params"]["title"], "Act");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn cancellation_stops_a_pending_permission() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // Claude withdraws a permission request when it is interrupted, and the
    // next message it reads is the next prompt, not an answer.
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"tool-1","input":{"command":"pwd"}}}'
next_message
case "$message" in *'"subtype":"interrupt"'*) ;; *) exit 21;; esac
printf '%s\n' '{"type":"control_cancel_request","request_id":"permission-1"}'
printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_tools"}'
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"after"}]}}'
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"run pwd"}]}})).await;
    let (permission, seen) = until_permission(&mut reader).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await;
    let (result, updates) = finish(&mut reader, 3, seen).await;
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    // The withdrawn request no longer requires action: the turn runs until
    // it ends, whether or not the client sees that in between.
    let states: Vec<&str> = updates
        .iter()
        .filter(|v| update(v)["sessionUpdate"] == "state_update")
        .map(|v| update(v)["state"].as_str().unwrap())
        .collect();
    let (last, rest) = states.split_last().unwrap();
    assert_eq!(*last, "idle", "{updates:?}");
    assert_eq!(rest[..2], ["running", "requires_action"], "{updates:?}");
    assert!(
        rest[2..].iter().all(|state| *state == "running"),
        "{states:?}"
    );
    // An answer to the withdrawn request does not reach claude.
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"cancelled"}}}),
    )
    .await;
    assert!(
        updates.iter().any(|v| {
            v["params"]["update"]["toolCallId"] == "tool-1"
                && v["params"]["update"]["status"] == "failed"
        }),
        "{updates:?}"
    );

    let (result, updates) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        updates.iter().any(|v| shown_text(v) == Some("after")),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

/// A fake claude's turn that calls `AskUserQuestion` with two questions as
/// `tool-1`, reads the answer into `$answer`, and ends the turn once the
/// answer holds each of `parts`, or else exits with 13.
fn asking_claude(parts: &[&str]) -> String {
    let checks: Vec<String> = parts
        .iter()
        .map(|part| format!("case \"$answer\" in *'{part}'*) ;; *) exit 13;; esac"))
        .collect();
    format!(
        r#"
next_prompt
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"tool-1","name":"AskUserQuestion","input":{QUESTIONS}}}]}}}}'
printf '%s\n' '{{"type":"control_request","request_id":"ask-1","request":{{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"tool-1","input":{QUESTIONS}}}}}'
IFS= read -r answer || exit 12
{}
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"tool-1","content":"answered"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2","result":"done"}}'
next_message
"#,
        checks.join("\n")
    )
}

/// The input of the `AskUserQuestion` call in `asking_claude`.
const QUESTIONS: &str = r#"{"questions":[{"question":"How should I format the output?","header":"Format","options":[{"label":"Summary","description":"Brief overview"},{"label":"Detailed","description":"Full explanation"}],"multiSelect":false},{"question":"Which sections should I include?","header":"Sections","options":[{"label":"Introduction","description":"Opening context"},{"label":"Conclusion","description":"Final summary"}],"multiSelect":true}]}"#;

/// The capabilities of a client that takes form elicitations.
fn forms() -> Value {
    json!({"elicitation":{"form":{}}})
}

/// Starts a bridge on the fake claude `script` as a client that takes forms,
/// prompts a new session, and reads until the client is asked to elicit.
/// Returns the bridge, the elicitation request, and what was read before it;
/// the prompt has ID 3.
async fn asked(
    temp: &Path,
    script: &str,
) -> (Child, BufReader<ChildStdout>, Value, Value, Vec<Value>) {
    let (bin, project) = dirs(temp);
    fake_claude(&bin, script);
    let (mut child, mut reader) = start(&bin);
    initialize_with(&mut child, &mut reader, forms()).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"ask me"}]}})).await;
    let (elicitation, before) = until_method(&mut reader, "elicitation/create").await;
    (child, reader, session, elicitation, before)
}

#[tokio::test]
async fn ask_user_question_becomes_a_form() {
    let temp = tempfile::tempdir().unwrap();
    let script = asking_claude(&[
        r#""request_id":"ask-1""#,
        r#""behavior":"allow""#,
        r#""toolUseID":"tool-1""#,
        r#""answers":{"How should I format the output?":"Summary","Which sections should I include?":"Introduction, Appendix"}"#,
        r#""questions":[{"#,
    ]);
    let (mut child, mut reader, session, elicitation, before) = asked(temp.path(), &script).await;
    let params = &elicitation["params"];
    assert_eq!(params["mode"], "form", "{elicitation}");
    assert_eq!(params["sessionId"], session, "{elicitation}");
    assert_eq!(params["toolCallId"], "tool-1", "{elicitation}");
    assert_eq!(params["message"], "Claude has questions", "{elicitation}");
    let schema = &params["requestedSchema"];
    assert_eq!(
        schema["properties"]["q1"],
        json!({
            "type": "string",
            "title": "Format",
            "description": "How should I format the output?",
            "oneOf": [
                {"const": "Summary", "title": "Summary", "description": "Brief overview"},
                {"const": "Detailed", "title": "Detailed", "description": "Full explanation"}
            ]
        }),
        "{elicitation}"
    );
    assert_eq!(schema["properties"]["q2"]["type"], "array", "{elicitation}");
    assert_eq!(
        schema["properties"]["q2-other"],
        json!({"type": "string", "title": "Other"}),
        "{elicitation}"
    );
    assert!(schema["required"].is_null(), "{elicitation}");
    // The turn requires action while the questions are open.
    let states: Vec<&str> = before
        .iter()
        .filter(|v| update(v)["sessionUpdate"] == "state_update")
        .map(|v| update(v)["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["running", "requires_action"], "{before:?}");
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":elicitation["id"],"result":{
            "action":"accept",
            "content":{"q1":"Summary","q2":["Introduction"],"q2-other":"Appendix"}
        }}),
    )
    .await;
    let (idle, updates) = finish(&mut reader, 3, before).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    let statuses: Vec<&Value> = updates
        .iter()
        .filter(|v| update(v)["toolCallId"] == "tool-1")
        .map(|v| &update(v)["status"])
        .filter(|status| !status.is_null())
        .collect();
    assert_eq!(statuses, ["pending", "pending", "in_progress", "completed"]);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn declined_questions_deny_the_tool() {
    let temp = tempfile::tempdir().unwrap();
    let script = asking_claude(&[
        r#""behavior":"deny""#,
        r#""message":"The user declined to answer.""#,
        r#""toolUseID":"tool-1""#,
    ]);
    let (mut child, mut reader, _, elicitation, before) = asked(temp.path(), &script).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":elicitation["id"],"result":{"action":"decline"}}),
    )
    .await;
    let (idle, updates) = finish(&mut reader, 3, before).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    assert!(
        updates
            .iter()
            .any(|v| update(v)["toolCallId"] == "tool-1" && update(v)["status"] == "failed"),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn withdrawn_questions_cancel_the_elicitation() {
    let temp = tempfile::tempdir().unwrap();
    // Claude withdraws the questions when it is interrupted.
    let script = format!(
        r#"
next_prompt
printf '%s\n' '{{"type":"control_request","request_id":"ask-1","request":{{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"tool-1","input":{QUESTIONS}}}}}'
next_message
case "$message" in *'"subtype":"interrupt"'*) ;; *) exit 21;; esac
printf '%s\n' '{{"type":"control_cancel_request","request_id":"ask-1"}}'
printf '%s\n' '{{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_tools"}}'
next_message
"#
    );
    let (mut child, mut reader, session, elicitation, mut seen) = asked(temp.path(), &script).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await;
    let (cancel, more) = until_method(&mut reader, "$/cancel_request").await;
    assert_eq!(cancel["params"]["requestId"], elicitation["id"], "{cancel}");
    seen.extend(more);
    let (idle, _) = finish(&mut reader, 3, seen).await;
    assert_eq!(idle["stopReason"], "cancelled", "{idle}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn passes_stdio_mcp_servers_to_claude() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
printf '%s\n' "$@" > args.txt
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then printf '%s' "$2" > mcp-config.json; break; fi
  shift
done
next_prompt
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let args = json!([
        "--url",
        "https://example.com/mcp?a=1&b=2",
        "{\"root\":\"/tmp\"}"
    ]);
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":project,"mcpServers":[{"type":"stdio","name":"filesystem","command":"/opt/tools/mcp-filesystem","args":args,
                "env":[{"name":"TOKEN","value":"test"}]}]
        }}),
    )
    .await;
    let (new, _) = response(&mut reader, 2).await;
    let id = new["result"]["sessionId"]
        .as_str()
        .expect("a session ID")
        .to_owned();
    let (idle, _) = prompt(&mut child, &mut reader, 3, &json!(id)).await;
    child.kill().await.unwrap();
    assert_eq!(idle["stopReason"], "end_turn");
    let config: Value =
        serde_json::from_slice(&fs::read(project.join("mcp-config.json")).unwrap()).unwrap();
    assert_eq!(
        config,
        json!({"mcpServers":{"filesystem":{
            "command":"/opt/tools/mcp-filesystem","args":args,"env":{"TOKEN":"test"}
        }}})
    );
    let claude_args = fs::read_to_string(project.join("args.txt")).unwrap();
    let claude_args: Vec<&str> = claude_args.lines().collect();
    let allowed = claude_args
        .iter()
        .position(|arg| *arg == "--allowedTools")
        .expect("--allowedTools");
    assert_eq!(claude_args[allowed + 1], "mcp__filesystem");
    assert_eq!(claude_args[allowed + 2], "--permission-mode");
}

/// A client's system prompt in `append` mode is appended to Claude's; one
/// in another mode is refused.
#[tokio::test]
async fn appends_the_client_system_prompt_to_claude() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
printf '%s\n' "$@" > args.txt
next_prompt
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let system_prompt =
        |mode: &str| json!({"mode":mode,"content":[{"type":"text","text":"Be brief."}]});
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":project,"mcpServers":[],"systemPrompt":system_prompt("override")
        }}),
    )
    .await;
    let (refused, _) = response(&mut reader, 2).await;
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":3,"method":"session/new","params":{
            "cwd":project,"mcpServers":[],"systemPrompt":system_prompt("append")
        }}),
    )
    .await;
    let (new, _) = response(&mut reader, 3).await;
    let id = new["result"]["sessionId"]
        .as_str()
        .expect("a session ID")
        .to_owned();
    let (idle, _) = prompt(&mut child, &mut reader, 4, &json!(id)).await;
    child.kill().await.unwrap();
    assert_eq!(idle["stopReason"], "end_turn");
    let claude_args = fs::read_to_string(project.join("args.txt")).unwrap();
    let claude_args: Vec<&str> = claude_args.lines().collect();
    let appended = claude_args
        .iter()
        .position(|arg| *arg == "--append-system-prompt")
        .expect("--append-system-prompt");
    assert_eq!(claude_args[appended + 1], "Be brief.");
}

#[tokio::test]
async fn new_sessions_are_fresh_and_resume_is_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
printf '%s\n' "$*" >> calls
next_prompt
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let mut ids = Vec::new();
    for _ in 0..2 {
        let (mut child, mut reader) = start(&bin);
        initialize(&mut child, &mut reader).await;
        let session = new_session(&mut child, &mut reader, 2, &project).await;
        let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
        assert_eq!(idle["stopReason"], "end_turn", "{idle}");
        child.kill().await.unwrap();
        ids.push(session.as_str().unwrap().to_owned());
    }
    assert_ne!(ids[0], ids[1]);
    // Claude saved the first session's transcript.
    write_transcript(temp.path(), &project, &ids[0]);

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":4,"method":"session/resume","params":{"sessionId":"not-a-session","cwd":project,"mcpServers":[]}})).await;
    let (unknown, _) = response(&mut reader, 4).await;
    assert_eq!(unknown["error"]["code"], -32002);
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/resume","params":{"sessionId":ids[0],"cwd":project,"mcpServers":[]}})).await;
    let (resumed, _) = response(&mut reader, 2).await;
    assert!(resumed.get("result").is_some(), "{resumed}");
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":ids[0],"prompt":[{"type":"text","text":"continue"}]}})).await;
    finished(&mut reader, 3).await;
    let calls = calls(&project);
    assert_eq!(calls.len(), 3);
    assert!(
        calls[0].contains(&format!("--session-id {}", ids[0])),
        "{calls:?}"
    );
    assert!(
        calls[1].contains(&format!("--session-id {}", ids[1])),
        "{calls:?}"
    );
    assert!(
        calls[2].contains(&format!("--resume {}", ids[0])),
        "{calls:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn failed_resume_fails_the_next_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // A claude that cannot resume reports why and exits without reading
    // the prompt.
    fake_claude_with_init(
        &bin,
        r#"
printf '%s\n' "$*" >> calls
case " $* " in
  *' --resume '*)
    printf '%s\n' '{"type":"result","is_error":true,"result":"resume failed"}'
    exit 1;;
esac
next_message
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{"commands":[]}}}'
"#,
        r#"
next_prompt
printf '%s\n' '{"type":"result","session_id":"79ed6f89-b693-4192-8169-4554404b43a2"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    child.kill().await.unwrap();
    write_transcript(temp.path(), &project, session.as_str().unwrap());

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/resume","params":{"sessionId":session,"cwd":project,"mcpServers":[]}})).await;
    let (resumed, _) = response(&mut reader, 2).await;
    assert!(resumed["error"].is_null(), "{resumed}");
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})).await;
    let (failed, updates) = response(&mut reader, 3).await;
    assert_eq!(failed["error"]["message"], "resume failed", "{failed}");
    // The prompt never entered the conversation, so no turn is reported.
    assert!(
        !updates.iter().any(|v| is_state(v, "running")),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

const HISTORY_ID: &str = "79ed6f89-b693-4192-8169-4554404b43a2";

fn dirs(temp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let bin = temp.join("bin");
    let project = temp.join("project");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&project).unwrap();
    (bin, project.canonicalize().unwrap())
}

fn write_transcript(temp: &Path, project: &Path, id: &str) {
    let transcripts = temp.join("empty-claude/projects/sample");
    fs::create_dir_all(&transcripts).unwrap();
    fs::write(
        transcripts.join(format!("{id}.jsonl")),
        format!(
            "{{\"cwd\":\"{}\",\"type\":\"user\",\"message\":{{\"content\":\"Transcript prompt\"}}}}\n",
            project.display()
        ),
    )
    .unwrap();
}

/// The arguments of each claude the bridge started in `project`, where a
/// fake claude records them with `printf '%s\n' "$*" >> calls`.
fn calls(project: &Path) -> Vec<String> {
    fs::read_to_string(project.join("calls"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

async fn new_session(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    request_id: i64,
    project: &Path,
) -> Value {
    send(child, json!({"jsonrpc":"2.0","id":request_id,"method":"session/new","params":{"cwd":project,"mcpServers":[]}})).await;
    let (new, _) = response(reader, request_id).await;
    let session = new["result"]["sessionId"].clone();
    // The new session reports itself idle, with nothing running.
    let snapshot = receive(reader).await;
    assert!(is_state(&snapshot, "idle"), "{snapshot}");
    assert_eq!(update(&snapshot)["background"], json!([]), "{snapshot}");
    assert_eq!(snapshot["params"]["sessionId"], session, "{snapshot}");
    // Its claude starts next and reports its commands.
    let commands = receive(reader).await;
    assert_eq!(
        update(&commands)["sessionUpdate"],
        "available_commands_update",
        "{commands}"
    );
    session
}

async fn prompt(
    child: &mut Child,
    reader: &mut BufReader<ChildStdout>,
    request_id: i64,
    session: &Value,
) -> (Value, Vec<Value>) {
    send(child, json!({"jsonrpc":"2.0","id":request_id,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})).await;
    finished(reader, request_id).await
}

/// Reads messages until one has `text` as its update content.
async fn until_text(reader: &mut BufReader<ChildStdout>, text: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let value = receive(reader).await;
        let done = shown_text(&value) == Some(text);
        seen.push(value);
        if done {
            return seen;
        }
    }
}

/// Reads messages after `seen` until one of them all has `text` as its
/// update content, and returns them all.
async fn and_until_text(
    mut seen: Vec<Value>,
    reader: &mut BufReader<ChildStdout>,
    text: &str,
) -> Vec<Value> {
    if !seen.iter().any(|value| shown_text(value) == Some(text)) {
        seen.extend(until_text(reader, text).await);
    }
    seen
}

/// The message of the error an idle update ends its turn with.
fn error_message(idle: &Value) -> Option<&str> {
    (idle["stopReason"] == "error").then(|| idle["error"]["message"].as_str().unwrap())
}

#[tokio::test]
async fn cancel_sent_right_after_prompt_cancels_it() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r"
next_prompt
interrupted
next_message
",
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let prompt = json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}});
    let cancel = json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}});
    let stdin = child.stdin.as_mut().unwrap();
    stdin
        .write_all(format!("{prompt}\n{cancel}\n").as_bytes())
        .await
        .unwrap();
    let (result, _) = finished(&mut reader, 3).await;
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    child.kill().await.unwrap();
}

/// A prompt claude drops when it is interrupted, before it takes the prompt
/// into a turn, is answered as cancelled.
#[tokio::test]
async fn cancel_drops_a_prompt_claude_has_not_taken() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r"
next_message
interrupted
next_message
",
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let prompt = json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}});
    let cancel = json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}});
    let stdin = child.stdin.as_mut().unwrap();
    stdin
        .write_all(format!("{prompt}\n{cancel}\n").as_bytes())
        .await
        .unwrap();
    let (result, updates) = response(&mut reader, 3).await;
    assert_eq!(result["error"]["code"], -32800, "{result}");
    assert!(
        !updates
            .iter()
            .any(|v| update(v)["sessionUpdate"] == "user_message"),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

/// A claude that exits in its first turn, and one that answers after it.
fn exiting_claude(bin: &Path) {
    fake_claude(
        bin,
        r#"
printf '%s\n' "$*" >> calls
next_prompt
if [ "$(wc -l < calls)" -eq 1 ]; then exit 1; fi
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
}

/// Prompts an exiting claude twice, and returns the arguments of both
/// claudes. When `transcript`, claude saved the session's transcript before
/// it exited.
async fn fail_then_prompt_again(
    temp: &Path,
    bin: &Path,
    project: &Path,
    transcript: bool,
) -> Vec<String> {
    let (mut child, mut reader) = start(bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, project).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert!(error_message(&idle).is_some(), "{idle}");
    if transcript {
        write_transcript(temp, project, session.as_str().unwrap());
    }
    let (idle, _) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    child.kill().await.unwrap();
    calls(project)
}

#[tokio::test]
async fn a_claude_that_exits_is_replaced_by_one_that_resumes() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    exiting_claude(&bin);
    let calls = fail_then_prompt_again(temp.path(), &bin, &project, true).await;
    assert_eq!(calls.len(), 2);
    assert!(calls[1].contains(" --resume "), "{calls:?}");
}

#[tokio::test]
async fn a_claude_that_exits_without_a_transcript_is_replaced_by_a_fresh_one() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    exiting_claude(&bin);
    let calls = fail_then_prompt_again(temp.path(), &bin, &project, false).await;
    assert_eq!(calls.len(), 2);
    assert!(!calls[1].contains("--resume"), "{calls:?}");
}

#[tokio::test]
async fn max_turns_reports_its_stop_reason() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
printf '%s\n' '{{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"{HISTORY_ID}","errors":["Reached maximum number of turns (6)"]}}'
exit 1
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["stopReason"], "max_turn_requests", "{idle}");
    assert_eq!(error_message(&idle), None);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn error_results_report_their_errors() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["first","second"]}'
exit 1
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(error_message(&idle), Some("first; second"), "{idle}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn cancel_fails_tool_calls_that_did_not_finish() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"sleep 30"}}]}}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}'
interrupted
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})).await;
    let seen = until_text(&mut reader, "working").await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await;
    let (result, updates) = finish(&mut reader, 3, seen).await;
    assert_eq!(result["stopReason"], "cancelled", "{result}");
    assert!(
        updates.iter().any(|v| {
            update(v)["sessionUpdate"] == "tool_call_update"
                && update(v)["toolCallId"] == "tool-1"
                && update(v)["status"] == "failed"
        }),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn prompt_content_reaches_claude() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    let captured = temp.path().join("message.json");
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
printf '%s\n' "$message" > '{}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}","result":"done"}}'
"#,
            captured.display()
        ),
    );
    let (mut child, mut reader) = start(&bin);
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":2,"capabilities":{},"info":{"name":"test","version":"1"}}}),
    )
    .await;
    let (init, _) = response(&mut reader, 1).await;
    let capabilities = &init["result"]["capabilities"]["session"]["prompt"];
    assert!(capabilities["image"].is_object(), "{init}");
    assert!(capabilities["embeddedContext"].is_object(), "{init}");
    assert!(capabilities["audio"].is_null(), "{init}");

    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let prompt = json!([
        {"type": "text", "text": "Look at these"},
        {"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo="},
        {"type": "resource_link", "name": "main.rs", "uri": "file:///p/src/main.rs"},
        {"type": "resource", "resource": {"uri": "file:///p/notes.md", "text": "Notes body", "mimeType": "text/markdown"}},
        {"type": "resource", "resource": {"uri": "file:///p/spec.pdf", "blob": "JVBERi0=", "mimeType": "application/pdf"}},
        {"type": "resource", "resource": {"uri": "file:///p/shot.jpg", "blob": "/9j/4A==", "mimeType": "image/jpeg"}}
    ]);
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":prompt}})).await;
    let (result, _) = finished(&mut reader, 3).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");

    let message: Value = serde_json::from_str(&fs::read_to_string(&captured).unwrap()).unwrap();
    assert_eq!(
        message["message"]["content"],
        json!([
            {"type": "text", "text": "Look at these"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
            {"type": "text", "text": "[@main.rs](file:///p/src/main.rs)"},
            {"type": "text", "text": "<context ref=\"file:///p/notes.md\">\nNotes body\n</context>"},
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0="}},
            {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "/9j/4A=="}}
        ])
    );
    let _ = child.kill().await;
}

#[tokio::test]
async fn unsupported_prompt_content_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    let ran = temp.path().join("ran");
    fake_claude(
        &bin,
        &format!(
            r#"
while :; do
  next_message
  case "$message" in
    *'"type":"user"'*) printf '%s\n' "$message" >> '{}';;
    *'"type":"control_request"'*) accept;;
  esac
done
"#,
            ran.display()
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let prompts = [
        json!([{"type": "text", "text": "listen"}, {"type": "audio", "mimeType": "audio/wav", "data": "UklGRg=="}]),
        json!([{"type": "image", "mimeType": "image/bmp", "data": "Qk0="}]),
        json!([{"type": "resource", "resource": {"uri": "file:///p/a.zip", "blob": "UEs=", "mimeType": "application/zip"}}]),
        json!([{"type": "text", "text": "  "}]),
        json!([]),
    ];
    for (id, prompt) in (10_i64..).zip(prompts) {
        send(&mut child, json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":session,"prompt":prompt}})).await;
        let (result, _) = response(&mut reader, id).await;
        assert_eq!(result["error"]["code"], -32602, "{result}");
    }
    // The config acknowledgement follows anything the bridge sent before it.
    let configured = set_config(&mut child, &mut reader, 20, &session, "mode", "plan").await;
    assert_eq!(option(&configured, "mode")["currentValue"], "plan");
    assert!(!ran.exists(), "a rejected prompt reached claude");
    let _ = child.kill().await;
}

#[tokio::test]
async fn failed_exit_reports_claude_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // The background process keeps stdout and stderr open after claude exits,
    // until the test ends it.
    fake_claude(
        &bin,
        r#"
next_prompt
state running
echo "unknown option --bogus" >&2
tail -f /dev/null &
printf '%s\n' "$!" > holder
exit 1
"#,
    );
    let _holder = KillOnDrop(project.join("holder"));
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    let error = error_message(&idle).unwrap();
    assert!(error.ends_with(": unknown option --bogus"), "{error}");
    // A claude that exits while working is no longer working.
    let states: Vec<String> = describe_updates(&updates)
        .into_iter()
        .filter(|update| update.starts_with("state_update"))
        .collect();
    assert_eq!(
        states,
        [
            "state_update: running".to_owned(),
            format!("state_update: idle error {error}")
        ]
    );
    child.kill().await.unwrap();
}

/// Kills, when dropped, the process whose ID a fake claude wrote to the file.
struct KillOnDrop(PathBuf);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Ok(pid) = fs::read_to_string(&self.0) {
            let _ = std::process::Command::new("kill").arg(pid.trim()).status();
        }
    }
}

/// Writes a transcript that mixes conversation entries with entries the
/// replay skips: Claude's bookkeeping, a meta message and a sidechain.
fn write_conversation(temp: &Path, project: &Path, id: &str) {
    let transcripts = temp.join("empty-claude/projects/sample");
    fs::create_dir_all(&transcripts).unwrap();
    let entries = [
        json!({"type": "queue-operation", "operation": "enqueue"}),
        json!({"cwd": project, "type": "user", "message": {"role": "user", "content": "Fix the build"}}),
        json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "<system-reminder>"}}),
        json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "", "signature": "x"}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "thinking", "thinking": "Checking"}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Running it"}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "cargo build"}}]}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": "subagent"}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
        json!({"type": "attachment", "attachment": {}}),
        json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "text", "text": "Look"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}}
        ]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_3", "name": "Agent", "input": {"prompt": "Read /a"}}]}}),
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_3", "content": [
            {"type": "text", "text": "a says hi"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}}
        ]}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_2", "name": "Read", "input": {"file_path": "/x"}}]}}),
    ];
    write_lines(&transcripts.join(format!("{id}.jsonl")), &entries);
    // The subagent toolu_3 started keeps its own transcript.
    let subagents = transcripts.join(id).join("subagents");
    fs::create_dir_all(&subagents).unwrap();
    fs::write(
        subagents.join("agent-a1.meta.json"),
        json!({"agentType": "general-purpose", "toolUseId": "toolu_3"}).to_string(),
    )
    .unwrap();
    write_lines(
        &subagents.join("agent-a1.jsonl"),
        &[
            json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": "Read /a"}}),
            json!({"type": "assistant", "isSidechain": true, "message": {"content": [
                {"type": "text", "text": "subagent text"},
                {"type": "tool_use", "id": "toolu_s1", "name": "Read", "input": {"file_path": "/a"}}
            ]}}),
            json!({"type": "user", "isSidechain": true, "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_s1", "content": "hi"}]}}),
        ],
    );
}

fn write_lines(path: &Path, entries: &[Value]) {
    let lines: Vec<String> = entries.iter().map(Value::to_string).collect();
    fs::write(path, lines.join("\n") + "\n").unwrap();
}

/// The work a state update lists: a tool call's ID, or a custom item's type
/// and ID.
fn background_items(list: &Value) -> Vec<String> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|item| match item["type"].as_str().unwrap() {
            "tool_call" => item["toolCallId"].as_str().unwrap().to_owned(),
            kind => format!(
                "{kind}:{}",
                item["taskId"].as_str().or(item["id"].as_str()).unwrap()
            ),
        })
        .collect()
}

/// One line per update: its kind and what it shows.
fn describe_updates(notifications: &[Value]) -> Vec<String> {
    notifications
        .iter()
        .filter(|n| n["method"] == "session/update")
        .map(|n| {
            let update = update(n);
            let kind = update["sessionUpdate"].as_str().unwrap();
            let blocks = |content: &Value| -> String {
                content
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| match c["text"].as_str() {
                        Some(text) => text.to_owned(),
                        None => c["type"].as_str().unwrap().to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            let detail = match kind {
                "state_update" => {
                    let mut detail = update["state"].as_str().unwrap().to_owned();
                    if let Some(reason) = update["stopReason"].as_str() {
                        detail = format!("{detail} {reason}");
                    }
                    if let Some(error) = update["error"]["message"].as_str() {
                        detail = format!("{detail} {error}");
                    }
                    let background = background_items(&update["background"]);
                    if !background.is_empty() {
                        detail = format!("{detail} [{}]", background.join(" "));
                    }
                    detail
                }
                "tool_call_update" => {
                    let mut detail = update["toolCallId"].to_string();
                    if let Some(title) = update["title"].as_str() {
                        detail = format!("{detail} {title}");
                    }
                    if let Some(status) = update["status"].as_str() {
                        detail = format!("{detail} {status}");
                    }
                    let content: Vec<String> = update["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|c| match c["content"]["text"].as_str() {
                            Some(text) => text.to_owned(),
                            None => c["content"]["type"].as_str().unwrap().to_owned(),
                        })
                        .collect();
                    if !content.is_empty() {
                        detail = format!("{detail} [{}]", content.join(" | "));
                    }
                    detail
                }
                // Whole messages carry their blocks, and chunks one block.
                "user_message" | "agent_message" | "agent_thought" => blocks(&update["content"]),
                _ => match update["content"]["type"].as_str() {
                    Some("text") => update["content"]["text"].to_string(),
                    Some(other) => other.to_owned(),
                    None => String::new(),
                },
            };
            format!("{kind}: {}", detail.replace('"', ""))
        })
        .collect()
}

const REPLAY: [&str; 11] = [
    "user_message: Fix the build",
    "agent_thought: Checking",
    "agent_message: Running it",
    "tool_call_update: toolu_1 Bash `cargo build` pending",
    "tool_call_update: toolu_1 completed [ok]",
    "user_message: Look | image",
    "tool_call_update: toolu_3 Agent pending",
    "tool_call_update: toolu_3 [Read /a]",
    "tool_call_update: toolu_3 completed [Read /a | a says hi | image]",
    "tool_call_update: toolu_2 Read `/x` pending",
    "tool_call_update: toolu_2 failed",
];

/// `describe_updates` without the command updates claude's output sends.
fn without_commands(notifications: &[Value]) -> Vec<String> {
    describe_updates(notifications)
        .into_iter()
        .filter(|update| !update.starts_with("available_commands_update"))
        .collect()
}

/// The replay of `write_conversation`, then the idle state a resume sends.
fn replay_and_state() -> Vec<String> {
    REPLAY
        .iter()
        .copied()
        .chain(["state_update: idle"])
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn resume_replays_a_claude_session_from_its_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
printf '%s\n' "$*" > args
next_prompt
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    prompt(&mut child, &mut reader, 3, &session).await;
    child.kill().await.unwrap();
    // Claude saved the session's transcript.
    write_conversation(temp.path(), &project, session.as_str().unwrap());

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    // A cursor other than the start is not supported.
    send(&mut child, json!({"jsonrpc":"2.0","id":5,"method":"session/resume","params":{"sessionId":session,"cwd":project,"mcpServers":[],"replayFrom":{"type":"cursor","cursor":"x"}}})).await;
    let (refused, notifications) = response(&mut reader, 5).await;
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    assert!(notifications.is_empty(), "{notifications:?}");

    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/resume","params":{"sessionId":session,"cwd":project,"mcpServers":[],"replayFrom":{"type":"start"}}})).await;
    let (loaded, notifications) = response(&mut reader, 2).await;
    assert!(loaded["result"].is_object(), "{loaded}");
    // The replay comes first, then the session's state, then the response.
    // Claude reports its commands from its own output, unordered with them.
    assert!(
        notifications
            .iter()
            .all(|n| n["params"]["sessionId"] == session),
        "{notifications:?}"
    );
    assert_eq!(without_commands(&notifications), replay_and_state());
    let (result, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    let args = fs::read_to_string(project.join("args")).unwrap();
    assert!(
        args.contains(&format!("--resume {}", session.as_str().unwrap())),
        "{args}"
    );
    child.kill().await.unwrap();

    // Without `replayFrom`, nothing replays.
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":4,"method":"session/resume","params":{"sessionId":session,"cwd":project,"mcpServers":[]}})).await;
    let (resumed, notifications) = response(&mut reader, 4).await;
    assert!(resumed["result"].is_object(), "{resumed}");
    assert_eq!(without_commands(&notifications), ["state_update: idle"]);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_session_keeps_one_claude_across_prompts() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
printf '%s\n' "$*" >> calls
turn=0
while :; do
  next_prompt
  turn=$((turn + 1))
  printf '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"turn %s"}}]}}}}\n' "$turn"
  printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
done
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    for (request_id, text) in [(3, "turn 1"), (4, "turn 2"), (5, "turn 3")] {
        let (result, updates) = prompt(&mut child, &mut reader, request_id, &session).await;
        assert_eq!(result["stopReason"], "end_turn", "{result}");
        assert!(
            updates.iter().any(|v| shown_text(v) == Some(text)),
            "missing {text} in {updates:?}"
        );
    }
    let calls = calls(&project);
    assert_eq!(calls.len(), 1, "{calls:?}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn turns_claude_starts_reach_the_client() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // After the prompt's turn, claude runs two turns of its own, as it does
    // when a background task reports back. The second one fails.
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
state running
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
state running
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"tool-1","name":"Bash","input":{{"command":"pwd"}}}}]}}}}'
state requires_action
state running
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"tool-1","content":""}}]}}}}'
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"woke"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
state running
printf '%s\n' '{{"type":"result","is_error":true,"result":"wake failed"}}'
state idle
state running
printf '%s\n' '{{"type":"result","subtype":"error_max_turns","is_error":true}}'
state idle
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"end"}}]}}}}'
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (result, before) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    let after = until_text(&mut reader, "end").await;
    let describe = |messages: &[Value]| -> Vec<String> {
        describe_updates(messages)
            .into_iter()
            .filter(|update| !update.starts_with("session_info_update"))
            .collect()
    };
    // The client has the prompt's response before what claude does next.
    assert_eq!(
        describe(&before),
        [
            "user_message: hello",
            "state_update: running",
            "state_update: idle end_turn"
        ]
    );
    assert_eq!(
        describe(&after),
        [
            "state_update: running",
            "tool_call_update: tool-1 Bash `pwd` pending",
            "tool_call_update: tool-1 completed",
            "agent_message: woke",
            "state_update: idle end_turn",
            "state_update: running",
            "state_update: idle error wake failed",
            "state_update: running",
            "state_update: idle max_turn_requests",
            "agent_message: end",
        ]
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_prompt_sent_during_a_turn_claude_started_ends_with_its_own_result() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // Claude reads the second prompt while a turn of its own runs, ends that
    // turn, and only then starts the prompt's.
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"waking"}}]}}}}'
next_message
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
started
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"answer"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (_, seen) = prompt(&mut child, &mut reader, 3, &session).await;
    and_until_text(seen, &mut reader, "waking").await;
    let (result, updates) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert!(
        updates.iter().any(|v| shown_text(v) == Some("answer")),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

/// The process group of `pid`.
fn process_group(pid: &str) -> String {
    let output = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", pid])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
async fn claude_runs_in_the_bridges_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
printf '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"%s"}}]}}}}\n' "$$"
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (_, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    let claude = updates
        .iter()
        .filter(|v| update(v)["sessionUpdate"] == "agent_message")
        .find_map(shown_text)
        .unwrap();
    let bridge = child.id().unwrap().to_string();
    assert_eq!(process_group(claude), process_group(&bridge));
    child.kill().await.unwrap();
}

#[tokio::test]
async fn resuming_with_mcp_servers_starts_a_claude_that_has_them() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
printf '%s\n' "$*" >> calls
while :; do
  next_prompt
  printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
done
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    prompt(&mut child, &mut reader, 3, &session).await;
    child.kill().await.unwrap();
    write_transcript(temp.path(), &project, session.as_str().unwrap());

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":4,"method":"session/resume","params":{
            "sessionId":session,"cwd":project,
            "mcpServers":[{"type":"stdio","name":"filesystem","command":"/usr/bin/node","args":["server.js"],"env":[]}]
        }}),
    )
    .await;
    response(&mut reader, 4).await;
    let (result, _) = prompt(&mut child, &mut reader, 5, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    let calls = calls(&project);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(!calls[0].contains("--mcp-config"), "{calls:?}");
    assert!(
        calls[1].contains("--mcp-config")
            && calls[1].contains(&format!("--resume {}", session.as_str().unwrap())),
        "{calls:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn tool_output_reaches_the_client() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Bash","input":{"command":"ls"}},{"type":"tool_use","id":"tool-2","name":"Read","input":{"file_path":"/a.png"}}]}}'
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-1","content":"a.png\nb.rs","is_error":true}]}}'
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool-2","content":[{"type":"text","text":"Read an image"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}},{"type":"tool_reference","tool_name":"WebSearch"}]}]}}'
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (result, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert_eq!(
        describe_updates(&updates)
            .into_iter()
            .filter(|update| !update.starts_with("session_info_update"))
            .collect::<Vec<_>>(),
        [
            "user_message: hello",
            "state_update: running",
            "tool_call_update: tool-1 Bash `ls` pending",
            "tool_call_update: tool-2 Read `/a.png` pending",
            "tool_call_update: tool-1 failed [a.png\nb.rs]",
            "tool_call_update: tool-2 completed [Read an image | image | {type:tool_reference,tool_name:WebSearch}]",
            "state_update: idle end_turn",
        ]
    );
    let image = updates
        .iter()
        .find_map(|n| n["params"]["update"]["content"].as_array()?.get(1).cloned())
        .unwrap();
    assert_eq!(
        image["content"],
        json!({"type": "image", "mimeType": "image/png", "data": "iVBORw0KGgo="})
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_subagents_activity_shows_under_its_tool_call() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // The subagent's lines name the Agent tool call that started it. Its
    // Bash call asks permission, so the client has that call.
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"agent-1","name":"Agent","input":{"prompt":"Look"}}]}}'
printf '%s\n' '{"type":"user","parent_tool_use_id":"agent-1","message":{"role":"user","content":[{"type":"text","text":"Look"}]}}'
printf '%s\n' '{"type":"stream_event","parent_tool_use_id":"agent-1","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"sub streamed"}}}'
printf '%s\n' '{"type":"stream_event","parent_tool_use_id":"agent-1","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"sub thinking"}}}'
printf '%s\n' '{"type":"assistant","parent_tool_use_id":"agent-1","message":{"content":[{"type":"thinking","thinking":"sub thinking"},{"type":"text","text":"sub text"},{"type":"tool_use","id":"sub-1","name":"Read","input":{"file_path":"/a"}}]}}'
printf '%s\n' '{"type":"user","parent_tool_use_id":"agent-1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"sub-1","content":"a"}]}}'
printf '%s\n' '{"type":"assistant","parent_tool_use_id":"agent-1","message":{"content":[{"type":"tool_use","id":"sub-2","name":"Bash","input":{"command":"pwd"}}]}}'
printf '%s\n' '{"type":"control_request","request_id":"perm-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"pwd"},"tool_use_id":"sub-2"}}'
next_message
printf '%s\n' '{"type":"user","parent_tool_use_id":"agent-1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"sub-2","content":"/"}]}}'
printf '%s\n' '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"agent-1","content":[{"type":"text","text":"a says hi"}]}]}}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}'
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})).await;
    let (permission, seen) = until_permission(&mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":permission["id"],"result":{"outcome":{"outcome":"selected","optionId":"allow_once"}}})).await;
    let (result, updates) = finish(&mut reader, 3, seen).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert_eq!(
        describe_updates(&updates)
            .into_iter()
            .filter(|update| !update.starts_with("session_info_update"))
            .collect::<Vec<_>>(),
        [
            "user_message: hello",
            "state_update: running",
            "tool_call_update: agent-1 Agent pending",
            "tool_call_update: agent-1 [Read /a]",
            "tool_call_update: agent-1 [Read /a | Bash pwd]",
            "tool_call_update: sub-2 Bash `pwd` pending",
            "state_update: requires_action",
            "tool_call_update: sub-2 in_progress",
            "state_update: running",
            "tool_call_update: sub-2 completed [/]",
            "tool_call_update: agent-1 completed [Read /a | Bash pwd | a says hi]",
            "agent_message: done",
            "state_update: idle end_turn",
        ]
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn claudes_echo_of_a_prompt_starts_its_turn() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // Claude says it took the prompt only by echoing it, here while the
    // turn streams, as it does for a prompt it folds into a running turn.
    fake_claude(
        &bin,
        r#"
next_message
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello "}}}'
echoed
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}'
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (result, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(result["stopReason"], "end_turn", "{result}");
    assert_eq!(
        describe_updates(&updates)
            .into_iter()
            .filter(|update| !update.starts_with("session_info_update"))
            .collect::<Vec<_>>(),
        [
            "agent_message_chunk: hello ",
            "user_message: hello",
            "state_update: running",
            "state_update: idle end_turn"
        ]
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_prompt_is_answered_when_it_enters_the_conversation() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}'
interrupted
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":session,"prompt":[{"type":"text","text":"hello"}]}})).await;
    // Only the prompt's own message comes before its response; the turn's
    // state and output follow it. The session's title is sent with the
    // prompt, apart from the turn, so it may arrive on either side.
    let (answer, before) = response(&mut reader, 3).await;
    let is_title = |v: &Value| update(v)["sessionUpdate"] == "session_info_update";
    let message: Vec<&Value> = before.iter().filter(|v| !is_title(v)).collect();
    assert_eq!(message.len(), 1, "{before:?}");
    assert_eq!(update(message[0])["sessionUpdate"], "user_message");
    assert_eq!(
        update(message[0])["messageId"],
        answer["result"]["messageId"]
    );
    assert_eq!(update(message[0])["content"][0]["text"], "hello");
    let mut after = until_text(&mut reader, "working").await;
    while !before.iter().chain(&after).any(is_title) {
        after.push(receive(&mut reader).await);
    }
    let title = before.iter().chain(&after).find(|v| is_title(v)).unwrap();
    assert_eq!(update(title)["title"], "hello", "{title}");
    let turn: Vec<Value> = after.iter().filter(|v| !is_title(v)).cloned().collect();
    assert_eq!(describe_updates(&turn[..1]), ["state_update: running"]);

    send(
        &mut child,
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await;
    let (idle, _) = finish(&mut reader, 3, [before, vec![answer], after].concat()).await;
    assert_eq!(idle["stopReason"], "cancelled", "{idle}");
    child.kill().await.unwrap();
}

/// Sends the prompt `text` as request `id`.
async fn send_prompt(child: &mut Child, id: i64, session: &Value, text: &str) {
    send(
        child,
        json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{
            "sessionId":session,"prompt":[{"type":"text","text":text}]
        }}),
    )
    .await;
}

/// The idle updates among `notifications`.
fn idles(notifications: &[Value]) -> Vec<&Value> {
    notifications
        .iter()
        .filter(|v| is_state(v, "idle"))
        .map(update)
        .collect()
}

#[tokio::test]
async fn a_prompt_sent_during_a_turn_steers_it() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // Claude folds the second prompt into the running turn and echoes it.
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' "$message" >> prompts
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}'
next_message
printf '%s\n' "$message" >> prompts
echoed
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"steered"}]}}'
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send_prompt(&mut child, 3, &session, "first").await;
    let (answer, seen) = response(&mut reader, 3).await;
    assert!(answer["result"].is_object(), "{answer}");
    let seen = and_until_text(seen, &mut reader, "working").await;
    send_prompt(&mut child, 4, &session, "steer").await;
    let (idle, updates) = finish(&mut reader, 4, seen).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    assert_eq!(idles(&updates).len(), 1, "{:?}", describe_updates(&updates));
    // Claude got both prompts before the turn ended, each marked for its
    // next opportunity.
    let prompts: Vec<Value> = fs::read_to_string(project.join("prompts"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    for prompt in &prompts {
        assert_eq!(prompt["type"], "user", "{prompt}");
        assert_eq!(prompt["priority"], "next", "{prompt}");
    }
    assert_eq!(prompts[1]["message"]["content"][0]["text"], "steer");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_preempted_turn_continues_with_the_prompt_that_preempted_it() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    // The second prompt preempts the turn before it shows output: claude
    // ends that turn as aborted and runs the prompt's turn.
    fake_claude(
        &bin,
        r#"
next_prompt
next_message
uuid=$(printf '%s\n' "$message" | sed 's/.*"uuid":"\([^"]*\)".*/\1/')
printf '{"type":"system","subtype":"turn_preempted","preempted_by_uuid":"%s"}\n' "$uuid"
printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_streaming"}'
started
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"answer"}]}}'
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send_prompt(&mut child, 3, &session, "first").await;
    let (answer, seen) = response(&mut reader, 3).await;
    assert!(answer["result"].is_object(), "{answer}");
    send_prompt(&mut child, 4, &session, "follow-up").await;
    let (idle, updates) = finish(&mut reader, 4, seen).await;
    assert_eq!(idle["stopReason"], "end_turn", "{idle}");
    assert_eq!(idles(&updates).len(), 1, "{:?}", describe_updates(&updates));
    assert!(
        updates.iter().any(|v| shown_text(v) == Some("answer")),
        "{updates:?}"
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn mcp_commands_are_absolute_paths() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(&bin, "exit 0");
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
        "cwd":project,"mcpServers":[{"type":"stdio","name":"x","command":"node","args":[],"env":[]}]
    }})).await;
    let (refused, _) = response(&mut reader, 2).await;
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    child.kill().await.unwrap();
}

#[tokio::test]
async fn replay_keeps_the_inserted_user_message_id() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
printf '%s\n' "$message" > captured.json
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (_, live) = prompt(&mut child, &mut reader, 3, &session).await;
    let original = live
        .iter()
        .find(|n| update(n)["sessionUpdate"] == "user_message")
        .unwrap();
    let original_id = update(original)["messageId"].clone();
    let mut retained: Value =
        serde_json::from_slice(&fs::read(project.join("captured.json")).unwrap()).unwrap();
    assert_eq!(retained["uuid"], original_id);
    retained["cwd"] = json!(project);
    child.kill().await.unwrap();
    let transcripts = temp.path().join("empty-claude/projects/sample");
    fs::create_dir_all(&transcripts).unwrap();
    write_lines(
        &transcripts.join(format!("{}.jsonl", session.as_str().unwrap())),
        &[retained],
    );

    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":4,"method":"session/resume","params":{
            "sessionId":session,"cwd":project,"mcpServers":[],"replayFrom":{"type":"start"}
        }}),
    )
    .await;
    let (resumed, replay) = response(&mut reader, 4).await;
    child.kill().await.unwrap();
    assert!(resumed["result"].is_object(), "{resumed}");
    let replayed = replay
        .iter()
        .find(|n| update(n)["sessionUpdate"] == "user_message")
        .unwrap();
    assert_eq!(
        update(replayed)["messageId"],
        original_id,
        "the replayed prompt keeps its message ID"
    );
}

#[tokio::test]
async fn unsupported_mcp_transport_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(&bin, "exit 0");
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
        "cwd":project,"mcpServers":[{"type":"http","name":"context","url":"https://example.com/mcp","headers":[]}]
    }})).await;
    let (answer, _) = response(&mut reader, 2).await;
    child.kill().await.unwrap();
    assert_eq!(
        answer["error"]["code"], -32602,
        "unsupported MCP server silently accepted: {answer}"
    );
}

#[tokio::test]
async fn a_queued_prompt_does_not_hide_a_running_turns_error() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"first turn running"}]}}'
next_message
printf '%s\n' '{"type":"result","is_error":true,"result":"first turn failed"}'
state idle
started
printf '%s\n' '{"type":"result"}'
next_message
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{
            "sessionId":session,"prompt":[{"type":"text","text":"first"}]
        }}),
    )
    .await;
    let before = until_text(&mut reader, "first turn running").await;
    assert!(before.iter().any(|v| is_state(v, "running")), "{before:?}");
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{
            "sessionId":session,"prompt":[{"type":"text","text":"second"}]
        }}),
    )
    .await;
    let (_, after) = finished(&mut reader, 4).await;
    child.kill().await.unwrap();
    assert!(
        after
            .iter()
            .any(|v| is_state(v, "idle") && error_message(update(v)) == Some("first turn failed")),
        "running turn's error disappeared: {:?}",
        describe_updates(&after)
    );
}

#[tokio::test]
async fn cancelled_work_that_exits_reports_cancelled() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        r#"
next_prompt
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]}}'
next_message
case "$message" in *'"subtype":"interrupt"'*) ;; *) exit 21;; esac
echo 'aborting the turn failed' >&2
exit 1
"#,
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{
            "sessionId":session,"prompt":[{"type":"text","text":"hello"}]
        }}),
    )
    .await;
    let before = until_text(&mut reader, "working").await;
    send(
        &mut child,
        json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":session}}),
    )
    .await;
    let (idle, _) = finish(&mut reader, 3, before).await;
    child.kill().await.unwrap();
    assert_eq!(
        idle["stopReason"], "cancelled",
        "cancellation was classified as a failure: {idle}"
    );
}

/// A fake claude that starts the background Bash `bg-1` as task `t1`, as
/// claude does, with `rest` after the turn it started in.
fn background_task(rest: &str) -> String {
    format!(
        r#"
next_prompt
state running
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"bg-1","name":"Bash","input":{{"command":"sleep 5","run_in_background":true}}}}]}}}}'
printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"t1"}}]}}'
printf '%s\n' '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"bg-1","is_backgrounded":true}}'
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"bg-1","content":"started"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
{rest}
"#
    )
}

#[tokio::test]
async fn a_background_task_keeps_its_tool_call_open_until_it_ends() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &background_task(&format!(
            r#"
printf '%s\n' '{{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"bg-1","status":"completed","summary":"slept"}}'
printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[]}}'
state running
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"woke"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
next_message
"#
        )),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, before) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(
        idle["background"],
        json!([{"type": "tool_call", "toolCallId": "bg-1"}])
    );
    // The result does not finish the tool call, and the turn's end does not
    // fail it.
    let describe = |messages: &[Value]| -> Vec<String> {
        describe_updates(messages)
            .into_iter()
            .filter(|update| !update.starts_with("session_info_update"))
            .collect()
    };
    assert_eq!(
        describe(&before),
        [
            "user_message: hello",
            "state_update: running",
            "tool_call_update: bg-1 Bash `sleep 5` pending",
            "state_update: running [_claude_task:t1]",
            "state_update: running [bg-1]",
            "tool_call_update: bg-1 in_progress [started]",
            "state_update: idle end_turn [bg-1]",
        ]
    );
    // The task ends: its tool call completes before the list drops it, and
    // the turn claude starts afterwards is reported.
    let after = until_text(&mut reader, "woke").await;
    let mut after = and_until_idle(&mut reader, after).await;
    after.retain(|value| value["method"] == "session/update");
    assert_eq!(
        describe(&after),
        [
            "tool_call_update: bg-1 completed [started | slept]",
            "state_update: idle",
            "state_update: running",
            "agent_message: woke",
            "state_update: idle end_turn",
        ]
    );
    child.kill().await.unwrap();
}

/// Reads until an idle state update follows what was `seen`.
async fn and_until_idle(reader: &mut BufReader<ChildStdout>, mut seen: Vec<Value>) -> Vec<Value> {
    while !seen.last().is_some_and(|value| is_state(value, "idle")) {
        seen.push(receive(reader).await);
    }
    seen
}

#[tokio::test]
async fn claude_exiting_fails_the_tool_calls_of_its_background_work() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(&bin, &background_task("exit 0"));
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(
        idle["background"],
        json!([{"type": "tool_call", "toolCallId": "bg-1"}])
    );
    let mut seen = Vec::new();
    while !seen.last().is_some_and(|value| is_state(value, "idle")) {
        seen.push(receive(&mut reader).await);
    }
    assert_eq!(
        describe_updates(&seen),
        ["tool_call_update: bg-1 failed", "state_update: idle"]
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_foreground_task_lists_nothing_and_a_background_one_is_listed() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
state running
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"fg-1","name":"Agent","input":{{}}}}]}}}}'
printf '%s\n' '{{"type":"system","subtype":"task_started","task_id":"t0","tool_use_id":"fg-1","is_backgrounded":false}}'
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"fg-1","content":"ok"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
next_prompt
state running
printf '%s\n' '{{"type":"system","subtype":"task_started","task_id":"t1","tool_use_id":"unknown-call","is_backgrounded":true}}'
printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"t1"}}]}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, updates) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(idle["background"], json!([]));
    assert!(
        updates
            .iter()
            .any(|v| update(v)["toolCallId"] == "fg-1" && update(v)["status"] == "completed"),
        "{updates:?}"
    );
    let (idle, _) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(
        idle["background"],
        json!([{"type": "_claude_task", "taskId": "t1"}])
    );
    child.kill().await.unwrap();
}

/// A turn of a fake claude that makes the scheduling call `name` with
/// `input`, and gets `output` as its result.
fn scheduling_turn(id: &str, name: &str, input: &str, output: &str) -> String {
    format!(
        r#"
next_prompt
state running
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}'
printf '%s\n' '{{"type":"user",{output}"message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"ok"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
"#
    )
}

#[tokio::test]
async fn scheduled_prompts_keep_the_session_listed() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    let far = 99_999_999_999_999_u64;
    let turns = [
        // A recurring job lasts until it is deleted.
        scheduling_turn(
            "cron-1",
            "CronCreate",
            "{}",
            &format!(r#""tool_use_result":{{"id":"job1","recurring":true,"nextFireAt":{far}}},"#),
        ),
        scheduling_turn(
            "cron-2",
            "CronDelete",
            r#"{"id":"job1"}"#,
            r#""tool_use_result":{"id":"job1"},"#,
        ),
        // A one-shot job whose time has passed ends with the turn that made it.
        scheduling_turn(
            "cron-3",
            "CronCreate",
            "{}",
            r#""tool_use_result":{"id":"job2","recurring":false,"nextFireAt":1},"#,
        ),
        // A wakeup lasts until it is stopped.
        scheduling_turn(
            "wake-1",
            "ScheduleWakeup",
            "{}",
            &format!(r#""tool_use_result":{{"scheduledFor":{far}}},"#),
        ),
        scheduling_turn(
            "wake-2",
            "ScheduleWakeup",
            r#"{"stop":true}"#,
            r#""tool_use_result":{"stopped":true},"#,
        ),
    ]
    .concat();
    fake_claude(&bin, &turns);
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let mut turn = async |id: i64| {
        let (_, updates) = prompt(&mut child, &mut reader, id, &session).await;
        describe_updates(&updates)
            .into_iter()
            .filter(|update| {
                !update.starts_with("session_info_update") && !update.starts_with("user_message")
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        turn(3).await,
        [
            "state_update: running",
            "tool_call_update: cron-1 CronCreate pending",
            "state_update: running [cron-1]",
            "tool_call_update: cron-1 pending [ok]",
            "state_update: idle end_turn [cron-1]",
        ]
    );
    assert_eq!(
        turn(4).await,
        [
            "state_update: running [cron-1]",
            "tool_call_update: cron-2 CronDelete pending",
            "tool_call_update: cron-1 completed",
            "state_update: running",
            "tool_call_update: cron-2 completed [ok]",
            "state_update: idle end_turn",
        ]
    );
    assert_eq!(
        turn(5).await,
        [
            "state_update: running",
            "tool_call_update: cron-3 CronCreate pending",
            "state_update: running [cron-3]",
            "tool_call_update: cron-3 pending [ok]",
            "tool_call_update: cron-3 completed",
            "state_update: idle end_turn",
        ]
    );
    assert_eq!(
        turn(6).await,
        [
            "state_update: running",
            "tool_call_update: wake-1 ScheduleWakeup pending",
            "state_update: running [wake-1]",
            "tool_call_update: wake-1 pending [ok]",
            "state_update: idle end_turn [wake-1]",
        ]
    );
    assert_eq!(
        turn(7).await,
        [
            "state_update: running [wake-1]",
            "tool_call_update: wake-2 ScheduleWakeup pending",
            "tool_call_update: wake-1 completed",
            "state_update: running",
            "tool_call_update: wake-2 completed [ok]",
            "state_update: idle end_turn",
        ]
    );
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_failed_stop_leaves_the_wakeup_listed() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    let far = 99_999_999_999_999_u64;
    let failed = format!(
        r#"
next_prompt
state running
printf '%s\n' '{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"wake-2","name":"ScheduleWakeup","input":{{"stop":true}}}}]}}}}'
printf '%s\n' '{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"wake-2","is_error":true,"content":"failed"}}]}}}}'
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
next_message
"#
    );
    let turns = [
        scheduling_turn(
            "wake-1",
            "ScheduleWakeup",
            "{}",
            &format!(r#""tool_use_result":{{"scheduledFor":{far}}},"#),
        ),
        failed,
    ]
    .concat();
    fake_claude(&bin, &turns);
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    let session = new_session(&mut child, &mut reader, 2, &project).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &session).await;
    assert_eq!(background_items(&idle["background"]), ["wake-1"]);
    let (idle, _) = prompt(&mut child, &mut reader, 4, &session).await;
    assert_eq!(background_items(&idle["background"]), ["wake-1"]);
    child.kill().await.unwrap();
}

#[tokio::test]
async fn a_resume_lists_the_scheduled_jobs_claude_restores() {
    let temp = tempfile::tempdir().unwrap();
    let (bin, project) = dirs(temp.path());
    let entry = |kind: &str, body: Value| {
        let mut entry = json!({
            "cwd": project, "timestamp": chrono::Utc::now().to_rfc3339(), "type": kind, "message": {"content": body},
        });
        entry["isSidechain"] = json!(false);
        entry
    };
    let far = 99_999_999_999_999_u64;
    let mut entries =
        vec![json!({"cwd": project, "type": "user", "message": {"content": "Schedule"}})];
    for (id, output) in [
        (
            "recurring",
            json!({"id": "keep", "recurring": true, "nextFireAt": far}),
        ),
        (
            "due",
            json!({"id": "due", "recurring": false, "nextFireAt": 1}),
        ),
        (
            "later",
            json!({"id": "later", "recurring": false, "nextFireAt": far}),
        ),
    ] {
        entries.push(entry(
            "assistant",
            json!([{"type": "tool_use", "id": id, "name": "CronCreate", "input": {}}]),
        ));
        let mut result = entry(
            "user",
            json!([{"type": "tool_result", "tool_use_id": id, "content": "ok"}]),
        );
        result["toolUseResult"] = output;
        entries.push(result);
    }
    entries.push(entry(
        "assistant",
        json!([{"type": "tool_use", "id": "w", "name": "ScheduleWakeup", "input": {}}]),
    ));
    let mut result = entry(
        "user",
        json!([{"type": "tool_result", "tool_use_id": "w", "content": "ok"}]),
    );
    result["toolUseResult"] = json!({"scheduledFor": far});
    entries.push(result);
    let transcripts = temp.path().join("empty-claude/projects/sample");
    fs::create_dir_all(&transcripts).unwrap();
    write_lines(&transcripts.join(format!("{HISTORY_ID}.jsonl")), &entries);
    fake_claude(
        &bin,
        &format!(
            r#"
next_prompt
state running
printf '%s\n' '{{"type":"result","session_id":"{HISTORY_ID}"}}'
state idle
next_message
"#
        ),
    );
    let (mut child, mut reader) = start(&bin);
    initialize(&mut child, &mut reader).await;
    send(&mut child, json!({"jsonrpc":"2.0","id":2,"method":"session/resume","params":{"sessionId":HISTORY_ID,"cwd":project,"mcpServers":[]}})).await;
    response(&mut reader, 2).await;
    let (idle, _) = prompt(&mut child, &mut reader, 3, &json!(HISTORY_ID)).await;
    assert_eq!(
        background_items(&idle["background"]),
        ["_claude_schedule:keep", "_claude_schedule:later"]
    );
    child.kill().await.unwrap();
}

/// Runs the bridge with `args` against the Claude config dir `claude`.
async fn read_sessions(claude: &Path, args: &[&std::ffi::OsStr]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_claude-code-acp"))
        .args(args)
        .env("CLAUDE_CONFIG_DIR", claude)
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(TIMEOUT, output)
        .await
        .expect("timed out reading sessions")
        .unwrap()
}

/// A transcript's first entry: a user prompt with its working directory,
/// entrypoint and time.
fn first_entry(cwd: &Path, entrypoint: &str, timestamp: &str, prompt: &str) -> Value {
    json!({"type": "user", "cwd": cwd, "entrypoint": entrypoint, "timestamp": timestamp,
        "message": {"role": "user", "content": prompt}})
}

#[tokio::test]
async fn sessions_lists_the_interactive_sessions_started_in_the_project() {
    let temp = tempfile::tempdir().unwrap();
    let (_, project) = dirs(temp.path());
    let sub = project.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let claude = temp.path().join("claude");
    let ids = [
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        "00000000-0000-4000-8000-000000000003",
        "00000000-0000-4000-8000-000000000004",
        "00000000-0000-4000-8000-000000000005",
    ];
    let started = [
        (&project, "cli", "2026-11-01T10:00:00.000Z"),
        (&project, "claude-desktop", "2026-11-02T10:00:00.000Z"),
        (&project, "sdk-cli", "2026-11-03T10:00:00.000Z"),
        (
            &temp.path().to_path_buf(),
            "cli",
            "2026-11-04T10:00:00.000Z",
        ),
        (&sub, "cli", "2026-11-05T10:00:00.000Z"),
    ];
    for (id, (cwd, entrypoint, timestamp)) in ids.iter().zip(started) {
        let transcripts = claude.join("projects").join(format!("p{}", &id[35..]));
        fs::create_dir_all(&transcripts).unwrap();
        write_lines(
            &transcripts.join(format!("{id}.jsonl")),
            &[
                json!({"type": "mode", "mode": "normal"}),
                first_entry(cwd, entrypoint, timestamp, "hi"),
            ],
        );
    }
    let output = read_sessions(&claude, &["--sessions".as_ref(), project.as_os_str()]).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        listed,
        json!([
            {"sessionId": ids[1], "startedAt": 1_793_613_600_000_i64},
            {"sessionId": ids[0], "startedAt": 1_793_527_200_000_i64},
        ])
    );
}

/// Runs the bridge's `--transcript` for session `id` in `project`.
async fn transcript_of(claude: &Path, project: &Path, id: &str) -> Value {
    let output = read_sessions(
        claude,
        &["--transcript".as_ref(), project.as_os_str(), id.as_ref()],
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn transcript_prints_the_title_and_the_updates_with_their_times() {
    let temp = tempfile::tempdir().unwrap();
    let (_, project) = dirs(temp.path());
    let claude = temp.path().join("claude");
    let transcripts = claude.join("projects/sample");
    fs::create_dir_all(&transcripts).unwrap();
    let at = |second: u32| format!("2026-11-01T10:00:{second:02}.000Z");
    let ms = |second: i64| 1_793_527_200_000_i64 + second * 1000;
    write_lines(
        &transcripts.join(format!("{HISTORY_ID}.jsonl")),
        &[
            first_entry(&project, "cli", &at(1), "Fix the build"),
            json!({"type": "custom-title", "customTitle": "Build fix"}),
            json!({"type": "ai-title", "aiTitle": "Fixing the build"}),
            json!({"type": "assistant", "timestamp": at(2), "message": {"content": [{"type": "text", "text": "Running it"}]}}),
            json!({"type": "assistant", "timestamp": at(3), "message": {"content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "cargo build"}}]}}),
            json!({"type": "user", "timestamp": at(4), "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "ok"}]}}),
            json!({"type": "ai-title", "aiTitle": "Build fixing"}),
            json!({"type": "assistant", "timestamp": at(5), "message": {"content": [{"type": "tool_use", "id": "toolu_2", "name": "Read", "input": {"file_path": "/x"}}]}}),
        ],
    );
    let transcript = transcript_of(&claude, &project, HISTORY_ID).await;
    assert_eq!(transcript["title"], "Build fix", "{transcript}");
    let updates = transcript["updates"].as_array().unwrap();
    let notifications: Vec<Value> = updates
        .iter()
        .map(|timed| json!({"method": "session/update", "params": {"update": timed["update"]}}))
        .collect();
    let times: Vec<i64> = updates
        .iter()
        .map(|timed| timed["at"].as_i64().unwrap())
        .collect();
    assert_eq!(
        describe_updates(&notifications),
        [
            "user_message: Fix the build",
            "agent_message: Running it",
            "tool_call_update: toolu_1 Bash `cargo build` pending",
            "tool_call_update: toolu_1 completed [ok]",
            "tool_call_update: toolu_2 Read `/x` pending",
            "tool_call_update: toolu_2 failed",
        ]
    );
    assert_eq!(times, [ms(1), ms(2), ms(3), ms(4), ms(5), ms(5)]);
}

#[tokio::test]
async fn transcript_titles_a_session_without_claudes_title_by_its_first_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let (_, project) = dirs(temp.path());
    let claude = temp.path().join("claude");
    let transcripts = claude.join("projects/sample");
    fs::create_dir_all(&transcripts).unwrap();
    write_lines(
        &transcripts.join(format!("{HISTORY_ID}.jsonl")),
        &[
            first_entry(
                &project,
                "cli",
                "2026-11-01T10:00:00.000Z",
                "Fix   the\nbuild",
            ),
            json!({"type": "user", "timestamp": "2026-11-01T10:00:01.000Z", "message": {"role": "user", "content": "Then test"}}),
        ],
    );
    let transcript = transcript_of(&claude, &project, HISTORY_ID).await;
    assert_eq!(transcript["title"], "Fix the build", "{transcript}");
}

#[tokio::test]
async fn transcript_of_an_unknown_session_fails() {
    let temp = tempfile::tempdir().unwrap();
    let (_, project) = dirs(temp.path());
    let output = read_sessions(
        &temp.path().join("claude"),
        &[
            "--transcript".as_ref(),
            project.as_os_str(),
            HISTORY_ID.as_ref(),
        ],
    )
    .await;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("not found"), "{stderr}");
    assert!(output.stdout.is_empty());
}
