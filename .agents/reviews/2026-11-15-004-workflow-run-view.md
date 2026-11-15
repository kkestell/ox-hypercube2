# Review of the desktop run view

## Scope and coverage

The staged changes for `.agents/plans/2026-11-15-004-workflow-run-view.md`: `WorkflowInfo`, `workflow.list`, and `created_at` in `ox-protocol`, the store, and the daemon; `oxctl run ls`; the desktop core's per-channel conversation views and run commands in `crates/ox/src/lib.rs`; `useConversations`, `Transcript`, `RunView`, the run and child rows, the New Task menu's workflows, `StartRunDialog`, the child composer, the file tree and viewer keyed by view ID, and the styles; the e2e fixture, helpers, and `run.e2e.ts`; and the workflow, architecture, glossary, and performance docs. Checked against the brief `.agents/briefs/2026-11-14-001-user-defined-workflows.md`, delivery task 4, and the plan's decisions. The change delivers the task's outcome: each Git workspace's New Task menu lists the workflows, the run view shows each started step's checks and child conversation, a child's request can be answered from either view, and a child takes prompts once its run has ended.

`stepTimeline` was traced against `run.rs`: `checks` stops at the first failing command, and the repair turn reruns the postconditions from the first, so the timeline's order matches the commands the runner records. The tree lines were compared with `Branch` in `~/src/hypercube/internal/ui/web/sidebar/session-item.tsx`: the line is centred on the parent's dot, the first child's line starts just below that dot, and each stroke ends at the edge of the child's dot.

Gaps:

- The live smoke suites were not run. The change does not touch the bridges, how `oxd` launches agents or exchanges ACP messages with them, or the smoke script.
- The sidebar and run view screenshots were not compared by eye with hypercube's screenshot. The comparison above is of the CSS geometry.

## Summary

| ID | Severity | Category      | Fix       | Title                                                 |
| -- | -------- | ------------- | --------- | ----------------------------------------------------- |
| F1 | Low      | Comments      | Not fixed | `now` took the doc comment of `resolved`              |
| F2 | Low      | Documentation | Not fixed | A sentence added to the architecture doc is unwrapped |

## F1. `now` took the doc comment of `resolved`

Low · Comments · Not fixed · `crates/oxd/src/daemon.rs:1362`

`now` was inserted between `resolved` and its doc comment. `now`'s rustdoc now begins with "`path` with symbolic links resolved, or as it is when it cannot be.", which describes `resolved`, and `resolved` has no doc comment.

### Fix

Not fixed.

## F2. A sentence added to the architecture doc is unwrapped

Low · Documentation · Not fixed · `docs/dev/architecture.md:185`

The "Workflow runs" paragraph gained "a session is a child of the run that owns its worktree." without rewrapping, which leaves a 127-character line in a file wrapped at 80 columns.

### Fix

Not fixed.

## Checks run

- `make check`: passed.
- `make e2e`: 13 of 13 spec files passed, including `run.e2e.ts` and the stress spec.

## Verdict

The change delivers task 4 as the brief and plan describe, with no High or Medium findings. F1 and F2 are Low and left unchanged.
