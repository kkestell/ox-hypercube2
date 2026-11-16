# Agent messages in the run view

## Scope and coverage

The staged changes for `.agents/plans/2026-11-16-002-run-view-agent-messages.md`: the removal of `post_status_update` and everything that carried a status update (`ox-mcp`, `ox-protocol`, `oxd`, the fake agent, the desktop core's conversation items, the UI protocol types), the run view listing each step's agent messages as Markdown, and the matching docs, mock scenario, and e2e spec. The plan links no brief; the review checks the change against the plan's Goal, Decisions, and Check your assumptions. Callers checked beyond the diff: `RunView.tsx`, `useConversations.ts`, `Markdown.tsx`, `Conversation.tsx`'s `Transcript`, the conversation and base styles, and the fake agent's turn handling.

Gaps:

- The full parallel `make e2e` was not rerun; only `run.e2e.ts` was.
- Following stops once the user scrolls the message's end out of view. No e2e test covers that part; it uses the same scroll test as the session view's `Transcript`.

## Summary

| ID | Severity | Category    | Fix       | Title                                                        |
| -- | -------- | ----------- | --------- | ------------------------------------------------------------ |
| F1 | Medium   | Correctness | Fixed     | Run view stops following a message once it grows             |
| F2 | Low      | Readability | Not fixed | Dot misaligns when a message starts with a non-paragraph block |

## F1. Run view stops following a message once it grows

Medium · Correctness · Fixed · `ui/src/features/runs/RunStatus.tsx:102`

The run view revealed the active step's latest message through a callback ref, `revealMessage`. React calls a callback ref only when the element mounts or the ref changes. Status updates arrived whole, so that was enough. Agent messages stream in chunk by chunk under one key, so the `li` mounts on the first chunk and then grows without being revealed again. A message that grows past the bottom of the view, such as a step's long final reply, runs off-screen while the step runs. That contradicts the plan ("follows the latest while the step runs") and `docs/workflows.md`.

### Evidence

`useConversations` appends each chunk to the item under the same `text:<message id>` key, so the `li` keeps its key and its stable `revealMessage` ref. A new e2e test streams "Reading the code", then appends 60 paragraphs to the same message. Against the staged `RunStatus.tsx` it fails: the message's end is below the view.

### Fix

The user chose to follow the message until the user scrolls away, as the session view does. `useFollow` replaces `revealMessage`. Each change to the latest message scrolls the run view so the message's end is in view, as long as the view is following. A scroll that leaves the end out of view stops following. A new latest message starts following again. `docs/workflows.md` now describes this. The new e2e test "keeps the end of the active step's growing message in view" covers the growth case.

## F2. Dot misaligns when a message starts with a non-paragraph block

Low · Readability · Not fixed · `ui/src/styles/run-view.css:116`

`.run-status-messages p` sets the line height that lines a message's first line up with its dot. A message that starts with a heading, list, or code block has a different first-line height, so its dot sits a pixel or more off that line. This is cosmetic and depends on how the agent formats the message. Left unchanged.

## Checks run

- `make check-ui`: passed.
- `make check-rust`: passed.
- `npx wdio run e2e/wdio.conf.ts --spec e2e/specs/run.e2e.ts` on a fresh debug build: 6/6 passed, including the new growth test.
- The new growth test against the staged `RunStatus.tsx` (temporarily restored, then reverted): failed as expected.

## Verdict

The removal is complete and consistent across the daemon, protocol, desktop core, UI, and docs. F1 is fixed as the user decided. F2 is recorded and left unchanged.
