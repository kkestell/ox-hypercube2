//! Directory listings and the one filesystem watcher that follows them.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use ox_protocol::{DirEntry, EntryKind};
use tokio::sync::watch;

/// Names a listing leaves out at every depth, whatever the entry's kind:
/// version-control folders, and the files operating systems leave behind.
/// A worktree's `.git` is a file.
const HIDDEN: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    "CVS",
    ".DS_Store",
    "Thumbs.db",
];

/// The directories being watched, by canonical path.
type Entries = Arc<Mutex<HashMap<PathBuf, Entry>>>;

struct Entry {
    /// Signalled when the directory or one of its entries changes.
    changed: watch::Sender<()>,
    /// The `Changes` that watch the directory.
    watchers: usize,
}

/// One filesystem watcher shared by every directory watch. Each directory is
/// watched non-recursively, once however many streams want it, until its
/// last stream ends.
//
// TODO: On Linux a directory deleted and recreated between two reads keeps a
// stale inotify watch, so its stream misses changes in the new directory.
// Revisit if the app runs on Linux.
//
// TODO: notify's FSEvents watcher restarts its stream from "now" each time a
// directory is watched or unwatched, so a change in another watched
// directory during the restart reaches no stream until that directory
// changes again. Revisit if open folders are seen to go stale.
pub struct Directories {
    /// Held while a directory is added to or removed from the watcher and
    /// the map, so the two agree. The event handler locks only the map,
    /// and the map's lock is never held while the watcher is called, since
    /// the `FSEvents` watcher waits for its handler to return when it changes
    /// its paths.
    watcher: Mutex<RecommendedWatcher>,
    entries: Entries,
}

impl Directories {
    pub fn new() -> notify::Result<Arc<Self>> {
        let entries = Entries::default();
        let handler = {
            let entries = entries.clone();
            move |event: notify::Result<notify::Event>| {
                let entries = entries.lock().unwrap();
                match event {
                    Ok(event) if !event.need_rescan() => {
                        if matches!(event.kind, EventKind::Access(_)) {
                            return;
                        }
                        // FSEvents reports real paths, which key the map.
                        for path in &event.paths {
                            for path in [Some(path.as_path()), path.parent()].into_iter().flatten()
                            {
                                if let Some(entry) = entries.get(path) {
                                    entry.changed.send_replace(());
                                }
                            }
                        }
                    }
                    // The watcher may have missed changes anywhere.
                    _ => {
                        for entry in entries.values() {
                            entry.changed.send_replace(());
                        }
                    }
                }
            }
        };
        let watcher = notify::recommended_watcher(handler)?;
        Ok(Arc::new(Self {
            watcher: Mutex::new(watcher),
            entries,
        }))
    }

    /// Watches the directory at `path` if it exists, else its nearest
    /// existing ancestor. Blocks while the watcher changes its paths.
    pub fn watch(self: &Arc<Self>, path: &Path) -> Changes {
        let mut watcher = self.watcher.lock().unwrap();
        for (depth, ancestor) in path.ancestors().enumerate() {
            if !ancestor.is_dir() {
                continue;
            }
            let Ok(canonical) = ancestor.canonicalize() else {
                continue;
            };
            let existing = self
                .entries
                .lock()
                .unwrap()
                .get_mut(&canonical)
                .map(|entry| {
                    entry.watchers += 1;
                    entry.changed.subscribe()
                });
            let changed = match existing {
                Some(changed) => changed,
                None => {
                    if watcher
                        .watch(&canonical, RecursiveMode::NonRecursive)
                        .is_err()
                    {
                        // Removed since it was found, or unreadable.
                        continue;
                    }
                    let (sender, changed) = watch::channel(());
                    self.entries.lock().unwrap().insert(
                        canonical.clone(),
                        Entry {
                            changed: sender,
                            watchers: 1,
                        },
                    );
                    changed
                }
            };
            return Changes {
                directories: self.clone(),
                path: canonical,
                target: depth == 0,
                changed,
            };
        }
        unreachable!("an absolute path has the root directory as an ancestor");
    }
}

/// Changes to a watched directory. Dropping it ends its watch.
pub struct Changes {
    directories: Arc<Directories>,
    /// The canonical path of the watched directory.
    path: PathBuf,
    /// Whether the watched directory is the one asked for rather than an
    /// ancestor.
    target: bool,
    changed: watch::Receiver<()>,
}

impl Changes {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_target(&self) -> bool {
        self.target
    }

    /// Marks every change so far seen.
    pub fn mark_seen(&mut self) {
        self.changed.borrow_and_update();
    }

    /// Waits for a change not yet seen.
    pub async fn changed(&mut self) {
        // The sender lives in the map while this receiver exists.
        let _ = self.changed.changed().await;
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        let mut watcher = self.directories.watcher.lock().unwrap();
        let mut entries = self.directories.entries.lock().unwrap();
        let entry = entries
            .get_mut(&self.path)
            .expect("a watched path has an entry");
        entry.watchers -= 1;
        if entry.watchers == 0 {
            entries.remove(&self.path);
            drop(entries);
            // Fails when the directory has gone, which ends its watch anyway.
            let _ = watcher.unwatch(&self.path);
        }
    }
}

/// The entries of the directory at `path`, sorted by name, without hidden
/// names or names that are not UTF-8, or nothing when it cannot be read.
pub fn read(path: &Path) -> Option<Vec<DirEntry>> {
    let mut entries: Vec<DirEntry> = fs::read_dir(path)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            if HIDDEN.contains(&name.as_str()) {
                return None;
            }
            let file_type = entry.file_type().ok()?;
            let directory = if file_type.is_symlink() {
                fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir())
            } else {
                file_type.is_dir()
            };
            let kind = if directory {
                EntryKind::Directory
            } else {
                EntryKind::File
            };
            Some(DirEntry { name, kind })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Some(entries)
}
