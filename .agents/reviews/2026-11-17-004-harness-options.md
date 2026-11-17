# Harnesses, models, and effort levels from a config file

## Scope and coverage

The staged changes implementing `.agents/plans/2026-11-17-003-harness-options.md`: the `ox_protocol::config` loader and example config, `make install`, the `session.request` checks and `SessionRequest` permission payload in `oxd`, the first prompt waiting on a new session's settings, the Claude and Pi bridges no longer listing or describing, `ox-mcp`'s `list_harnesses` and `start_session`, the desktop core's `agents`, the UI's `draftOptions` and `sessionOptions`, the request panel, tests, and docs. Checked against the brief `.agents/briefs/2026-11-17-002-harness-options.md`, with the plan's changed Check your assumptions entries (example config contents, refused settings failing the first prompt, a mode in requests) taking precedence. The staged diff also carries the worktree's earlier uncommitted mock-UI and request-panel work, which was reviewed as part of the request panel.

Gaps:

- `make e2e` and the Claude and Pi smoke runs were not rerun; the work log records them passing, and no code they exercise changed in the review.
- The request panel and pickers were not checked on screen.

## Summary

| ID | Severity | Category    | Fix       | Title                                                            |
| -- | -------- | ----------- | --------- | ---------------------------------------------------------------- |
| F1 | Medium   | Correctness | Fixed     | Running session's unlisted effort or mode shows as another value |
| F2 | Low      | Intent      | Fixed     | Request panel omits the model's provider                         |
| F3 | Low      | Readability | Fixed     | Unwrapped doc comment and dropped blank line                     |

## F1. Running session's unlisted effort or mode shows as another value

Medium · Correctness · Fixed · `ui/src/features/composer/controls.ts:197`

`sessionOptions` offers a running session's effort levels from the config model the session reports, at the reported value. When that value is not one of the model's listed levels, the effort slider gets an index of -1: the browser puts its thumb on the first level and the picker button shows no effort at all. A session's mode not in the config shows the same way, because the native select falls back to its first option, so a session in an unlisted mode appears to be in the first listed mode.

Normal use reaches this. With the example config, switching a running Claude session from Opus 5.5 at High to Haiku 5.5 (Low, Medium) leaves Claude at `high`. The picker then shows Haiku with the slider at Low, while Claude still runs at High. The plan's rule is that a reported value the config does not list shows as its ID, which held only for the model.

### Evidence

A new test in `Composer.test.tsx` had a session report model `deep` (High, Max), effort `medium`, and mode `plan`. Before the fix, the picker read `Deep`, with no effort shown.

### Fix

`sessionOptions` adds a reported effort or mode that the listed choices lack to the end of those choices, named by its ID. It does this only when the config lists some choices, so a model or harness with none still has no picker. The test now expects the picker to read `Deepmedium`, the slider to sit on that value, and the Mode select to show `plan`.

## F2. Request panel omits the model's provider

Low · Intent · Fixed · `ui/src/features/composer/Permission.tsx:72`

The brief says the request the user approves shows the model and effort "as the composer shows them". The composer's model picker shows a model's provider beside its name, for example `GPT-6 Luna OpenAI High`. `SessionRequest` carries only the model's name, so the panel shows `GPT-6 Luna High`. Every model name in the example config is unique, so nothing is ambiguous today. Fixing it means adding the provider to `SessionRequest` and to the panel.

### Fix

`SessionRequest` carries the model's `provider` from the config, in `ox-protocol`, the daemon's `chosen`, and the UI type. The panel shows it beside the model name, dimmed like the model picker's provider. The panel test expects `Claude CodeOpusAnthropicHighManual`.

## F3. Unwrapped doc comment and dropped blank line

Low · Readability · Fixed · `ui/src/features/composer/DraftComposer.tsx:22`

In the `DraftComposer` doc comment, the new sentence and the old one share a single line of about 130 characters, where the surrounding comment wraps at 80. In `ui/src/styles/composer.css:286`, the change dropped the blank line between the `.request-panel .description` rule and the comment that follows it.

### Fix

The doc comment wraps at 80 columns, and the blank line is back.

## Checks run

- `make check` on the staged changes before fixing: passed (fmt, clippy, build, workspace tests, UI check and tests, ruff).
- `npx vitest run src/features/composer/Composer.test.tsx` with the new test before the fix: failed as F1 describes. After the fix: passed.
- `make check-ui` after the fix: passed (83 tests).
- Pi 1.5.2's `get_state` and thinking-level clamping, read from its installed source, to confirm Pi always reports a thinking level and that the example config's effort levels are ones its models support.

## Verdict

The change does what the plan and brief ask for. Ox reads what it offers from its config, requests are checked against the config, and refused settings fail the first prompt. F1, F2, and F3 are fixed.
