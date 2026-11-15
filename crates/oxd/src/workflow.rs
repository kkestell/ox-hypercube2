//! Workflow definitions: YAML files in the daemon's workflows directory, each
//! a named sequence of agent steps.

use std::io;
use std::path::Path;

use ox_protocol::{ErrorBody, Harness, code};
use serde::Deserialize;

use crate::daemon::invalid_request;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<Step>,
}

/// An agent step: a prompt sent to a new child session.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub name: String,
    pub agent: AgentStep,
    pub prompt: String,
}

/// The harness a step runs, and the values of its config options. Each value
/// is the ID the harness's bridge offers for the option.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStep {
    pub harness: Harness,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
}

/// Reads the definition `<dir>/<workflow>.yaml`. Refuses a `workflow` that is
/// not a plain file name or starts with `.`, a file that is missing
/// (`NOT_FOUND`) or does not parse, and a workflow with no steps.
pub fn load(dir: &Path, workflow: &str) -> Result<Workflow, ErrorBody> {
    if workflow.starts_with('.') || Path::new(workflow).file_name() != Some(workflow.as_ref()) {
        return Err(invalid_request(format!(
            "{workflow:?} is not a workflow name"
        )));
    }
    let path = dir.join(format!("{workflow}.yaml"));
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ErrorBody {
                code: code::NOT_FOUND,
                message: format!("no workflow {workflow}"),
            });
        }
        Err(error) => {
            return Err(invalid_request(format!(
                "reading {}: {error}",
                path.display()
            )));
        }
    };
    parse(&text).map_err(|error| invalid_request(format!("{}: {error}", path.display())))
}

/// A definition's text as a workflow.
fn parse(text: &str) -> Result<Workflow, String> {
    let workflow: Workflow = serde_saphyr::from_str(text).map_err(|error| error.to_string())?;
    if workflow.steps.is_empty() {
        return Err("the workflow has no steps".into());
    }
    Ok(workflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_share_an_agent_through_an_alias() {
        let workflow = parse(
            "name: Plan and review
steps:
  - name: Plan
    agent: &claude
      harness: claude
      model: claude-opus-5-5
      effort: high
      permission_mode: manual
    prompt: |
      Write a plan.
  - name: Review
    agent: *claude
    prompt: Review it.
",
        )
        .unwrap();
        let claude = AgentStep {
            harness: Harness::Claude,
            model: Some("claude-opus-5-5".into()),
            effort: Some("high".into()),
            permission_mode: Some("manual".into()),
        };
        assert_eq!(
            workflow,
            Workflow {
                name: "Plan and review".into(),
                steps: vec![
                    Step {
                        name: "Plan".into(),
                        agent: claude.clone(),
                        prompt: "Write a plan.\n".into(),
                    },
                    Step {
                        name: "Review".into(),
                        agent: claude,
                        prompt: "Review it.".into(),
                    },
                ],
            }
        );
    }

    #[test]
    fn a_definition_without_steps_or_with_an_unknown_field_is_refused() {
        for (text, expected) in [
            ("name: Empty\nsteps: []\n", "no steps"),
            (
                "name: Checked
steps:
  - name: Plan
    agent: {harness: pi}
    prompt: Plan.
    postconditions: []
",
                "postconditions",
            ),
        ] {
            let error = parse(text).unwrap_err();
            assert!(error.contains(expected), "{text}: {error}");
        }
    }

    #[test]
    fn a_name_that_is_not_a_plain_file_name_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["../secret", "nested/plan", ".hidden", "", "/plan"] {
            let error = load(dir.path(), name).unwrap_err();
            assert_eq!(error.code, code::INVALID_REQUEST, "{name:?}: {error:?}");
        }
        assert_eq!(load(dir.path(), "plan").unwrap_err().code, code::NOT_FOUND);
    }
}
