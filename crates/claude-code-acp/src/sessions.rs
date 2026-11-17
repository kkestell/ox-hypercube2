//! Claude sessions read from their transcripts without starting Claude, for
//! a client that imports them: `--sessions` and `--transcript`.

use std::path::Path;

use chrono::DateTime;
use serde_json::{Value, json};

use crate::history::History;
use crate::updates::Updates;

/// The interactive sessions started in `dir`, newest first, as
/// `[{"sessionId", "startedAt"}]` with `startedAt` in epoch milliseconds.
pub fn sessions(history: &History, dir: &Path) -> Value {
    let mut sessions: Vec<(String, i64)> = history
        .sessions(dir)
        .into_iter()
        // TODO: a transcript whose head has no timestamp is left out; Claude
        // stamps the entry that names the entrypoint.
        .filter_map(|listed| Some((listed.id, millis(listed.started_at.as_deref()?)?)))
        .collect();
    sessions.sort_by(|(a_id, a_at), (b_id, b_at)| b_at.cmp(a_at).then_with(|| a_id.cmp(b_id)));
    sessions
        .into_iter()
        .map(|(id, at)| json!({"sessionId": id, "startedAt": at}))
        .collect()
}

/// Session `id`'s title and the updates its transcript replays as, each with
/// the time of the entry that produced it, as
/// `{"title", "updates": [{"at", "update"}]}`.
pub fn transcript(history: &History, dir: &Path, id: &str) -> Result<Value, String> {
    if !history.find(id, dir) {
        return Err(format!("session {id} not found"));
    }
    let entries = history.messages(id, dir);
    let mut subagents = history.subagents(id, dir);
    let updates = Updates::listed(id);
    // An entry without a timestamp takes the previous entry's; one before
    // any timestamp takes the first.
    let mut at = entries.iter().find_map(entry_millis).unwrap_or(0);
    let mut timed = Vec::new();
    let mut stamp = |at: i64, updates: &Updates| {
        timed.extend(
            updates
                .take_listed()
                .into_iter()
                .map(|update| json!({"at": at, "update": update})),
        );
    };
    for entry in &entries {
        at = entry_millis(entry).unwrap_or(at);
        updates.replay_entry(entry, &mut subagents);
        stamp(at, &updates);
    }
    updates.fail_open_tools();
    stamp(at, &updates);
    Ok(json!({"title": history.title(id, dir), "updates": timed}))
}

fn entry_millis(entry: &Value) -> Option<i64> {
    millis(entry["timestamp"].as_str()?)
}

fn millis(timestamp: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|time| time.timestamp_millis())
}
