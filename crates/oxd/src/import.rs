//! Claude Code sessions imported from their transcripts, which the Claude
//! bridge reads with its `--sessions` and `--transcript` modes.

use std::collections::BTreeMap;
use std::path::Path;

use ox_protocol::schema::{SessionUpdate, StopReason};
use ox_protocol::{Event, EventKind};
use serde::Deserialize;

/// A Claude session the bridge lists.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSession {
    pub session_id: String,
    /// When the session started, in milliseconds since the Unix epoch.
    pub started_at: u64,
}

/// A Claude session's title and history, as the bridge reads them from its
/// transcript.
#[derive(Debug, Deserialize)]
pub struct Transcript {
    pub title: Option<String>,
    pub updates: Vec<TimedUpdate>,
}

/// An update of a transcript, with the time of the entry that produced it.
#[derive(Debug, Deserialize)]
pub struct TimedUpdate {
    /// In milliseconds since the Unix epoch.
    pub at: u64,
    pub update: SessionUpdate,
}

/// The interactive Claude sessions started in `dir`, newest first. The
/// bridge runs with `env` over the daemon's environment.
pub async fn sessions(
    bridge: &Path,
    dir: &Path,
    env: &BTreeMap<String, String>,
) -> Result<Vec<ClaudeSession>, String> {
    ox_protocol::bridge_mode(bridge, &["--sessions".as_ref(), dir.as_os_str()], env).await
}

/// The transcript of Claude session `id`, started in `dir`.
pub async fn transcript(
    bridge: &Path,
    dir: &Path,
    id: &str,
    env: &BTreeMap<String, String>,
) -> Result<Transcript, String> {
    ox_protocol::bridge_mode(
        bridge,
        &["--transcript".as_ref(), dir.as_os_str(), id.as_ref()],
        env,
    )
    .await
}

/// The history of session `session_id` from a transcript's updates: each
/// user message starts a turn, which ends when the next one starts, and the
/// last turn ends after the last event. Every other update is recorded as
/// it is. Each event has its update's time; a turn's end has the time of the
/// event before it.
pub fn events(session_id: &str, updates: Vec<TimedUpdate>) -> Vec<Event> {
    let mut events: Vec<Event> = Vec::new();
    let mut seq = 0;
    let mut push = |kind: EventKind, recorded_at: u64| {
        seq += 1;
        events.push(Event {
            session_id: session_id.into(),
            seq,
            recorded_at,
            kind,
        });
    };
    let turn_ended = || EventKind::TurnEnded {
        stop_reason: Some(StopReason::EndTurn),
        error: None,
    };
    let mut prompted = false;
    let mut last = 0;
    for TimedUpdate { at, update } in updates {
        match update {
            SessionUpdate::UserMessage(message)
                if message
                    .content
                    .value()
                    .is_some_and(|content| !content.is_empty()) =>
            {
                if prompted {
                    push(turn_ended(), last);
                }
                prompted = true;
                let prompt = message.content.take().unwrap_or_default();
                push(EventKind::PromptStarted { prompt, from: None }, at);
            }
            update => push(EventKind::Update(update), at),
        }
        last = at;
    }
    if prompted {
        push(turn_ended(), last);
    }
    events
}

#[cfg(test)]
mod tests {
    use ox_protocol::schema::{
        AgentMessage, ContentBlock, TextContent, ToolCallStatus, ToolCallUpdate, UserMessage,
    };

    use super::*;

    fn text(text: &str) -> Vec<ContentBlock> {
        vec![ContentBlock::Text(TextContent::new(text))]
    }

    fn user(at: u64, prompt: &str) -> TimedUpdate {
        TimedUpdate {
            at,
            update: SessionUpdate::UserMessage(UserMessage::new("u").content(text(prompt))),
        }
    }

    fn agent(at: u64, reply: &str) -> TimedUpdate {
        TimedUpdate {
            at,
            update: SessionUpdate::AgentMessage(AgentMessage::new("a").content(text(reply))),
        }
    }

    fn tool(at: u64) -> TimedUpdate {
        TimedUpdate {
            at,
            update: SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new("t").status(ToolCallStatus::Completed),
            ),
        }
    }

    /// Each event's seq, time and kind, with updates by their kind.
    fn described(events: &[Event]) -> Vec<(u64, u64, String)> {
        events
            .iter()
            .map(|event| {
                assert_eq!(event.session_id, "s");
                let kind = match &event.kind {
                    EventKind::PromptStarted { prompt, from: None } => match &prompt[..] {
                        [ContentBlock::Text(text)] => format!("prompt {}", text.text),
                        _ => panic!("unexpected prompt {prompt:?}"),
                    },
                    EventKind::TurnEnded {
                        stop_reason: Some(StopReason::EndTurn),
                        error: None,
                    } => "turn ended".into(),
                    EventKind::Update(SessionUpdate::AgentMessage(_)) => "agent message".into(),
                    EventKind::Update(SessionUpdate::ToolCallUpdate(_)) => "tool call".into(),
                    kind => panic!("unexpected event {kind:?}"),
                };
                (event.seq, event.recorded_at, kind)
            })
            .collect()
    }

    #[test]
    fn user_messages_start_turns_that_end_before_the_next() {
        let events = events(
            "s",
            vec![
                user(10, "Fix it"),
                agent(20, "On it"),
                tool(30),
                user(40, "Thanks"),
                agent(50, "Welcome"),
            ],
        );
        let expected = [
            (1, 10, "prompt Fix it"),
            (2, 20, "agent message"),
            (3, 30, "tool call"),
            (4, 30, "turn ended"),
            (5, 40, "prompt Thanks"),
            (6, 50, "agent message"),
            (7, 50, "turn ended"),
        ];
        assert_eq!(
            described(&events),
            expected.map(|(seq, at, kind)| (seq, at, kind.to_owned()))
        );
    }

    #[test]
    fn updates_without_a_prompt_end_no_turn() {
        let events = events("s", vec![agent(10, "Hello"), tool(20)]);
        assert_eq!(
            described(&events),
            [
                (1, 10, "agent message".to_owned()),
                (2, 20, "tool call".to_owned())
            ]
        );
    }
}
