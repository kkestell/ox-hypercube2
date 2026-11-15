//! Forward file snapshots to webview channels.
use crate::{App, Task};
use ox_protocol::FileContent;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, State, ipc::Channel};

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state by value"
)]
pub fn watch_file(
    handle: AppHandle,
    app: State<'_, App>,
    path: PathBuf,
    channel: Channel<FileContent>,
) {
    let id = channel.id();
    let task = tauri::async_runtime::spawn(async move {
        let app = handle.state::<App>();
        let watch = match app.client().await {
            Ok(client) => client.watch_file(&path).await.map_err(crate::message),
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
    app.files.lock().unwrap().insert(id, Task(task));
}

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri passes managed state by value"
)]
pub fn unwatch_file(app: State<'_, App>, channel_id: u32) {
    app.files.lock().unwrap().remove(&channel_id);
}
