//! A session's compact transcript: its prompts, agent messages, tool calls
//! and turn ends, each an entry of plain text, read a page at a time from
//! the latest.

use std::collections::HashMap;

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v2::schema::{
    ContentBlock, SessionUpdate, StopReason, ToolCallId, ToolCallStatus,
};
use ox_protocol::{Event, EventKind, Sender, TranscriptPage};

/// The most characters one page holds, counting a blank line between
/// entries as two.
const BUDGET: usize = 40_000;

/// One entry, keyed by the seq of its first event.
#[derive(Debug, PartialEq)]
struct Entry {
    seq: u64,
    text: String,
}

/// A tool call as its updates have built it.
#[derive(Default)]
struct Call {
    entry: usize,
    name: Option<String>,
    title: Option<String>,
    failed: bool,
}

/// The entries of `events`, in the order their first events were recorded.
fn entries(events: &[Event]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    // The entry of each agent message and tool call.
    let mut messages: HashMap<String, usize> = HashMap::new();
    let mut calls: HashMap<ToolCallId, Call> = HashMap::new();
    for event in events {
        let seq = event.seq;
        match &event.kind {
            EventKind::PromptStarted { prompt, from }
            | EventKind::PromptSteered { prompt, from } => {
                add(&mut entries, seq, prompt_line(prompt, from.as_ref()));
            }
            EventKind::Update(SessionUpdate::AgentMessageChunk(chunk)) => {
                let index = *messages
                    .entry(chunk.message_id.to_string())
                    .or_insert_with(|| add(&mut entries, seq, "Assistant: ".into()));
                entries[index]
                    .text
                    .push_str(&text(std::slice::from_ref(&chunk.content)));
            }
            EventKind::Update(SessionUpdate::AgentMessage(message)) => {
                let index = *messages
                    .entry(message.message_id.to_string())
                    .or_insert_with(|| add(&mut entries, seq, "Assistant: ".into()));
                match &message.content {
                    MaybeUndefined::Value(content) => {
                        entries[index].text = format!("Assistant: {}", text(content));
                    }
                    MaybeUndefined::Null => entries[index].text = "Assistant: ".into(),
                    MaybeUndefined::Undefined => {}
                }
            }
            EventKind::Update(SessionUpdate::ToolCallUpdate(update)) => {
                let call = calls
                    .entry(update.tool_call_id.clone())
                    .or_insert_with(|| Call {
                        entry: add(&mut entries, seq, String::new()),
                        ..Call::default()
                    });
                update.name.clone().update_to(&mut call.name);
                update.title.clone().update_to(&mut call.title);
                match &update.status {
                    MaybeUndefined::Value(status) => {
                        call.failed = *status == ToolCallStatus::Failed;
                    }
                    MaybeUndefined::Null => call.failed = false,
                    MaybeUndefined::Undefined => {}
                }
                let title = call
                    .title
                    .as_deref()
                    .or(call.name.as_deref())
                    .unwrap_or("Tool call");
                let failed = if call.failed { " (failed)" } else { "" };
                entries[call.entry].text = format!("Tool: {title}{failed}");
            }
            EventKind::TurnEnded { stop_reason, error } => {
                add(
                    &mut entries,
                    seq,
                    turn_end(stop_reason.as_ref(), error.as_deref()),
                );
            }
            _ => {}
        }
    }
    // A message with no text says nothing.
    entries.retain(|entry| entry.text != "Assistant: ");
    entries
}

/// Adds an entry and returns its index.
fn add(entries: &mut Vec<Entry>, seq: u64, text: String) -> usize {
    entries.push(Entry { seq, text });
    entries.len() - 1
}

/// A prompt's entry: `User:`, or the sender of a peer message.
fn prompt_line(prompt: &[ContentBlock], from: Option<&Sender>) -> String {
    let text = prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    match from {
        None => format!("User: {text}"),
        Some(Sender {
            session_id,
            title: Some(title),
        }) => format!("Message from {session_id} (\"{title}\"): {text}"),
        Some(Sender {
            session_id,
            title: None,
        }) => format!("Message from {session_id}: {text}"),
    }
}

/// The text blocks of a message, joined.
fn text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

/// How a turn ended.
fn turn_end(stop_reason: Option<&StopReason>, error: Option<&str>) -> String {
    match (error, stop_reason) {
        (Some(error), _) => format!("(turn failed: {error})"),
        (None, Some(StopReason::Error(details))) => format!(
            "(turn failed: {})",
            details
                .error
                .as_ref()
                .map_or("the agent failed", |error| error.message.as_str())
        ),
        (None, Some(StopReason::EndTurn) | None) => "(turn finished)".into(),
        (None, Some(StopReason::Cancelled)) => "(turn cancelled)".into(),
        (None, Some(other)) => {
            let value = serde_json::to_value(other).expect("stop reasons serialize");
            let name = value["stopReason"].as_str().unwrap_or("unknown");
            format!("(turn ended: {name})")
        }
    }
}

/// The page of `events`' transcript that ends before seq `before`, or at the
/// end: the latest entries that fit the budget. An entry longer than the
/// budget alone is cut and ends with `…`.
pub fn page(events: &[Event], before: Option<u64>) -> TranscriptPage {
    let entries = entries(events);
    let earlier = entries
        .iter()
        .take_while(|entry| before.is_none_or(|before| entry.seq < before));
    let mut page: Vec<String> = Vec::new();
    let mut oldest = None;
    let mut size = 0;
    let mut more = false;
    for entry in earlier.collect::<Vec<_>>().into_iter().rev() {
        let length = entry.text.chars().count();
        let text = if size + length <= BUDGET {
            entry.text.clone()
        } else if page.is_empty() {
            let mut cut: String = entry.text.chars().take(BUDGET).collect();
            cut.push('…');
            cut
        } else {
            more = true;
            break;
        };
        page.push(text);
        size += length + 2;
        oldest = Some(entry.seq);
    }
    page.reverse();
    TranscriptPage {
        text: page.join("\n\n"),
        before: oldest.filter(|_| more),
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::v2::schema::{
        AgentMessage, ContentChunk, ErrorStopReason, TextContent, ToolCallUpdate,
    };

    use super::*;

    fn events(kinds: Vec<EventKind>) -> Vec<Event> {
        kinds
            .into_iter()
            .zip(1..)
            .map(|(kind, seq)| Event {
                session_id: "s".into(),
                seq,
                recorded_at: 0,
                kind,
            })
            .collect()
    }

    fn block(text: &str) -> ContentBlock {
        ContentBlock::Text(TextContent::new(text))
    }

    fn prompt(text: &str) -> EventKind {
        EventKind::PromptStarted {
            prompt: vec![block(text)],
            from: None,
        }
    }

    fn chunk(id: &str, text: &str) -> EventKind {
        EventKind::Update(SessionUpdate::AgentMessageChunk(ContentChunk::new(
            block(text),
            id,
        )))
    }

    fn tool(id: &str, title: Option<&str>, status: Option<ToolCallStatus>) -> EventKind {
        let mut update = ToolCallUpdate::new(id);
        if let Some(title) = title {
            update.title = MaybeUndefined::Value(title.into());
        }
        if let Some(status) = status {
            update.status = MaybeUndefined::Value(status);
        }
        EventKind::Update(SessionUpdate::ToolCallUpdate(update))
    }

    fn ended(stop_reason: Option<StopReason>, error: Option<&str>) -> EventKind {
        EventKind::TurnEnded {
            stop_reason,
            error: error.map(String::from),
        }
    }

    fn lines(kinds: Vec<EventKind>) -> Vec<(u64, String)> {
        entries(&events(kinds))
            .into_iter()
            .map(|entry| (entry.seq, entry.text))
            .collect()
    }

    #[test]
    fn events_fold_into_one_entry_each_keyed_by_their_first_event() {
        let peer = |title: Option<&str>| EventKind::PromptSteered {
            prompt: vec![block("are you done?")],
            from: Some(Sender {
                session_id: "peer".into(),
                title: title.map(String::from),
            }),
        };
        let mut whole = AgentMessage::new("m2");
        whole.content = MaybeUndefined::Value(vec![block("Replaced.")]);
        let lines = lines(vec![
            prompt("Fix the build"),
            chunk("m1", "Looking"),
            tool("t1", Some("Read `a.rs`"), None),
            chunk("m1", " now."),
            tool("t1", None, Some(ToolCallStatus::Failed)),
            peer(Some("Plan")),
            peer(None),
            chunk("m2", "Draft"),
            EventKind::Update(SessionUpdate::AgentMessage(whole)),
            ended(Some(StopReason::EndTurn), None),
        ]);
        assert_eq!(
            lines,
            [
                (1, "User: Fix the build".into()),
                (2, "Assistant: Looking now.".into()),
                (3, "Tool: Read `a.rs` (failed)".into()),
                (6, "Message from peer (\"Plan\"): are you done?".into()),
                (7, "Message from peer: are you done?".into()),
                (8, "Assistant: Replaced.".into()),
                (10, "(turn finished)".into()),
            ]
        );
    }

    #[test]
    fn each_kind_of_turn_end_has_its_line() {
        let mut details = ErrorStopReason::new();
        details.error = Some(Box::new(agent_client_protocol::v2::schema::Error::new(
            1,
            "overloaded",
        )));
        for (end, expected) in [
            (ended(None, None), "(turn finished)"),
            (ended(Some(StopReason::Cancelled), None), "(turn cancelled)"),
            (
                ended(None, Some("the agent exited")),
                "(turn failed: the agent exited)",
            ),
            (
                ended(Some(StopReason::Error(details)), None),
                "(turn failed: overloaded)",
            ),
            (
                ended(Some(StopReason::MaxTokens), None),
                "(turn ended: max_tokens)",
            ),
        ] {
            assert_eq!(lines(vec![end])[0].1, expected);
        }
    }

    #[test]
    fn an_empty_history_has_no_entries() {
        assert_eq!(
            page(&[], None),
            TranscriptPage {
                text: String::new(),
                before: None,
            }
        );
    }

    #[test]
    fn a_page_stops_at_the_budget_and_names_the_earlier_entries() {
        // Each entry is "User: " and 19,990 characters: two fit with the blank
        // line between them, and a third does not.
        let long = |c: char| prompt(&c.to_string().repeat(19_990));
        let history = events(vec![long('a'), long('b'), long('c')]);
        let latest = page(&history, None);
        assert_eq!(latest.before, Some(2));
        assert!(
            latest.text.starts_with("User: bbb"),
            "{}",
            &latest.text[..20]
        );
        assert!(latest.text.ends_with("ccc"));
        assert_eq!(latest.text.matches("\n\n").count(), 1);

        let earlier = page(&history, latest.before);
        assert_eq!(earlier.before, None);
        assert!(earlier.text.starts_with("User: aaa"));
        assert!(earlier.text.ends_with("aaa"));
    }

    #[test]
    fn an_entry_over_the_budget_is_cut() {
        let history = events(vec![prompt("hi"), prompt(&"x".repeat(BUDGET))]);
        let latest = page(&history, None);
        assert_eq!(latest.text.chars().count(), BUDGET + 1);
        assert!(latest.text.starts_with("User: xxx"));
        assert!(latest.text.ends_with("x…"));
        assert_eq!(latest.before, Some(2));
        assert_eq!(page(&history, Some(2)).text, "User: hi");
    }
}
