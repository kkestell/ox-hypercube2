mod conversation;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt::Display;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ox_protocol::schema::{
    AvailableCommand, ContentBlock, ElicitationAction, SessionConfigOption, TextContent,
};
use ox_protocol::{
    Branches, Checkout, Client, ConfigSetting, ConfigValue, Deleted, DirEntry, Error, FileContent,
    Focus, Harness, ListingChange, RunInfo, SessionInfo, WorkflowInfo, WorkspaceInfo, code,
    content, default_socket_path,
};
use serde::{Deserialize, Serialize};
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::conversation::{Conversation, ConversationUpdate, coalesce};

/// A task that stops when dropped.
struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The app's connection to oxd and the tasks that forward its streams to
/// the webview.
#[derive(Default)]
struct App {
    client: tokio::sync::Mutex<Option<Arc<Client>>>,
    watch: Mutex<Option<Task>>,
    /// The conversation views and the directory and file watches, by the ID
    /// of the channel each sends on.
    streams: Mutex<HashMap<u32, Task>>,
    /// The session or run the window shows while it is in front, which the
    /// watch's task sends to oxd.
    focus: tokio::sync::watch::Sender<Option<Focus>>,
}

impl App {
    /// The open connection to oxd, connecting first, and starting oxd, if
    /// there is none.
    async fn client(&self) -> Result<Arc<Client>, String> {
        let mut client = self.client.lock().await;
        if let Some(client) = client.as_ref().filter(|client| !client.is_closed()) {
            return Ok(client.clone());
        }
        let connected = Arc::new(
            Client::connect_or_start(&default_socket_path())
                .await
                .map_err(message)?,
        );
        *client = Some(connected.clone());
        Ok(connected)
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WatchMessage {
    /// Changes to the listing. A watch's first lists every workspace and
    /// session.
    Changes {
        changes: Vec<ListingChange>,
    },
    /// The watch could not start.
    Error {
        message: String,
    },
    Disconnected,
}

/// Whether a directory of `path` holds an executable file named `name`.
fn on_path(path: &OsStr, name: &str) -> bool {
    std::env::split_paths(path).any(|directory| {
        std::fs::metadata(directory.join(name))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

/// The harnesses whose executables are on the `PATH` sessions get.
#[tauri::command]
fn agents() -> Vec<Harness> {
    let Some(path) = utf8_env().remove("PATH") else {
        return Vec::new();
    };
    Harness::ALL
        .into_iter()
        .filter(|harness| on_path(path.as_ref(), harness.executable()))
        .collect()
}

// `watch`, `view_session`, and the file and directory watch commands are
// synchronous, so they run on the main thread in the order the webview calls
// them, and the last call's task is the one that stays.

/// Sends the workspaces and sessions lists on `channel` each time either
/// changes, replacing the previous watch. A new watch means the webview
/// reloaded or reconnected, and in both it views its conversations and
/// watches its files and directories again once the watch connects, so this
/// ends their previous views and watches. Once connected, the watch's task
/// also sends oxd the current focus and then each change to it, so a new
/// connection has the window's focus.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn watch(handle: AppHandle, app: State<'_, App>, channel: Channel<WatchMessage>) {
    let task = tauri::async_runtime::spawn(async move {
        let app = handle.state::<App>();
        let watch = match app.client().await {
            Ok(client) => client
                .watch()
                .await
                .map(|watch| (client, watch))
                .map_err(message),
            Err(error) => Err(error),
        };
        let (client, mut watch) = match watch {
            Ok(watch) => watch,
            Err(message) => {
                let _ = channel.send(WatchMessage::Error { message });
                return;
            }
        };
        let mut focus = app.focus.subscribe();
        focus.mark_changed();
        loop {
            // A focus change goes to oxd before the listing changes that
            // follow it, so the webview sees what oxd did after the focus.
            tokio::select! {
                biased;
                Ok(()) = focus.changed() => {
                    let target = focus.borrow_and_update().clone();
                    // A refused focus, such as on a session just deleted,
                    // leaves the connection unfocused.
                    let _ = client.focus(target).await;
                }
                changes = watch.next() => {
                    let Ok(changes) = changes else {
                        let _ = channel.send(WatchMessage::Disconnected);
                        return;
                    };
                    if channel.send(WatchMessage::Changes { changes }).is_err() {
                        return;
                    }
                }
            }
        }
    });
    *app.watch.lock().unwrap() = Some(Task(task));
    app.streams.lock().unwrap().clear();
}

/// Sets the session or run the window shows in front of the user, or clears
/// it with `None` while the window is in the background. The watch's task
/// sends it to oxd.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn focus(app: State<'_, App>, focus: Option<Focus>) {
    app.focus.send_replace(focus);
}

/// Sends a session's conversation on `channel` from its first event, in
/// batches of the items from the events received together, until `unwatch`
/// ends it. The view ends when the session closes or the
/// connection drops, and a view that cannot connect or subscribe sends
/// nothing. The workspace watch reports the connection, and the webview
/// views the session again once the watch reconnects.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn view_session(
    handle: AppHandle,
    app: State<'_, App>,
    session_id: String,
    channel: Channel<Vec<ConversationUpdate>>,
) {
    let id = channel.id();
    let task = tauri::async_runtime::spawn(async move {
        let app = handle.state::<App>();
        let Ok(client) = app.client().await else {
            return;
        };
        let Ok(mut events) = client.subscribe(&session_id, 0).await else {
            return;
        };
        let mut conversation = Conversation::default();
        while let Ok(events) = events.next_batch().await {
            let items = coalesce(events.iter().flat_map(|event| conversation.apply(event)));
            if !items.is_empty() && channel.send(items).is_err() {
                return;
            }
        }
    });
    app.streams.lock().unwrap().insert(id, Task(task));
}

/// Sends the entries of the directory at `path` on `channel` each time they
/// change, until `unwatch` ends it. The watch ends when the
/// connection drops, and a watch that cannot connect sends nothing; the
/// webview watches again once the workspace watch reconnects.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn watch_directory(
    handle: AppHandle,
    app: State<'_, App>,
    path: PathBuf,
    channel: Channel<Vec<DirEntry>>,
) {
    let id = channel.id();
    let task = tauri::async_runtime::spawn(async move {
        let app = handle.state::<App>();
        let Ok(client) = app.client().await else {
            return;
        };
        let Ok(mut watch) = client.watch_directory(&path).await else {
            return;
        };
        while let Ok(entries) = watch.next().await {
            if channel.send(entries).is_err() {
                return;
            }
        }
    });
    app.streams.lock().unwrap().insert(id, Task(task));
}

/// Sends the contents of the file at `path` on `channel` each time they
/// change, until `unwatch` ends it. A watch that cannot connect sends the
/// error.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn watch_file(
    handle: AppHandle,
    app: State<'_, App>,
    path: PathBuf,
    channel: Channel<FileContent>,
) {
    let id = channel.id();
    let task = tauri::async_runtime::spawn(async move {
        let app = handle.state::<App>();
        let watch = match app.client().await {
            Ok(client) => client.watch_file(&path).await.map_err(message),
            Err(error) => Err(error),
        };
        let mut watch = match watch {
            Ok(watch) => watch,
            Err(message) => {
                let _ = channel.send(FileContent::Error { message });
                return;
            }
        };
        while let Ok(content) = watch.next().await {
            if channel.send(content).is_err() {
                return;
            }
        }
    });
    app.streams.lock().unwrap().insert(id, Task(task));
}

/// Ends the conversation view, directory watch or file watch that sends on
/// the channel `channel_id`.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn unwatch(app: State<'_, App>, channel_id: u32) {
    app.streams.lock().unwrap().remove(&channel_id);
}

#[tauri::command]
async fn add_workspace(app: State<'_, App>, path: PathBuf) -> Result<WorkspaceInfo, String> {
    app.client()
        .await?
        .add_workspace(path)
        .await
        .map_err(message)
}

#[tauri::command]
async fn remove_workspace(app: State<'_, App>, workspace_id: String) -> Result<(), String> {
    app.client()
        .await?
        .remove_workspace(&workspace_id)
        .await
        .map_err(message)
}

/// The workspace's branches.
#[tauri::command]
async fn workspace_branches(app: State<'_, App>, workspace_id: String) -> Result<Branches, String> {
    app.client()
        .await?
        .branches(&workspace_id, utf8_env())
        .await
        .map_err(message)
}

/// Fast-forwards the workspace's branch when both branches are still at the
/// commits used for the comparison.
#[tauri::command]
async fn move_branch(
    app: State<'_, App>,
    workspace_id: String,
    branch: String,
    to: String,
    expected: String,
    source_branch: String,
) -> Result<(), String> {
    app.client()
        .await?
        .fast_forward_branch(
            &workspace_id,
            &branch,
            &expected,
            &source_branch,
            &to,
            utf8_env(),
        )
        .await
        .map_err(message)
}

/// The bridge executable of `agent`, beside the app's.
fn bridge(agent: Harness) -> Result<PathBuf, String> {
    Ok(std::env::current_exe()
        .map_err(message)?
        .with_file_name(agent.bridge()))
}

/// The ACP prompt of a message's text and attached files.
fn prompt_blocks(text: String, attachments: &[PathBuf]) -> Result<Vec<ContentBlock>, String> {
    let mut prompt = Vec::new();
    if !text.is_empty() {
        prompt.push(ContentBlock::Text(TextContent::new(text)));
    }
    for path in attachments {
        prompt.push(content::file_link(path).map_err(message)?);
    }
    Ok(prompt)
}

/// Creates a session of `agent` whose first prompt is the message, with
/// its agent set to `config`.
#[tauri::command]
async fn new_session(
    app: State<'_, App>,
    agent: Harness,
    workspace_id: String,
    checkout: Checkout,
    text: String,
    attachments: Vec<PathBuf>,
    config: Vec<ConfigSetting>,
) -> Result<SessionInfo, String> {
    let command = vec![bridge(agent)?.to_string_lossy().into_owned()];
    let prompt = prompt_blocks(text, &attachments)?;
    app.client()
        .await?
        .new_session(
            &workspace_id,
            command,
            utf8_env(),
            checkout,
            config,
            Some(prompt),
        )
        .await
        .map_err(message)
}

/// The config options and slash commands an agent offers a new session.
#[derive(Serialize, Deserialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct Controls {
    config_options: Vec<SessionConfigOption>,
    available_commands: Vec<AvailableCommand>,
}

/// The controls `agent` offers a new session in `cwd`, which its bridge's
/// `--describe` mode reports. A bridge that fails gives its standard error.
#[tauri::command]
async fn agent_controls(agent: Harness, cwd: PathBuf) -> Result<Controls, String> {
    let output = tokio::process::Command::new(bridge(agent)?)
        .arg("--describe")
        .arg(&cwd)
        .env_clear()
        .envs(utf8_env())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(message)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(message)
}

/// Restarts the agent of an exited session, which replays its conversation.
#[tauri::command]
async fn load_session(app: State<'_, App>, session_id: String) -> Result<(), String> {
    app.client()
        .await?
        .load_session(&session_id, utf8_env())
        .await
        .map(drop)
        .map_err(message)
}

#[tauri::command]
async fn prompt(
    app: State<'_, App>,
    session_id: String,
    text: String,
    attachments: Vec<PathBuf>,
) -> Result<(), String> {
    let prompt = prompt_blocks(text, &attachments)?;
    app.client()
        .await?
        .prompt(&session_id, prompt)
        .await
        .map_err(message)
}

#[tauri::command]
async fn cancel(app: State<'_, App>, session_id: String) -> Result<(), String> {
    app.client()
        .await?
        .cancel(&session_id)
        .await
        .map_err(message)
}

#[tauri::command]
async fn permit(
    app: State<'_, App>,
    session_id: String,
    request_id: String,
    option_id: String,
) -> Result<(), String> {
    app.client()
        .await?
        .permit(&session_id, &request_id, &option_id)
        .await
        .map_err(message)
}

#[tauri::command]
async fn respond_elicitation(
    app: State<'_, App>,
    session_id: String,
    request_id: String,
    action: ElicitationAction,
) -> Result<(), String> {
    app.client()
        .await?
        .respond_elicitation(&session_id, &request_id, action)
        .await
        .map_err(message)
}

#[tauri::command]
async fn set_config_option(
    app: State<'_, App>,
    session_id: String,
    config_id: String,
    value: ConfigValue,
) -> Result<(), String> {
    app.client()
        .await?
        .set_config_option(&session_id, &config_id, value)
        .await
        .map(drop)
        .map_err(message)
}

/// The outcome of deleting a session or a run.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Deletion {
    Deleted {
        kept_branch: Option<String>,
    },
    /// The worktree has work the delete would lose; `message` lists it.
    UnsavedWork {
        message: String,
    },
}

/// Deletes a session, giving up its worktree's unsaved work if `discard`.
#[tauri::command]
async fn delete_session(
    app: State<'_, App>,
    session_id: String,
    discard: bool,
) -> Result<Deletion, String> {
    deletion(
        app.client()
            .await?
            .delete(&session_id, discard, utf8_env())
            .await,
    )
}

/// The definitions in oxd's workflows directory.
#[tauri::command]
async fn workflows(app: State<'_, App>) -> Result<Vec<WorkflowInfo>, String> {
    app.client().await?.list_workflows().await.map_err(message)
}

/// Starts a run of `workflow` in a new worktree of the workspace, with
/// `input` for its first step.
#[tauri::command]
async fn start_run(
    app: State<'_, App>,
    workspace_id: String,
    workflow: String,
    input: String,
) -> Result<RunInfo, String> {
    app.client()
        .await?
        .start_run(&workspace_id, &workflow, &input, utf8_env())
        .await
        .map_err(message)
}

#[tauri::command]
async fn cancel_run(app: State<'_, App>, run_id: String) -> Result<(), String> {
    app.client()
        .await?
        .cancel_run(&run_id)
        .await
        .map_err(message)
}

/// Deletes a run with its child sessions and worktree, giving up the
/// worktree's unsaved work if `discard`.
#[tauri::command]
async fn delete_run(
    app: State<'_, App>,
    run_id: String,
    discard: bool,
) -> Result<Deletion, String> {
    deletion(
        app.client()
            .await?
            .delete_run(&run_id, discard, utf8_env())
            .await,
    )
}

/// The outcome of archiving a session.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Archival {
    Archived,
    /// The session's worktree has work the archive would lose; `message`
    /// lists it.
    UnsavedWork {
        message: String,
    },
}

/// Archives a session, giving up its worktree's unsaved work if `discard`.
#[tauri::command]
async fn archive_session(
    app: State<'_, App>,
    session_id: String,
    discard: bool,
) -> Result<Archival, String> {
    archival(
        app.client()
            .await?
            .archive(&session_id, discard, utf8_env())
            .await,
    )
}

/// Unarchives a session.
#[tauri::command]
async fn unarchive_session(app: State<'_, App>, session_id: String) -> Result<(), String> {
    app.client()
        .await?
        .unarchive(&session_id, utf8_env())
        .await
        .map(drop)
        .map_err(message)
}

/// The outcome of a `session.archive` answer.
fn archival<T>(answer: Result<T, Error>) -> Result<Archival, String> {
    match answer {
        Ok(_) => Ok(Archival::Archived),
        Err(Error::Remote(error)) if error.code == code::UNSAVED_WORK => {
            Ok(Archival::UnsavedWork {
                message: error.message,
            })
        }
        Err(error) => Err(message(error)),
    }
}

/// The outcome of a `session.delete` or `run.delete` answer. A session or run
/// that is already gone counts as deleted.
fn deletion(answer: Result<Deleted, Error>) -> Result<Deletion, String> {
    match answer {
        Ok(Deleted { kept_branch }) => Ok(Deletion::Deleted { kept_branch }),
        Err(Error::Remote(error)) if error.code == code::UNSAVED_WORK => {
            Ok(Deletion::UnsavedWork {
                message: error.message,
            })
        }
        Err(Error::Remote(error)) if error.code == code::NOT_FOUND => {
            Ok(Deletion::Deleted { kept_branch: None })
        }
        Err(error) => Err(message(error)),
    }
}

fn message(error: impl Display) -> String {
    error.to_string()
}

/// The environment variables whose names and values are UTF-8.
fn utf8_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());
    #[cfg(feature = "webdriver")]
    let builder = builder.plugin(tauri_plugin_wdio_webdriver::init());
    builder
        .manage(App::default())
        .invoke_handler(tauri::generate_handler![
            agents,
            watch,
            focus,
            view_session,
            watch_directory,
            watch_file,
            unwatch,
            add_workspace,
            remove_workspace,
            workspace_branches,
            move_branch,
            new_session,
            agent_controls,
            load_session,
            prompt,
            cancel,
            permit,
            respond_elicitation,
            set_config_option,
            delete_session,
            archive_session,
            unarchive_session,
            workflows,
            start_run,
            cancel_run,
            delete_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use ox_protocol::ErrorBody;

    use super::*;

    fn remote<T>(code: i32, message: &str) -> Result<T, Error> {
        Err(Error::Remote(ErrorBody {
            code,
            message: message.into(),
        }))
    }

    #[test]
    fn finds_executable_harnesses_on_path() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let file = |directory: &tempfile::TempDir, name: &str, mode: u32| {
            let path = directory.path().join(name);
            std::fs::write(&path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        let claude = Harness::Claude.executable();
        let pi = Harness::Pi.executable();
        file(&first, claude, 0o755);
        file(&first, pi, 0o644);
        std::fs::create_dir(second.path().join(pi)).unwrap();
        file(&second, "codex", 0o755);
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();

        for (name, found) in [
            (claude, true),
            (pi, false),
            ("codex", true),
            ("gemini", false),
        ] {
            assert_eq!(on_path(&path, name), found, "{name}");
        }
    }

    #[test]
    fn archival_maps_the_answer() {
        assert_eq!(
            archival(remote::<()>(code::UNSAVED_WORK, "untracked a.txt")),
            Ok(Archival::UnsavedWork {
                message: "untracked a.txt".into()
            })
        );
        assert_eq!(
            archival(remote::<()>(code::NOT_FOUND, "no session s")),
            Err("no session s".into())
        );
    }

    #[test]
    fn deletion_maps_the_answer() {
        assert_eq!(
            deletion(Ok(Deleted {
                kept_branch: Some("ox/1".into())
            })),
            Ok(Deletion::Deleted {
                kept_branch: Some("ox/1".into())
            })
        );
        assert_eq!(
            deletion(remote(code::UNSAVED_WORK, "untracked a.txt")),
            Ok(Deletion::UnsavedWork {
                message: "untracked a.txt".into()
            })
        );
        assert_eq!(
            deletion(remote(code::NOT_FOUND, "no session s")),
            Ok(Deletion::Deleted { kept_branch: None })
        );
        assert_eq!(
            deletion(remote(code::GIT, "worktree is locked")),
            Err("worktree is locked".into())
        );
        assert_eq!(
            deletion(Err(Error::Closed)),
            Err("connection to oxd closed".into())
        );
    }
}
