# Clients state intent; oxd sequences session loads and config

## Plan

`.agents/plans/2026-11-17-008-session-intent-operations.md`

## Summary

`focus` now loads an exited, unarchived session that has started, and `session.load` is gone from the protocol, client, daemon and Tauri app. `session.set_config` sets only the settings it names. The session composer shows a pick at once and sends only what the pick changed. The goal is met.

## Departures from the plan

- `scripts/smoke-acp.py` called `session.load`; it now sends `focus` with the same params. The plan did not name it.
- The plan has the archived-session cases (`worktrees.rs` archived check) wait for a logged refusal. Focus spawns nothing for an archived session, so nothing is logged; those tests focus the session and assert it stays `exited`. The missing-directory and branch-move cases wait on the log as planned.
- `ModelPicker`'s `choose` skips a pick equal to the shown model, which `useChosen` did before it was deleted.

## Decisions

- `settle_config` removes the effort only when the answers hold no `effort` at all, refused or not.
- `common::load` stays focused on the session after it returns.
- A model pick always records `effort: null` in `picked`; the clearing effect drops it at once when the config has no effort.

## Checks run

- `cargo fmt --check`, `cargo clippy --all-targets` — clean.
- `cargo test -p oxd -p ox-protocol -p ox` — pass (`ox-mcp` rebuilt first, so the oxd agent tests see the current protocol).
- `npm run check` and `npx vitest run` in `ui/` — pass.

## Follow-up work

- Focus is the only trigger for loading a saved session, so a focused session whose load was refused during a branch move loads on the next focus.
