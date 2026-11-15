//! Live text snapshots using the daemon's shared filesystem watcher.
use crate::directories::Directories;
use ox_protocol::{FileContent, ServerMessage};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::mpsc;

fn read(path: &Path) -> FileContent {
    let text = (|| {
        let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Not a regular file".to_owned());
        }
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        if text.contains('\0') {
            return Err("Binary files cannot be displayed".to_owned());
        }
        Ok(text)
    })();
    match text {
        Ok(text) => FileContent::Text { text },
        Err(message) => FileContent::Error { message },
    }
}

pub async fn stream(
    directories: Arc<Directories>,
    path: PathBuf,
    subscription: u64,
    outgoing: mpsc::Sender<ServerMessage>,
) {
    let parent = path.parent().unwrap_or(Path::new("/")).to_owned();
    let watch = || {
        let directories = directories.clone();
        let parent = parent.clone();
        tokio::task::spawn_blocking(move || directories.watch(&parent))
    };
    let mut changes = watch().await.expect("watching does not panic");
    let mut sent = None;
    loop {
        changes.mark_seen();
        let path = path.clone();
        let content = tokio::task::spawn_blocking(move || read(&path))
            .await
            .expect("reading does not panic");
        if !changes.is_target() || matches!(content, FileContent::Error { .. }) {
            let next = watch().await.expect("watching does not panic");
            if next.path() != changes.path() {
                changes = next;
                continue;
            }
        }
        if sent.as_ref() != Some(&content) {
            if outgoing
                .send(ServerMessage::File {
                    subscription,
                    content: content.clone(),
                })
                .await
                .is_err()
            {
                return;
            }
            sent = Some(content);
        }
        changes.changed().await;
    }
}
