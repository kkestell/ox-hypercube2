# Agent messages in the run view

## Plan

`.agents/plans/2026-11-16-002-run-view-agent-messages.md`

## Summary

The `post_status_update` tool is gone, along with its daemon method, event kind, request, conversation item, fake agent directive and instruction paragraph. The run view now lists each step's non-blank agent messages (`text` items of the child conversation) as Markdown, oldest first, and still follows the latest one while the step runs. The plan's goal is met.

## Departures from the plan

- `docs/dev/architecture.md`, "Instructions", also said a child session is told to post status updates. That clause is removed with the others.
- `crates/oxd/src/session.rs` no longer imports `crate::daemon::now`; `post_status_update` was its only user there. `daemon::now` stays `pub` for `run.rs`.
- The development database `target/dev/oxd.sock.db` did not exist in this worktree, so there was nothing to delete.

## Checks run

- `make check-rust` — Passed.
- `make check-ui` — Passed after `npx biome format --write e2e/specs/run.e2e.ts` wrapped one long line. The worktree needed `npm ci` in `ui/` first.
- `make e2e` — `run.e2e.ts` passed (5/5), including the rewritten agent message test. In the full parallel run, `archive`, `commands`, `connection`, `files` and `merge` failed. They failed on missing workspace sections and on WebDriver `ECONNREFUSED`, in code this change does not touch. Each passed when run alone with `npx wdio run e2e/wdio.conf.ts --spec e2e/specs/<name>.e2e.ts` from `ui/`.

## Follow-up work

- Delete the installed database before running the installed app on this change. A stored `status_update` event no longer parses.
