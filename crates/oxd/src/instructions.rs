//! Ox's instructions to a session's agent.

/// The workflow step a run's child session runs.
pub struct StepRole {
    pub workflow: String,
    pub step: String,
}

/// Ox's instructions to a session's agent: how to use the `ox` tools when
/// the agent has them, and a workflow step's role. A session with neither
/// gets none.
pub fn text(step: Option<&StepRole>, ox_tools: bool) -> Option<String> {
    if step.is_none() && !ox_tools {
        return None;
    }
    let mut paragraphs = vec!["You are running in Ox, a desktop app for coding agents.".to_owned()];
    if ox_tools {
        paragraphs.push(
            "Use the `ox` tool `set_title` to give this session a short title once its task \
             is clear, and again if the task changes."
                .into(),
        );
    }
    if let Some(StepRole { workflow, step }) = step {
        paragraphs.push(format!(
            "You are running the step \"{step}\" of the workflow \"{workflow}\". Your final \
             reply is handed to the next step as its input."
        ));
    }
    Some(paragraphs.join("\n\n"))
}
