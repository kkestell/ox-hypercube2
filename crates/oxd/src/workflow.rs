//! Workflow definitions: YAML files in the daemon's workflows directory, each
//! a named sequence of agent and shell steps, each step with optional
//! preconditions and postconditions.

use std::io;
use std::path::Path;

use ox_protocol::{ErrorBody, Harness, WorkflowInfo, code};
use serde::Deserialize;

use crate::daemon::invalid_request;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<Step>,
}

/// A step: its action, with the checks that run before and after it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "StepFields")]
pub struct Step {
    pub name: String,
    pub action: Action,
    pub preconditions: Vec<Check>,
    pub postconditions: Vec<Check>,
}

/// What a step runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A prompt sent to a new child session.
    Agent { agent: AgentStep, prompt: String },
    /// A command run with `/bin/sh -c` in the run's checkout.
    Shell { command: String },
}

/// A shell command that passes when it exits with status 0.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub shell: String,
}

/// A step as the definition gives it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepFields {
    name: String,
    agent: Option<AgentStep>,
    prompt: Option<String>,
    shell: Option<String>,
    preconditions: Option<Vec<Check>>,
    postconditions: Option<Vec<Check>>,
}

impl TryFrom<StepFields> for Step {
    type Error = String;

    fn try_from(fields: StepFields) -> Result<Self, String> {
        let action = match (fields.agent, fields.prompt, fields.shell) {
            (Some(agent), Some(prompt), None) => Action::Agent { agent, prompt },
            (None, None, Some(command)) => Action::Shell { command },
            _ => {
                return Err(format!(
                    "step {} needs either agent and prompt, or shell",
                    fields.name
                ));
            }
        };
        Ok(Self {
            name: fields.name,
            action,
            preconditions: fields.preconditions.unwrap_or_default(),
            postconditions: fields.postconditions.unwrap_or_default(),
        })
    }
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

/// The definitions in `dir`: every `*.yaml` file whose name does not start
/// with `.`, sorted by file name, each with its `name`, or with the error
/// `load` gives for it. A missing directory lists nothing.
pub fn list(dir: &Path) -> Vec<WorkflowInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut workflows: Vec<String> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "yaml" {
                return None;
            }
            let workflow = path.file_stem()?.to_str()?;
            (!workflow.starts_with('.')).then(|| workflow.to_owned())
        })
        .collect();
    workflows.sort();
    workflows
        .into_iter()
        .map(|workflow| match load(dir, &workflow) {
            Ok(definition) => WorkflowInfo {
                workflow,
                name: definition.name,
                error: None,
            },
            Err(error) => WorkflowInfo {
                name: workflow.clone(),
                workflow,
                error: Some(error.message),
            },
        })
        .collect()
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

    fn check(shell: &str) -> Check {
        Check {
            shell: shell.into(),
        }
    }

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
                        action: Action::Agent {
                            agent: claude.clone(),
                            prompt: "Write a plan.\n".into(),
                        },
                        preconditions: Vec::new(),
                        postconditions: Vec::new(),
                    },
                    Step {
                        name: "Review".into(),
                        action: Action::Agent {
                            agent: claude,
                            prompt: "Review it.".into(),
                        },
                        preconditions: Vec::new(),
                        postconditions: Vec::new(),
                    },
                ],
            }
        );
    }

    #[test]
    fn shell_steps_and_checks_parse() {
        let workflow = parse(
            "name: Checked
steps:
  - name: Build
    shell: make
    postconditions:
      - shell: test -f out
  - name: Plan
    agent: {harness: pi}
    preconditions:
      - shell: test -z \"$(git status --porcelain)\"
      - shell: |
          set -eu
          true
    prompt: Plan.
    postconditions:
      - shell: test -n plan
",
        )
        .unwrap();
        assert_eq!(
            workflow.steps,
            [
                Step {
                    name: "Build".into(),
                    action: Action::Shell {
                        command: "make".into()
                    },
                    preconditions: Vec::new(),
                    postconditions: vec![check("test -f out")],
                },
                Step {
                    name: "Plan".into(),
                    action: Action::Agent {
                        agent: AgentStep {
                            harness: Harness::Pi,
                            model: None,
                            effort: None,
                            permission_mode: None,
                        },
                        prompt: "Plan.".into(),
                    },
                    preconditions: vec![
                        check(r#"test -z "$(git status --porcelain)""#),
                        check("set -eu\ntrue\n"),
                    ],
                    postconditions: vec![check("test -n plan")],
                },
            ]
        );
    }

    #[test]
    fn a_definition_without_steps_or_with_an_unknown_field_is_refused() {
        let either = "step Plan needs either agent and prompt, or shell";
        for (text, expected) in [
            ("name: Empty\nsteps: []\n", "no steps"),
            (
                "name: Retried
steps:
  - name: Plan
    agent: {harness: pi}
    prompt: Plan.
    retries: 2
",
                "retries",
            ),
            (
                "name: Both
steps:
  - name: Plan
    agent: {harness: pi}
    prompt: Plan.
    shell: make
",
                either,
            ),
            (
                "name: Neither
steps:
  - name: Plan
    postconditions: []
",
                either,
            ),
            (
                "name: Prompt only
steps:
  - name: Plan
    prompt: Plan.
",
                either,
            ),
            (
                "name: Timed check
steps:
  - name: Build
    shell: make
    postconditions:
      - shell: test -f out
        timeout: 5
",
                "timeout",
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

    #[test]
    fn list_gives_each_definition_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let steps = "steps:\n  - name: Build\n    shell: make\n";
        std::fs::write(
            dir.path().join("b-plan.yaml"),
            format!("name: Plan\n{steps}"),
        )
        .unwrap();
        std::fs::write(dir.path().join("a-broken.yaml"), "name: Broken\n").unwrap();
        std::fs::write(
            dir.path().join(".hidden.yaml"),
            format!("name: Hidden\n{steps}"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("notes.yml"),
            format!("name: Notes\n{steps}"),
        )
        .unwrap();
        let workflows = list(dir.path());
        assert_eq!(
            workflows
                .iter()
                .map(|info| (info.workflow.as_str(), info.name.as_str()))
                .collect::<Vec<_>>(),
            [("a-broken", "a-broken"), ("b-plan", "Plan")]
        );
        let error = workflows[0].error.as_deref().unwrap();
        assert!(
            error.contains("a-broken.yaml") && error.contains("steps"),
            "{error}"
        );
        assert_eq!(workflows[1].error, None);
        assert!(list(&dir.path().join("missing")).is_empty());
    }
}
