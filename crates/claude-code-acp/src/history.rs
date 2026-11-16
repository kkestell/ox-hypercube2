use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use serde_json::Value;
use uuid::Uuid;

/// The transcripts Claude keeps under its config directory.
pub struct History {
    root: PathBuf,
}

impl History {
    pub fn new(claude_dir: &Path) -> Self {
        Self {
            root: claude_dir.join("projects"),
        }
    }

    pub fn find(&self, id: &str, cwd: &Path) -> bool {
        self.transcript(id, cwd).is_some()
    }

    /// The `user` and `assistant` entries of a session's transcript, in
    /// order. The transcript's other entries are Claude's bookkeeping, and
    /// its sidechain entries belong to subagents.
    pub fn messages(&self, id: &str, cwd: &Path) -> Vec<Value> {
        let mut entries = self.entries(id, cwd);
        entries.retain(|entry| entry["isSidechain"] != true);
        entries
    }

    /// The `user` and `assistant` entries of a session's transcript, in
    /// order, with the sidechain entries of subagents.
    pub fn entries(&self, id: &str, cwd: &Path) -> Vec<Value> {
        self.transcript(id, cwd)
            .map(|path| messages(&path))
            .unwrap_or_default()
    }

    /// The `user` and `assistant` entries of each subagent of a session, by
    /// the tool call that started it. Claude keeps a subagent's transcript
    /// in `<session>/subagents/agent-<id>.jsonl`, beside a `.meta.json` that
    /// names the tool call as `toolUseId`.
    pub fn subagents(&self, id: &str, cwd: &Path) -> HashMap<String, Vec<Value>> {
        let Some(dir) = self
            .transcript(id, cwd)
            .map(|path| path.with_extension("").join("subagents"))
        else {
            return HashMap::new();
        };
        let Ok(files) = fs::read_dir(&dir) else {
            return HashMap::new();
        };
        files
            .flatten()
            .filter_map(|file| {
                let path = file.path();
                let name = path.file_name()?.to_str()?.strip_suffix(".meta.json")?;
                let meta: Value = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
                let tool_use_id = meta["toolUseId"].as_str()?.to_owned();
                Some((tool_use_id, messages(&dir.join(format!("{name}.jsonl")))))
            })
            .collect()
    }

    /// The transcript of session `id` started in `cwd`.
    fn transcript(&self, id: &str, cwd: &Path) -> Option<PathBuf> {
        Uuid::parse_str(id).ok()?;
        let projects = fs::read_dir(&self.root).ok()?;
        projects
            .flatten()
            .map(|project| project.path().join(format!("{id}.jsonl")))
            .find(|path| transcript_cwd(path).is_some_and(|found| found == cwd))
    }
}

/// The `user` and `assistant` entries of the transcript at `path`.
fn messages(path: &Path) -> Vec<Value> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|entry| matches!(entry["type"].as_str(), Some("user" | "assistant")))
        .collect()
}

/// The working directory of the session whose transcript is at `path`, from
/// the first entry that has one.
fn transcript_cwd(path: &Path) -> Option<PathBuf> {
    if !path.metadata().ok()?.is_file() {
        return None;
    }
    let mut reader = BufReader::new(File::open(path).ok()?).take(1024 * 1024);
    let mut line = String::new();
    while reader.read_line(&mut line).ok()? > 0 {
        if let Ok(entry) = serde_json::from_str::<Value>(&line)
            && let Some(cwd) = entry.get("cwd").and_then(Value::as_str)
        {
            return Some(PathBuf::from(cwd));
        }
        line.clear();
    }
    None
}
