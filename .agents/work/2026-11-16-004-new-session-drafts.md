# New sessions start on first send

## Plan

`.agents/plans/2026-11-16-004-new-session-drafts.md`

## Summary

New Session opens a draft that lives only in the desktop app's local storage. Sending its first message calls `session.new` with the prompt and the shown option values. The daemon records `PromptStarted`, titles the session from the prompt, and then starts the worktree and agent. Once the agent is up it sets the options and sends the prompt. Both bridges have a `--describe <dir>` mode. The desktop core runs it as `agent_controls`, and the UI caches the result per agent and workspace. The plan's goal is met.

## Departures from the plan

- `crates/ox/Cargo.toml` gains tokio's `process` feature, which `agent_controls` needs to run the bridge.
- The Claude probe does not go through `Claude::start`, which needs an ACP-bound `Updates` and has no way to wait for the initialize answer. Instead, `runner.rs` splits out `spawn`, `initialize_request` and `initialized`, which both `Claude::start` and the new `runner::commands` use. `StartRequest`'s `session_id`/`resume` fields become `Conversation::{New, Resume, Unsaved}`.
- `crates/pi-acp/src/updates.rs` (not in the plan) makes `Updates`' connection optional, because `Pi::start` needs an `Updates` and `--describe` has no ACP client. Bridge-side, `Bridge::describe`'s queries moved into a free `description(pi)`.
- `useAgents` moved from `SessionList` to `App`, which passes `agents` down, because `useControls` needs the installed agents too.
- `useDrafts` and `useUnsent` depend on each other: one needs the draft IDs to prune, the other needs the unsent text to drop empty drafts. `useDrafts` therefore exposes `dropEmpty(viewed, empty)`, and an effect in `App` calls it whenever the viewed ID or the unsent text changes. The plan called for this only when the viewed ID changes, but the result is the same.
- `useLastUsed` also takes `parents` so that it can skip run children. It reads values only from the newest session per agent and workspace, and only once that session reports options. This way an older session never replaces the values a draft was just sent with.
- `Composer` takes an extra `optionsDisabled` prop, which disables the pickers of a session that is still starting.
- `Composer.test.tsx` needed no change. The UI test core answers `agent_controls` with empty controls by default.

## Decisions

- `add_session` and `start_in_worktree` take the `Opening` with `#[expect(clippy::too_many_arguments)]`, as `save_session` already does. `add_session` takes the first prompt separately and fills `Opening.prompt` with the seq that `queue_prompt` returns.
- `oxd` has its own `prompt_text`, the text blocks joined by newlines, matching the bridges' helpers, and feeds it to `acp_bridge::title`.
- Draft IDs are `draft-<uuid>`. `draftTitle` collapses whitespace and falls back to "New session". The sidebar gets draft titles as a map memoized on their JSON, so typing in a session's composer re-renders neither the sidebar nor its run rows.
- Delete Draft on the viewed draft also clears the view.
- A probe that never answers holds the probe queue. This is a `TODO` in `controls.ts`.

## Checks run

- `make check-rust`: passed.
- `make check-ui`: passed, 69 tests.
- `make check-scripts`: passed.
- `make e2e`: passed, 4 spec files.
- `make smoke-claude`: passed, including `describe_lists_options_and_commands`.
- `make smoke-pi`: passed, including `describe_lists_options_and_commands`.

## Manual verification

1. A describe probe against the real harnesses, through the smoke script's `check_describe`. Each run printed a model option and the `ox-command-probe` command.

   ```sh
   make smoke-claude && make smoke-pi
   ```

## Follow-up work

- Cached controls of removed workspaces stay in local storage.
- A never-started session's first prompt is not shown after a daemon restart. This is a `TODO` in `daemon.rs` restore.
