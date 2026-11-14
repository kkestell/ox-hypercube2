# Pi models, thinking levels, and commands

## Scope and coverage

Reviewed the staged changes in `crates/pi-acp/src/{bridge,commands,main,options,pi}.rs`
and `crates/pi-acp/tests/protocol.rs` against [the plan](../plans/2026-11-14-004-pi-models-thinking-commands.md)
and [task 4](../scratch/pi.md#4-models-thinking-levels-and-commands).
There is no separate brief. Read the complete changed files and traced their
callers through daemon option/catalog updates and composer model/command use.
Checked request correlation, exit settlement, startup and resume publication,
config changes, native skill names, and the complete protocol-test diff.

The review used scripted Pi processes at the ACP boundary. It did not send live
model prompts or drive the desktop. The implementer's live Pi 1.4.0 RPC check is
recorded in [the work log](../work/2026-11-14-004-pi-models-thinking-commands.md)
and was not repeated during this review. The plan's accepted limitations,
including pre-prompt preference persistence and concurrent config/prompt
ordering, remain unchanged.

## Summary

| ID | Severity | Category | Fix | Title |
| -- | -------- | -------- | --- | ----- |
| F1 | Medium | Correctness | Fixed | Config-triggered restart leaves the command catalog stale |

No High or Low findings.

## F1. Config-triggered restart leaves the command catalog stale

Medium · Correctness · Fixed · `crates/pi-acp/src/bridge.rs:338`

After Pi exits, a model or thinking change can start its replacement. That path
ignored whether `start_pi` created a new process and discarded the command catalog
queried after the change. Prompts then reused that process without publishing its
catalog. Skills added before the restart therefore remained absent from command
completion until another restart or resume, contrary to the planned refresh at
each Pi start.

### Evidence

The regression `a_config_change_restarts_pi_and_refreshes_its_commands` observes
Pi exit after accepting a prompt, replaces its fake catalog with `skill:new`, and
changes thinking level. With the original implementation the config request
succeeds but waiting for `available_commands_update` times out. With the fix it
receives the new skill. The replacement fake answers requests by type, without
requiring a particular description/config query order.

### Fix

When `set_config` starts Pi, it now publishes the startup description through the
same function used by new sessions and prompt-triggered restarts. This also
publishes startup catalogs if Pi subsequently rejects the requested change. The
existing post-change description still supplies the resulting config options.
The fix required no new behavior decision.

## Checks run

- Focused restart regression — failed without the fix because no command update
  arrived; passed with the fix.
- `cargo fmt --all --check` initially reported formatting in the new test;
  `cargo fmt --all` applied it before the required check.
- The first `make check-rust` failed in unchanged Claude protocol test
  `a_prompt_is_answered_when_it_enters_the_conversation`: its title notification
  was absent before the prompt response. The isolated test passed immediately;
  no Claude files were changed. Initial output:
  `/tmp/ox-pi-models-review-check-rust.log`.
- Final `make check-rust` — passed formatting, clippy with warnings denied,
  `ox-mcp` build, and all workspace tests, including five Pi unit tests and 19 Pi
  protocol tests. Output: `/tmp/ox-pi-models-review-check-rust-rerun.log`.
- `git diff --check` — passed.
- UI end-to-end and Claude smoke checks are not required for these Pi-only changes.

## Verdict

F1 is fixed and covered by a regression. The change meets task 4 and the approved
plan; no unresolved findings remain.
