# Review: fold a completed turn's work under "Worked"

## Scope and coverage

I reviewed the staged changes for `.agents/plans/2026-11-16-003-worked-groups.md`: the new `Item::TurnCompleted` in `crates/ox/src/conversation.rs` and its tests, the `turn_completed` protocol type, `shown`, `rows` and `WorkedGroup` in `ui/src/features/conversation/Conversation.tsx`, the Worked styles, the mock scenario, the e2e specs, and the README and performance notes. I traced the new item through `coalesce` and the conversation stream in `crates/ox/src/lib.rs`, the UI's keyed item map in `useConversations.ts`, the transcript's follow-the-end scrolling, and the run view's agent-message filter. I checked the change against the brief `agents/briefs/2026-11-16-003-worked-groups.md`. It delivers the Outcome, keeps each Decision, and leaves the run view and durations alone.

Gaps:

- I did not rerun the e2e suite or view the mock scenario. The work log records both.

## Summary

| ID | Severity | Category | Fix       | Title                                                      |
| -- | -------- | -------- | --------- | ---------------------------------------------------------- |
| F1 | Low      | Testing  | Not fixed | New turn-end table test repeats the existing error cases   |

## F1. New turn-end table test repeats the existing error cases

Low · Testing · Not fixed · `crates/ox/src/conversation.rs:747`

`a_completed_turn_is_marked` covers five turn ends. Only two of them, `EndTurn` and no stop reason, test the new behavior. The other three (cancelled, an error stop reason, and an error string) are already covered by `a_turn_ends_as_an_error_with_its_details` at `crates/ox/src/conversation.rs:788`. As a result, the turn-end mapping is split across two tests that overlap. Behavior is unaffected.

### Fix

Not fixed. Low findings are recorded only. A later fix would cut the new test down to the two completed cases, or add those two cases to the existing test.

## Checks run

- `cargo test -p ox --lib conversation`: 14 passed.
- `make check-ui` (Biome, `tsc`, `tsc -p e2e`): passed.
- Traced `rows` by hand for a turn with only a reply, a turn with only blank work before the reply, a turn ending in a not-shown permission item after the reply, an empty turn, and a turn ending on a tool call. Each matches the plan.

## Verdict

The change does what the plan and brief require, and I found no correctness problems. The only finding, F1, is a Low test overlap, recorded and not fixed.
