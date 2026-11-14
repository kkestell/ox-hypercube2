use agent_client_protocol::v2::schema::ContentBlock;
use serde_json::{Value, json};

/// Converts an ACP prompt into the `message` and `images` of Pi's `prompt`
/// command.
///
/// Text blocks and resource links, as Markdown links Pi can follow with its
/// tools, join into the message, one per line. Images become Pi images.
pub fn pi_prompt(prompt: &[ContentBlock]) -> Result<(String, Vec<Value>), String> {
    let empty = prompt.iter().all(|block| match block {
        ContentBlock::Text(text) => text.text.trim().is_empty(),
        _ => false,
    });
    if empty {
        return Err("Empty prompt".to_owned());
    }
    let mut lines = Vec::new();
    let mut images = Vec::new();
    for block in prompt {
        match block {
            ContentBlock::Text(text) => lines.push(text.text.clone()),
            ContentBlock::ResourceLink(link) => {
                lines.push(format!("[@{}]({})", link.name, link.uri));
            }
            ContentBlock::Image(image) => images.push(json!({
                "type": "image",
                "data": image.data,
                "mimeType": image.mime_type,
            })),
            ContentBlock::Audio(_) => return Err("Audio prompts are not supported".to_owned()),
            ContentBlock::Resource(_) => {
                return Err("Embedded resources are not supported".to_owned());
            }
            _ => return Err("Unsupported content block".to_owned()),
        }
    }
    Ok((lines.join("\n"), images))
}

/// The text of a prompt's text blocks, for its session title.
pub fn prompt_text(prompt: &[ContentBlock]) -> String {
    prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
