# Fold a completed turn's work under "Worked"

## Plan

`agents/plans/2026-11-16-003-worked-groups.md`

## Summary

The core emits `TurnCompleted` for a turn that ends with `EndTurn` or no stop
reason and no error. The UI's `rows` folds a completed turn's shown work before
its text reply into a memoized `WorkedGroup`, closed by default; errored,
cancelled, exited and reply-less turns stay expanded. The plan's goal is met.

## Decisions

- The Worked toggle spans the transcript's width and carries the rule as its
  bottom border, a spacing unit below the label, like the full-width summaries
  of thoughts. Open work sits below it in `.worked-items`.
- The `turn_completed` item stays in the rows after the group as an item row;
  `ItemView` renders nothing for it.
- The mock's completed turn uses `thought:1`, `text:1`, `tool:1`, `tool:2`,
  `text:2` keys, and the running turn's items were rekeyed to `8`, `tool:3`
  and `text:3` so every key is unique.

## Checks run

- `cargo test -p ox conversation` — 14 passed.
- `make check-rust` — passes; 330 tests.
- `make check-ui` — passes.
- `make e2e` — first run: two spec files failed. The new Worked test's
  `group.$(".text")` matched the thought's Markdown before the mid-turn
  message; it now selects `.worked-items > .text`. Every
  `archive.e2e.ts` test failed to find its workspace section, and the spec
  passed on rerun with no change. Second full run: 14 of 14 spec files
  passed, including `stress.e2e.ts` with the long turn ending on `tool end`.

## Manual verification

1. The mock `sidebar` scenario's `Fix sidebar colors` session, closed and
   open, in headless Chrome at 1200×900.

   ```sh
   cd ui && npx vite --port 1431
   # open http://localhost:1431/mock/, select "Fix sidebar colors", click Worked
   ```

   Closed: prompt, a muted "Worked ›" over a full-width rule, the reply, then
   the running turn's prompt, tool call and text. Open: the chevron points
   down and the thought, mid-turn text and two tool calls sit below the rule
   with the transcript's spacing. The toggle's left edge matches the tool call
   rows (234 px); its label is the muted foreground at the transcript's font.
