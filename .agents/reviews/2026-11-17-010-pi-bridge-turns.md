# Pi bridge turn coordination

## Scope and coverage

Reviewed the changed bridge, turn coordinator, protocol tests, and
documentation against the Pi structure task and its two linked bugs.
Traced ACP input dispatch, oxd's handling of prompt and config answers,
and Pi 1.5.2's RPC and run lifecycle. [Plan](../plans/2026-11-17-010-pi-bridge-turns.md)
· [Work log](../work/2026-11-17-009-pi-bridge-turns.md)

The full live smoke run used the builder's patch. The final revision was
checked with real Pi for the config response behavior changed in review.

## Summary

| ID | Severity | Category | Fix | Title |
| -- | -------- | -------- | --- | ----- |
| F1 | Medium | Correctness | Fixed | A later effort change can make an earlier one report failure |
| F2 | Low | Readability | Fixed | Waiting commands use a type that also admits queries and stops |
| F3 | Low | Testing | Fixed | Protocol tests miss two run and steering event boundaries |

## F1. A later effort change can make an earlier one report failure

Medium · Correctness · Fixed · `crates/pi-acp/src/turns.rs:110`

The composer's effort slider sends each change even while earlier ones
are pending.
When Pi answered the first change, the coordinator sent the next change
before the first ACP handler could query the resulting state. Pi's
`set_thinking_level` and `get_state` handlers are synchronous. The first ACP
handler could then see the second effort level and report that Pi refused
the first choice. oxd skips refused settings when saving config.

### Evidence

The protocol regression
`overlapping_effort_changes_each_report_the_level_they_applied` had Pi apply
`low` and then `high` after both requests reached the bridge. The first
request returned `Pi runs first/one at effort high, not low` on the builder's
revision and on `2cc5006`. The race predated the refactor. The new
coordinator always produced that order when the first change's answer
arrived with another change in its queue.

### Fix

The coordinator sends `get_state` as soon as Pi acknowledges a config
change, ahead of later config changes and prompts. Pi reads the state when
it receives the query, and later commands can proceed without waiting for
the query's answer. The ACP handler builds its response from that snapshot.
It previously also queried the command catalog and discarded the result.

## F2. Waiting commands use a type that also admits queries and stops

Low · Readability · Fixed · `crates/pi-acp/src/turns.rs:19`

The FIFO held `Pending` values with a config flag. To understand what could
wait, a reader had to check every enqueue site, then follow the flag through
dispatch, response handling, cancellation and exit. Queries and stops use a
direct dispatch path.

### Fix

`Ordered` names the two operations that can wait, prompts and config
changes. `Pending` records those operations alongside the queries and stops
that have already been sent.

## F3. Protocol tests miss two run and steering event boundaries

Low · Testing · Fixed · `crates/pi-acp/src/pi.rs:304`

Deleting `agent_start` passed the builder's tests because the active runs
they exercised also had a `started` prompt answer. An extension can start
its own run. With `agent_start` removed, a following `handled` answer ends
the turn in the bridge while the run continues in Pi. Broadening the user
`message_start` handler to other roles also passed. A tool-result message
could then accept a steer before Pi takes it in.

### Fix

The existing cancellation test now includes a run started by an extension
command. The existing steering test includes a tool-result message before
the steered user message. Both tests reject their respective mutations.

## Checks run

- `make check-rust` passed on the reviewed revision, including 13 Pi unit
  tests and 32 Pi protocol tests.
- The three new or expanded protocol regressions failed on detached
  `2cc5006` as expected. The model test sent a prompt before the model
  answer, the dialog test opened a second form, and the effort test reported
  the later effort on the earlier request.
- Diff-scoped mutation testing of the builder's revision checked 104
  mutants. It caught 50, 45 failed to build, five timed out, and four
  survived. Two survivors changed required behavior and are covered in F3.
  The other two changed stderr diagnostics for stop responses and removed
  an assistant block reset. The block map already drains at each
  `message_end` in Pi's normal message sequence. A focused run on the final
  revision caught both F3 mutants and both mutations of config response
  parsing.
- The work log records the standard smoke script stopping at the obsolete
  integer daemon handshake and the full Pi 1.5.2 smoke suite passing with
  the current handshake supplied in memory. A focused real-Pi probe on the
  final bridge submitted one model and two effort changes together. It
  received `low` and `high` in the respective effort responses without
  generating model output.

## Verdict

The bridge centralizes turn dispatch, holds prompts behind prior config
changes and cancellation, and declines late dialogs during cancellation.
The review fixed effort response attribution and covered the run events
that govern when cancellation and steering take effect.
