# Pi smoke check and docs

## Scope and coverage

Reviewed the staged change against [Pi smoke check and docs](../plans/2026-11-14-006-pi-smoke-and-docs.md)
and task 6 of the [Pi task list](../scratch/pi.md#6-smoke-check-and-docs).
The user's accepted OpenRouter model overrides the task list's earlier model.
Read the complete smoke script, Makefile, README, ACP chart, architecture, and
testing guide. Traced the chart's new Pi claims through the bridge sources and
checked the implementation's saved live artifacts.

The shared run and concrete Claude/Pi classes keep the lifecycle checks together
while giving each harness its own checks. Both Make targets select their agent
explicitly, direct invocations retain Claude as the default, and the testing
guide remains the authoritative home for required check commands.

Coverage limits:

- Did not repeat the implementer's passing workspace, script, or live checks;
  no implementation changed during review. The work log records those results,
  and both live artifact directories were inspected independently.
- Live results establish compatibility with the selected installed harnesses
  and models. They do not exercise other providers or every extension dialog
  type; Pi's live form check exercises `select`.

## Summary

| ID | Severity | Category | Fix | Title |
| -- | -------- | -------- | --- | ----- |
| F1 | Low | Documentation | Not fixed | Native replay chart overstates Pi write diffs |

## F1. Native replay chart overstates Pi write diffs

Low · Documentation · Not fixed · `docs/dev/acp-v2.md:33`

The `session/resume` caveat says Pi's native replay includes `edit`/`write`
diffs. Historical `write` calls replay their input and result, but no diff:
the bridge does not know whether the file existed before the recorded write.
The chart therefore promises more history content than the bridge supplies.
Live write updates are unaffected.

### Evidence

`crates/pi-acp/src/history.rs:73` passes `None` as the historical existence
argument to `tools::diff`. In `crates/pi-acp/src/tools.rs:102`, `write` produces
a diff only for `Some(true)` or `Some(false)`; `None` takes the final branch
and returns no diff. A `session/resume` with `replayFrom: start` reaches this
path for a successful write recorded in the native transcript. This conclusion
is established by code inspection, rather than a reproduced runtime failure.
Historical edits do produce diffs.

### Disposition

Left unchanged under the user's instruction to record Low findings without
fixing them. A documentation correction can describe edit diffs in native
replay without adding historical write reconstruction.

## Checks run

- AST comparison against `HEAD`: 21 existing shared/Claude methods are
  unchanged. The concatenated bodies of `check_cancel_permission` and
  `check_cancel_generation` exactly preserve the old cancellation method.
- Inspected `/private/tmp/ox-acp-mocbo6gk/results.json`: 23 reports, ending in
  `passed`, including the original Claude-specific checks.
- Inspected `/private/tmp/ox-acp-rfp43rt2/results.json`: 18 reports, ending in
  `passed`, with exact model
  `openrouter/deepseek/deepseek-v4.1-flash-1017`, skill argument marker,
  extension form answer, cancellation, restart, parking, and archive checks.
- Inspected Pi's raw `events.jsonl`: the elicitation occurs identically four
  times, covering live delivery, reattach, daemon restart, and archive replay.
  The first inspection assumed three occurrences and failed its count
  assertion; inspecting the fourth identified archive replay, and the corrected
  equality check passed. This was an inspection assumption, not a smoke failure.
- Traced all new ACP chart support claims through Pi's request handlers,
  configuration, commands, content, dialogs, updates, tools, and native history.
- Reviewed the implementer's [work log](../work/2026-11-14-006-pi-smoke-and-docs.md)
  recording passing `make check-rust`, `make check-scripts`, `make smoke-claude`,
  and `make smoke-pi`. Those commands were not rerun during review.

## Verdict

No High or Medium findings. The implementation meets the requested smoke and
documentation scope; F1 is a Low documentation inaccuracy left unchanged as
instructed. No implementation fixes were needed.
