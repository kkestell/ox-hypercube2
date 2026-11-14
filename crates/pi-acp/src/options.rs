//! Pi's model and thinking choices as ACP session configuration.

use agent_client_protocol::v2::schema::{
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
};
use serde_json::{Value, json};

const LEVELS: [(&str, &str); 7] = [
    ("off", "Off"),
    ("minimal", "Minimal"),
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "XHigh"),
    ("max", "Max"),
];

pub fn build(state: &Value, models: &Value, levels: &Value) -> Vec<SessionConfigOption> {
    let (Some(provider), Some(id)) = (
        state["model"]["provider"].as_str(),
        state["model"]["id"].as_str(),
    ) else {
        return Vec::new();
    };
    let models = models.as_array().map(Vec::as_slice).unwrap_or_default();
    let choices = models
        .iter()
        .filter_map(|model| {
            let provider = model["provider"].as_str()?;
            let id = model["id"].as_str()?;
            let name = model["name"].as_str()?;
            let label = match provider {
                "openai-codex" => "OpenAI subscription",
                "openai" => "OpenAI",
                "openrouter" => "OpenRouter",
                "opencode-go" => "OpenCode Go",
                _ => provider,
            };
            Some(
                SessionConfigSelectOption::new(format!("{provider}/{id}"), name).meta(
                    serde_json::Map::from_iter([("ox/provider".into(), json!(label))]),
                ),
            )
        })
        .collect::<Vec<_>>();
    let mut options = vec![
        SessionConfigOption::select("model", "Model", format!("{provider}/{id}"), choices)
            .category(SessionConfigOptionCategory::Model),
    ];
    let levels = levels.as_array().map(Vec::as_slice).unwrap_or_default();
    if levels.len() > 1
        && let Some(current) = state["thinkingLevel"].as_str()
    {
        let choices = levels
            .iter()
            .filter_map(|level| {
                let level = level.as_str()?;
                let (_, name) = LEVELS.iter().find(|(value, _)| *value == level)?;
                Some(SessionConfigSelectOption::new(level, *name))
            })
            .collect::<Vec<_>>();
        options.push(
            SessionConfigOption::select("thinking", "Thinking", current.to_owned(), choices)
                .category(SessionConfigOptionCategory::ThoughtLevel),
        );
    }
    options
}

pub fn command(id: &str, value: &str) -> Option<Value> {
    match id {
        "model" => {
            let (provider, model) = value.split_once('/')?;
            if provider.is_empty() || model.is_empty() {
                return None;
            }
            Some(json!({"type":"set_model", "provider":provider, "modelId":model}))
        }
        "thinking" if LEVELS.iter().any(|(level, _)| *level == value) => {
            Some(json!({"type":"set_thinking_level", "level":value}))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_choices_preserve_ids_and_names_with_separate_providers() {
        let options = serde_json::to_value(build(
            &json!({"model":{"provider":"openai-codex","id":"org/one"},"thinkingLevel":"high"}),
            &json!([
                {"provider":"openai-codex","id":"org/one","name":"Shared"},
                {"provider":"openrouter","id":"two","name":"Shared"},
                {"provider":"opencode-go","id":"three","name":"Unique"},
                {"provider":"custom","id":"four","name":"Custom"}
            ]),
            &json!(["off", "minimal", "low", "medium", "high", "xhigh", "max"]),
        ))
        .unwrap();
        assert_eq!(options[0]["currentValue"], "openai-codex/org/one");
        assert_eq!(options[0]["category"], "model");
        assert_eq!(
            options[0]["options"],
            json!([
                {"value":"openai-codex/org/one", "name":"Shared", "_meta":{"ox/provider":"OpenAI subscription"}},
                {"value":"openrouter/two", "name":"Shared", "_meta":{"ox/provider":"OpenRouter"}},
                {"value":"opencode-go/three", "name":"Unique", "_meta":{"ox/provider":"OpenCode Go"}},
                {"value":"custom/four", "name":"Custom", "_meta":{"ox/provider":"custom"}}
            ])
        );
        assert_eq!(options[1]["category"], "thought_level");
        assert_eq!(options[1]["currentValue"], "high");
        assert_eq!(
            options[1]["options"],
            json!(LEVELS.map(|(value, name)| json!({"value":value,"name":name})))
        );
    }

    #[test]
    fn hides_unavailable_choices() {
        assert!(build(&Value::Null, &json!([]), &json!(["off", "high"])).is_empty());
        let options = build(
            &json!({"model":{"provider":"a","id":"one"},"thinkingLevel":"off"}),
            &json!([]),
            &json!(["off"]),
        );
        assert_eq!(options.len(), 1);
    }

    #[test]
    fn parses_supported_changes_without_splitting_model_ids() {
        assert_eq!(
            command("model", "a/org/one"),
            Some(json!({"type":"set_model","provider":"a","modelId":"org/one"}))
        );
        assert_eq!(
            command("thinking", "max"),
            Some(json!({"type":"set_thinking_level","level":"max"}))
        );
        for (id, value) in [
            ("model", "one"),
            ("model", "/one"),
            ("model", "a/"),
            ("thinking", "huge"),
            ("other", "a/one"),
        ] {
            assert_eq!(command(id, value), None, "{id}: {value}");
        }
    }
}
