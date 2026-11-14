mod conversation;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt::Display;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ox_protocol::schema::{ContentBlock, ElicitationAction, TextContent};
use ox_protocol::{
    Branches, Checkout, Client, ConfigValue, Deleted, DirEntry, Error, ListingChange, SessionInfo,
    WorkspaceInfo, code, content, default_socket_path,
};
use serde::{Deserialize, Serialize};
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use crate::conversation::{Conversation, Item, coalesce};

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
    view: Mutex<Option<Task>>,
    /// The directory watches, by the ID of the channel each sends on.
    directories: Mutex<HashMap<u32, Task>>,
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

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Agent {
    Claude,
    Pi,
}

impl Agent {
    const ALL: [Agent; 2] = [Agent::Claude, Agent::Pi];

    /// The name of the coding-agent CLI the agent's bridge drives.
    fn harness(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Pi => "pi",
        }
    }

    /// The name of the agent's ACP bridge, which ships beside the app's
    /// executable.
    fn bridge(self) -> &'static str {
        match self {
            Agent::Claude => "claude-code-acp",
            Agent::Pi => "pi-acp",
        }
    }
}

/// Whether a directory of `path` holds an executable file named `name`.
fn on_path(path: &OsStr, name: &str) -> bool {
    std::env::split_paths(path).any(|directory| {
        std::fs::metadata(directory.join(name))
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    })
}

/// The agents whose harnesses are on the `PATH` sessions get.
#[tauri::command]
fn agents() -> Vec<Agent> {
    let Some(path) = utf8_env().remove("PATH") else {
        return Vec::new();
    };
    Agent::ALL
        .into_iter()
        .filter(|agent| on_path(path.as_ref(), agent.harness()))
        .collect()
}

// `watch`, `view_session`, `watch_directory`, and `unwatch_directory` are
// synchronous, so they run on the main thread in the order the webview calls
// them, and the last call's task is the one that stays.

/// Sends the workspaces and sessions lists on `channel` each time either
/// changes, replacing the previous watch. A new watch means the webview
/// reloaded or reconnected, and in both it watches its directories again once
/// the watch connects, so this ends every directory watch.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn watch(handle: AppHandle, app: State<'_, App>, channel: Channel<WatchMessage>) {
    let task = tauri::async_runtime::spawn(async move {
        let app = handle.state::<App>();
        let watch = match app.client().await {
            Ok(client) => client.watch().await.map_err(message),
            Err(error) => Err(error),
        };
        let mut watch = match watch {
            Ok(watch) => watch,
            Err(message) => {
                let _ = channel.send(WatchMessage::Error { message });
                return;
            }
        };
        loop {
            let Ok(changes) = watch.next().await else {
                let _ = channel.send(WatchMessage::Disconnected);
                return;
            };
            if channel.send(WatchMessage::Changes { changes }).is_err() {
                return;
            }
        }
    });
    *app.watch.lock().unwrap() = Some(Task(task));
    app.directories.lock().unwrap().clear();
}

/// Sends a session's conversation on `channel` from its first event, in
/// batches of the items from the events received together, replacing the
/// previous view; with no session, ends the previous view. The
/// view ends when the session closes or the connection drops, and a view that
/// cannot connect or subscribe sends nothing. The workspace watch reports the
/// connection, and the webview views the session again once the watch
/// reconnects.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn view_session(
    handle: AppHandle,
    app: State<'_, App>,
    session_id: Option<String>,
    channel: Channel<Vec<Item>>,
) {
    let task = session_id.map(|session_id| {
        Task(tauri::async_runtime::spawn(async move {
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
        }))
    });
    *app.view.lock().unwrap() = task;
}

/// Sends the entries of the directory at `path` on `channel` each time they
/// change, until `unwatch_directory` ends it. The watch ends when the
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
    app.directories.lock().unwrap().insert(id, Task(task));
}

/// Ends the directory watch that sends on the channel `channel_id`.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state to commands as `State` by value"
)]
fn unwatch_directory(app: State<'_, App>, channel_id: u32) {
    app.directories.lock().unwrap().remove(&channel_id);
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

/// The workspace's branches, as they stand without a fetch.
#[tauri::command]
async fn workspace_branches(app: State<'_, App>, workspace_id: String) -> Result<Branches, String> {
    app.client()
        .await?
        .branches(&workspace_id, false, utf8_env())
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

#[tauri::command]
async fn new_session(
    app: State<'_, App>,
    agent: Agent,
    workspace_id: String,
    checkout: Checkout,
) -> Result<SessionInfo, String> {
    let bridge = std::env::current_exe()
        .map_err(message)?
        .with_file_name(agent.bridge());
    let command = vec![bridge.to_string_lossy().into_owned()];
    app.client()
        .await?
        .new_session(&workspace_id, command, utf8_env(), checkout)
        .await
        .map_err(message)
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
    let mut prompt = Vec::new();
    if !text.is_empty() {
        prompt.push(ContentBlock::Text(TextContent::new(text)));
    }
    for path in attachments {
        prompt.push(content::file_link(&path).map_err(message)?);
    }
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

/// The outcome of deleting a session.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Deletion {
    Deleted {
        kept_branch: Option<String>,
    },
    /// The session's worktree has work the delete would lose; `message`
    /// lists it.
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

/// The outcome of a `session.delete` answer. A session that is already gone
/// counts as deleted.
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
            view_session,
            watch_directory,
            unwatch_directory,
            add_workspace,
            remove_workspace,
            workspace_branches,
            move_branch,
            new_session,
            load_session,
            prompt,
            cancel,
            permit,
            respond_elicitation,
            set_config_option,
            delete_session,
            archive_session,
            unarchive_session,
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
        file(&first, "claude", 0o755);
        file(&first, "pi", 0o644);
        std::fs::create_dir(second.path().join("pi")).unwrap();
        file(&second, "codex", 0o755);
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();

        for (name, found) in [
            ("claude", true),
            ("pi", false),
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
