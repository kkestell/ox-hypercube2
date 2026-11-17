# Automatic session and workflow-run titles

## Problem

Session naming currently depends on the coding agent, while workflow runs use their input as a title. The user wants a separately configured harness, model, and effort level to name new sessions and workflow runs consistently without spending coding-agent turns on naming.

## Outcome

The user can choose one app-wide harness, model, and effort level for automatic titling. New standalone sessions and workflow runs receive a short title generated from their initial request. Workflow step sessions keep their step names.

## Decisions

- **Replace agent-driven titling.** The coding agent no longer names or retitles its session; a separate title generator handles naming.
- **Use one app-wide configuration.** Session and workflow-run titling share the same harness, model, and effort selection, rather than separate workspace settings.
- **Require the completion extension over ACP.** Only harnesses that support our extension are eligible. Title generation must run without tools or conversation persistence; direct harness invocation is not the chosen interface.
- **Require explicit configuration.** Automatic titling stays off until the user chooses a harness, model, and effort level. Do not silently choose a provider or model that could incur charges.
- **Generate once without blocking work.** Titling must not delay session or workflow-run execution. Preserve manual renames; do not overwrite them with a generated title.
- **Fall back to the initial request text.** Use it while automatic titling is unconfigured or pending, and retain it if generation fails.
- **Keep workflow step names.** Generate a title only for the overall workflow run, not its step sessions, so the workflow structure remains clear.

## Out of scope

- Fixing Pi slash-command autocomplete.
- Automatic retitling as a conversation evolves.
- Per-workspace titling configuration.
