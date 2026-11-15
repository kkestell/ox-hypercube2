# Add the desktop run view

## Plan

`.agents/plans/2026-11-15-004-workflow-run-view.md`

## Summary

Each workspace's New Task menu lists the workflows from `workflow.list` and starts a run with an input. The sidebar shows each run among the workspace's sessions in `created_at` order, with its child sessions beneath it on hypercube's tree lines. The run view shows each started step's checks, commands and child conversation, and takes answers to the children's requests. A child's composer cancels the run while it runs and takes prompts once it has ended. The plan's goal is met.

## Departures from the plan

- The e2e fixture also links `oxctl` and `oxd-fake-agent` into `target/debug/e2e-bin/`, and `binary()` resolves there. An `oxctl` call that finds no daemon starts `oxd` from beside itself, so an `oxctl` run from `target/debug` could start a daemon whose `pi-acp` is the real bridge, for example after a spec stops the daemon.
- `make e2e` also builds `claude-code-acp`, which the fixture links.
- The development databases (`target/dev/oxd.sock.db*`, `~/.local/share/ox/oxd.sock.db*`) were already absent, so the schema reset deleted nothing.
- Rows carry an `exited` class, so the e2e selectors that matched `@class="sidebar-row session"` exactly now use `contains(@class, "session")`.
- `rowAction` returns whether each menu item was enabled, which the stuck-session test in `archive.e2e.ts` uses in place of `sessionAction`'s `archiveDisabled` argument.

## Decisions

- A child row's stroke runs to the child's dot, as hypercube's `Branch` does, not to the edge of its status box. The geometry uses a new `--sidebar-row-gap` token, the gap `.sidebar-row .select` already had.
- `childStatus` takes `session | undefined`, so a shell step's heading in the run view uses the same function. A pending step's status is grey with the label "Pending"; neither view shows one.
- `CommandView` has its own status labels ("Succeeded", "Cancelled", "Interrupted") rather than `ToolStatus`'s "Completed". Its details are a "Command" section with the whole command, an "Exited with status <n>" section, and the output section.
- A request panel with no entry below it (`.request-panel:last-child`) has all four corners rounded and no negative margin, so the run view's request stands on its own.
- `Transcript` scrolls a followed view after each of its renders instead of taking a dependency, since both callers render it only when their content changes.
- Workflow menu items appear only for a workspace in a Git working tree, and the separator only when there is at least one workflow.
- `resetDaemon` also removes the workflows directory, so each test starts with no workflow definitions, as it starts with no sessions or workspaces.
- `.menu button:hover` became `:hover:enabled`, and disabled menu items use `--disabled-opacity`, for the disabled entry of a broken definition.

## Checks run

- `make check` — passed.
- `make e2e` — 13 of 13 spec files passed, the stress spec included. An earlier run failed `worktree.e2e.ts` "offers a new task and a session in the checkout in Git" because `run.e2e.ts`'s definitions stayed in the shared workflows directory; `resetDaemon` now removes them.
- `npx wdio run e2e/wdio.conf.ts --spec e2e/specs/run.e2e.ts` — 5 of 5 passed.

## Manual verification

1. The sidebar tree and run view against hypercube's sidebar and the session rows.

   ```sh
   cd ui && npx wdio run e2e/wdio.conf.ts --spec e2e/specs/run.e2e.ts
   # then open run-view.png in the printed OX_E2E_DIR, and
   # ~/src/hypercube/docs/images/overview-dark.png
   ```

   The line runs down from the centre of the run's filled dot, starting just below it, with strokes across to the hollow child dots, and the last child's line turns a rounded corner. Child titles are `--text-xs`, and a child's dot lines up with the start of the run's title, as in hypercube. Run and session rows share height, padding and dot size. Step headings use the tool call's dot column, and commands render as tool calls with a code span after the heading.
