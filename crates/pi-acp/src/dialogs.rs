//! Pi extension dialogs as session-scoped forms and their answers as RPC responses.

use std::time::Duration;

use agent_client_protocol::v2::schema::{
    ElicitationAction, ElicitationContentValue, ElicitationSchema, EnumOption, StringPropertySchema,
};
use serde_json::{Value, json};

pub fn form(request: &Value) -> Option<(String, ElicitationSchema)> {
    let mut field = StringPropertySchema::new().title(request["title"].as_str().map(str::to_owned));
    let required = match request["method"].as_str()? {
        "select" => {
            let mut options = Vec::new();
            for option in request["options"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
            {
                if !options.contains(&option) {
                    options.push(option);
                }
            }
            if options.is_empty() {
                return None;
            }
            field = field.one_of(
                options
                    .into_iter()
                    .map(|s| EnumOption::new(s, s))
                    .collect::<Vec<_>>(),
            );
            true
        }
        "confirm" => {
            field = field
                .one_of(vec![
                    EnumOption::new("yes", "Yes"),
                    EnumOption::new("no", "No"),
                ])
                .description(nonempty(request, "message"));
            true
        }
        "input" => {
            field = field.description(nonempty(request, "placeholder"));
            false
        }
        "editor" => {
            field = field.default_value(request["prefill"].as_str().map(str::to_owned));
            false
        }
        _ => return None,
    };
    Some((
        "Pi has a question".to_owned(),
        ElicitationSchema::new().property("answer", field, required),
    ))
}

/// How long the dialog stays open: its `timeout` in milliseconds. Pi treats
/// zero as no timeout.
pub fn timeout(request: &Value) -> Option<Duration> {
    request["timeout"]
        .as_u64()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
}

fn nonempty(request: &Value, key: &str) -> Option<String> {
    request[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub fn response(request: &Value, action: Option<ElicitationAction>) -> Value {
    let mut response = json!({"type": "extension_ui_response", "id": request["id"]});
    let answer = match action {
        Some(ElicitationAction::Accept(accepted)) => {
            let content = accepted.content.unwrap_or_default();
            match content.get("answer") {
                Some(ElicitationContentValue::String(text)) => Some(text.clone()),
                None if matches!(request["method"].as_str(), Some("input" | "editor")) => {
                    Some(String::new())
                }
                _ => None,
            }
        }
        _ => None,
    };
    match (request["method"].as_str(), answer) {
        (Some("select"), Some(answer))
            if request["options"]
                .as_array()
                .is_some_and(|options| options.contains(&json!(answer))) =>
        {
            response["value"] = json!(answer);
        }
        (Some("confirm"), Some(answer)) if answer == "yes" || answer == "no" => {
            response["confirmed"] = json!(answer == "yes");
        }
        (Some("input" | "editor"), Some(answer)) => response["value"] = json!(answer),
        _ => response["cancelled"] = json!(true),
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialogs_become_choices_or_optional_text() {
        for (request, expected, required) in [
            (
                json!({"method":"select", "title":"Pick", "options":["Allow", "Block", "Allow", 3]}),
                json!({"type":"string", "title":"Pick", "oneOf":[{"const":"Allow","title":"Allow"},{"const":"Block","title":"Block"}]}),
                true,
            ),
            (
                json!({"method":"confirm", "title":"Run?", "message":"Delete build"}),
                json!({"type":"string", "title":"Run?", "description":"Delete build", "oneOf":[{"const":"yes","title":"Yes"},{"const":"no","title":"No"}]}),
                true,
            ),
            (
                json!({"method":"input", "title":"Name", "placeholder":"Optional name"}),
                json!({"type":"string", "title":"Name", "description":"Optional name"}),
                false,
            ),
            (
                json!({"method":"editor", "title":"Edit", "prefill":"first\nsecond"}),
                json!({"type":"string", "title":"Edit", "default":"first\nsecond"}),
                false,
            ),
        ] {
            let (message, schema) = form(&request).unwrap();
            let schema = serde_json::to_value(schema).unwrap();
            assert_eq!(message, "Pi has a question");
            assert_eq!(schema["properties"]["answer"], expected, "{request}");
            assert_eq!(
                schema["required"] == json!(["answer"]),
                required,
                "{request}"
            );
        }
        assert!(form(&json!({"method":"select", "options":[3]})).is_none());
        assert!(form(&json!({"method":"notify"})).is_none());
    }

    #[test]
    fn a_positive_timeout_limits_the_dialog_and_zero_or_none_leaves_it_open() {
        for (request, expected) in [
            (json!({"timeout": 50}), Some(Duration::from_millis(50))),
            (json!({"timeout": 0}), None),
            (json!({}), None),
            (json!({"timeout": -5}), None),
            (json!({"timeout": "50"}), None),
        ] {
            assert_eq!(timeout(&request), expected, "{request}");
        }
    }

    #[test]
    fn accepted_answers_return_pi_values_and_invalid_answers_cancel() {
        for (method, content, expected) in [
            (
                "select",
                json!({"answer":"Block"}),
                json!({"value":"Block"}),
            ),
            (
                "select",
                json!({"answer":"Other"}),
                json!({"cancelled":true}),
            ),
            (
                "confirm",
                json!({"answer":"yes"}),
                json!({"confirmed":true}),
            ),
            (
                "confirm",
                json!({"answer":"no"}),
                json!({"confirmed":false}),
            ),
            (
                "confirm",
                json!({"answer":"other"}),
                json!({"cancelled":true}),
            ),
            ("input", json!({"answer":"text"}), json!({"value":"text"})),
            ("input", json!({}), json!({"value":""})),
            (
                "editor",
                json!({"answer":"first\nsecond"}),
                json!({"value":"first\nsecond"}),
            ),
            ("editor", json!({"answer":3}), json!({"cancelled":true})),
        ] {
            let request = json!({"method":method,"id":"ask-1","options":["Allow","Block"]});
            let action =
                serde_json::from_value(json!({"action":"accept","content":content})).unwrap();
            let mut expected = expected;
            expected["type"] = json!("extension_ui_response");
            expected["id"] = json!("ask-1");
            assert_eq!(
                response(&request, Some(action)),
                expected,
                "{method}: {content}"
            );
        }
    }

    #[test]
    fn unanswered_dialogs_are_cancelled() {
        for action in [
            Some(ElicitationAction::Decline),
            Some(ElicitationAction::Cancel),
            None,
        ] {
            assert_eq!(
                response(&json!({"id":"ask-1","method":"input"}), action),
                json!({"type":"extension_ui_response","id":"ask-1","cancelled":true})
            );
        }
    }
}
