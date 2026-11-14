use crate::{content, tools, updates::Updates};
use agent_client_protocol::v2::schema::ToolCallContent;
use serde_json::Value;
use std::{collections::HashMap, fs, io, path::Path};

/// Native history is recovery for clients without saved displayed history.
/// Replay message entries in file order, including those before compaction.
pub fn replay(path: &Path, cwd: &Path, updates: &Updates) -> io::Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let entries: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let mut calls = HashMap::new();
    for entry in entries {
        if entry["type"] != "message" {
            continue;
        }
        let id = entry["id"].as_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Pi message entry has no ID")
        })?;
        let message = &entry["message"];
        match message["role"].as_str() {
            Some("user") => updates.user_message(id, content::from_pi(&message["content"])),
            Some("assistant") => {
                for (index, block) in message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let block_id = format!("{id}:{index}");
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = block["text"].as_str() {
                                updates.message(&block_id, text.to_owned());
                            }
                        }
                        Some("thinking") => {
                            if let Some(text) = block["thinking"].as_str() {
                                updates.thought(&block_id, text.to_owned());
                            }
                        }
                        Some("toolCall") => {
                            if let (Some(id), Some(name)) =
                                (block["id"].as_str(), block["name"].as_str())
                            {
                                updates.open_tool(id, name, &block["arguments"], cwd);
                                calls.insert(
                                    id.to_owned(),
                                    (name.to_owned(), block["arguments"].clone()),
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("toolResult") => {
                let Some(id) = message["toolCallId"].as_str() else {
                    continue;
                };
                let failed = message["isError"] == true;
                let mut result: Vec<ToolCallContent> = calls
                    .remove(id)
                    .filter(|_| !failed)
                    .and_then(|(name, args)| {
                        tools::diff(&name, &args, &message["details"], cwd, None)
                    })
                    .into_iter()
                    .collect();
                result.extend(
                    content::from_pi(&message["content"])
                        .into_iter()
                        .map(ToolCallContent::from),
                );
                updates.tool_ended(id, failed, result);
            }
            _ => {}
        }
    }
    Ok(())
}
