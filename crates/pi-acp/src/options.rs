//! Pi's model and thinking level as ACP session configuration. Each option
//! offers only its current value: the client lists the values it offers.

use agent_client_protocol::v2::schema::{
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
};
use serde_json::{Value, json};

/// The `model` option, `provider/id`, and the `effort` option, Pi's thinking
/// level, from Pi's state.
pub fn build(state: &Value) -> Vec<SessionConfigOption> {
    let (Some(provider), Some(id)) = (
        state["model"]["provider"].as_str(),
        state["model"]["id"].as_str(),
    ) else {
        return Vec::new();
    };
    let mut options = vec![current(
        "model",
        "Model",
        format!("{provider}/{id}"),
        SessionConfigOptionCategory::Model,
    )];
    if let Some(level) = state["thinkingLevel"].as_str() {
        options.push(current(
            "effort",
            "Effort",
            level.to_owned(),
            SessionConfigOptionCategory::ThoughtLevel,
        ));
    }
    options
}

/// A select option whose only choice is its current value.
fn current(
    id: &'static str,
    name: &'static str,
    value: String,
    category: SessionConfigOptionCategory,
) -> SessionConfigOption {
    let choice = SessionConfigSelectOption::new(value.clone(), value.clone());
    SessionConfigOption::select(id, name, value, vec![choice]).category(category)
}

/// The Pi command that sets option `id` to `value`.
pub fn command(id: &str, value: &str) -> Option<Value> {
    match id {
        "model" => {
            let (provider, model) = value.split_once('/')?;
            if provider.is_empty() || model.is_empty() {
                return None;
            }
            Some(json!({"type":"set_model", "provider":provider, "modelId":model}))
        }
        "effort" if !value.is_empty() => Some(json!({"type":"set_thinking_level", "level":value})),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_and_effort_offer_their_current_values() {
        let options = serde_json::to_value(build(
            &json!({"model":{"provider":"openai","id":"org/one"},"thinkingLevel":"high"}),
        ))
        .unwrap();
        assert_eq!(
            options,
            json!([
                {
                    "type":"select", "configId":"model", "name":"Model", "category":"model",
                    "currentValue":"openai/org/one",
                    "options":[{"value":"openai/org/one", "name":"openai/org/one"}]
                },
                {
                    "type":"select", "configId":"effort", "name":"Effort", "category":"thought_level",
                    "currentValue":"high",
                    "options":[{"value":"high", "name":"high"}]
                }
            ])
        );
    }

    #[test]
    fn leaves_out_what_the_state_lacks() {
        assert!(build(&Value::Null).is_empty());
        let options = build(&json!({"model":{"provider":"a","id":"one"}}));
        assert_eq!(options.len(), 1);
    }

    #[test]
    fn parses_supported_changes_without_splitting_model_ids() {
        assert_eq!(
            command("model", "a/org/one"),
            Some(json!({"type":"set_model","provider":"a","modelId":"org/one"}))
        );
        assert_eq!(
            command("effort", "huge"),
            Some(json!({"type":"set_thinking_level","level":"huge"}))
        );
        for (id, value) in [
            ("model", "one"),
            ("model", "/one"),
            ("model", "a/"),
            ("effort", ""),
            ("thinking", "high"),
            ("other", "a/one"),
        ] {
            assert_eq!(command(id, value), None, "{id}: {value}");
        }
    }
}
