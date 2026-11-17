# New sessions start on first send

## Scope and coverage

I reviewed the staged changes for `.agents/plans/2026-11-16-004-new-session-drafts.md`: the `session.new` prompt and config fields, the daemon's queued first prompt and its state transitions, the opening options and prompt in the agent task, both bridges' `--describe` modes, the desktop core's `agent_controls` and `prompt_blocks`, and the UI's drafts, unsent text, controls cache, last-used values, composer split, draft view and sidebar rows, with their tests and documentation. I checked the change against the brief `.agents/briefs/2026-11-16-003-new-session-drafts.md` and the plan's Check your assumptions decisions. It delivers the brief's Outcome and keeps its Decisions.

Gaps:

- I did not run `make e2e`, `make smoke-claude` or `make smoke-pi`. The work log reports them passing.
- I did not run the desktop app.

## Summary

| ID | Severity | Category    | Fix       | Title                                              |
| -- | -------- | ----------- | --------- | -------------------------------------------------- |
| F1 | Low      | Performance | Not fixed | A failed controls probe runs again on any new entry |
| F2 | Low      | Comments    | Not fixed | `now`'s doc comment sits on `prompt_text`          |

## F1. A failed controls probe runs again on any new entry

Low · Performance · Not fixed · `ui/src/features/composer/controls.ts:60`

The effect that probes each installed agent and workspace pair with no entry depends on `controls`. A failed probe leaves its pair without an entry, so every later change to `controls` probes that pair again. Each successful probe changes `controls`, including the background probe that runs each time a draft is viewed. A pair whose probe always fails, such as one whose workspace directory was deleted, is therefore probed again on every draft view and after every other successful probe.

### Evidence

`probe` removes the pair from `probing` in `finally` and stores nothing on failure. `setControls` always makes a new object on success, which reruns the effect at `controls.ts:60`, whose loop calls `probe` for every pair not `in controls`.

### Fix

Not fixed.

## F2. `now`'s doc comment sits on `prompt_text`

Low · Comments · Not fixed · `crates/oxd/src/daemon.rs:1284`

`prompt_text` was inserted between `now`'s doc comment and `now`. `prompt_text` now carries both doc lines, "The current time in milliseconds since the Unix epoch." and its own, and `now` has none.

### Fix

Not fixed.

## Checks run

- `make check-rust`: passed (format, clippy with `-D warnings`, all workspace tests).
- `make check-ui`: passed, 69 tests in 8 files.

## Verdict

The change meets the plan and the brief. The daemon records the first prompt before the agent or worktree starts and ends its turn when either fails, the bridges' describe modes and the controls cache give drafts their pickers and commands, and drafts and unsent text persist as the brief asks. F1 and F2 are minor and left as they are.
