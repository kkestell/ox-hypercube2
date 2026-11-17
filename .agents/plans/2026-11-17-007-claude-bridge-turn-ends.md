# The Claude Code bridge ends a turn the same way however Claude ends it

## Goal

Close "Cancelling during a preempted turn can leave it open" (`.agents/todo.md`, Claude Code bridge) and the Claude Code half of the Structure item on bridges inferring turns, with the smallest change to `crates/claude-code-acp/src/runner.rs`.

## Decisions

- **Preemption handling goes.** Claude Code sends `turn_preempted` only to a client whose initialize request declares `rapidFollowupPreempt` (Claude Code 2.1.326's schema: "without both, the turn is not stopped at all"). The bridge does not declare it. `Output::preempted`, `continued` and the `turn_preempted` arm are removed, with the protocol test that scripted the frame.
- **`idle` during a turn ends it as a result does.** `close_turn` withdraws open dialogs, fails unfinished tool calls and clears the streamed message and thought. `result` and an `idle` that arrives while working both call it.
- **A cancel ends with its turn.** An `idle` during a turn clears `cancelling`, so a later turn's error is not reported as cancelled. Claude's schema describes `idle` as firing after the result is flushed, so a cancelled turn's result still sees the cancel.

## Test plan

`idle_without_a_result_ends_the_turn_and_its_cancel`: a cancelled turn that Claude ends with `idle` and no result fails its open tool call, and the next turn's error is reported as that error. It fails on the previous runner. The existing protocol tests pass unchanged.
