//! Pi's extension commands, prompt templates, and skills as ACP commands.

use std::collections::HashSet;

use agent_client_protocol::v2::schema::{AvailableCommand, Command};
use serde_json::Value;

pub fn catalog(value: &Value) -> Vec<AvailableCommand> {
    let mut names = HashSet::new();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry["name"].as_str()?;
            if name.is_empty() || name.chars().any(char::is_whitespace) || !names.insert(name) {
                return None;
            }
            Some(AvailableCommand::Command(Command::new(
                name,
                entry["description"].as_str().unwrap_or_default(),
            )))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_pi_order_and_skill_names_with_optional_descriptions() {
        assert_eq!(
            serde_json::to_value(catalog(&json!([
                {"name":"template","description":"A template"},
                {"name":"extension"},
                {"name":"skill:review","description":"Review"},
                {"name":"template","description":"Duplicate"},
                {"name":""}, {"name":"bad name"}, {"name":"bad\tname"}, {}
            ])))
            .unwrap(),
            json!([
                {"type":"command","name":"template","description":"A template"},
                {"type":"command","name":"extension","description":""},
                {"type":"command","name":"skill:review","description":"Review"}
            ])
        );
        assert!(catalog(&Value::Null).is_empty());
    }
}
