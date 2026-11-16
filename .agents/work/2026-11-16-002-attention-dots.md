# Status dots show what needs the user

## Plan

`.agents/plans/2026-11-16-002-attention-dots.md`

## Summary

oxd records an unseen `finished` or `error` on sessions and runs, saves it in `session_unseen` and `run_unseen`, and clears it on a connection's `focus`. The desktop core forwards the window's focus over its watch connection, the webview sends it only while the window is in front, and the sidebar dots follow the plan's rules. The plan's goal is met.

## Departures from the plan

- `useFocus` takes only the focus (`useFocus(focus)`), not `connection`: the desktop core resends the focus on each new connection itself, so the webview has no use for the connection count. `App` memoizes the focus object from the viewed run or session ID.
- The core's watch task uses a `biased` select that sends a focus change before forwarding the next listing. Without it, nothing observable orders a focus change before a later turn's end, and the background-window e2e test could not synchronize; with it, the listing that shows a prompt's turn running proves oxd already has the focus that preceded the prompt.
- `ui/src/features/conversation/Conversation.tsx`: the failed tool-call icon uses `.status.error`, since `.status.failed` is gone from the dot vocabulary.
- `crates/ox/Cargo.toml`: tokio gets the `macros` feature for `select!` (already enabled in the workspace; `Cargo.lock` is unchanged).
- `ui/e2e/wdio.conf.ts` and `reload()` call a new `holdForeground()` helper. `document.hasFocus()` is true under the driver, but the full suite failed twice when another app came to the front mid-run (a real `blur`). The helper stops the window's trusted `focus`/`blur` events in the capture phase and dispatches `focus`, so tests depend only on the `blur`/`focus` they dispatch.
- `Daemon::focus` returns a `FocusGuard` enum of `SessionFocus`/`RunFocus` guards; it carries `#[expect(dead_code)]` because the guards are held only for their drop.
- `ui/e2e/helpers.ts` exports `oxctl` and adds `sessionId(cwd)` for the attention spec.

## Decisions

- `Session::unseen` is a mutex beside the state, changed only inside `Session::update`, and computed in `note_unseen` from the batch's events; a park's or an archive's batch contains `AgentParked` and so sets nothing.
- The `TODO` for crash-interrupted turns sits in `State::fill`, where such a turn first gets its error.
- Store unit tests cover the new tables: replacement, deletion with `None`, and cascade on session and run removal.
- `focus` on an unknown run is tested beside an unknown session, at no extra setup.

## Checks run

- `make check-rust` — passed (fmt, clippy, all workspace tests).
- `cd ui && npm run check` — passed.
- `cd ui && npx wdio run e2e/wdio.conf.ts` after the e2e build — 14 of 14 spec files passed.

## Manual verification

1. The trusted-event interception works in WebKit. A throwaway spec (deleted afterwards) opened a session, ran `osascript -e 'tell application "Finder" to activate'`, waited until `document.hasFocus()` was false, ran a turn, and saw the session's dot stay gray at Idle.

   ```sh
   cd ui && npx wdio run e2e/wdio.conf.ts --spec e2e/specs/<probe>.e2e.ts
   ```

   Passed.

2. Another worktree's e2e suite running at the same time shares the embedded WebDriver and makes this one fail (session rows not found, `ECONNREFUSED`, script timeouts). The final full run was made while no other suite ran.

## Follow-up work

- The mock `sidebar` scenario was updated but not reviewed by eye against its peers; `cd ui && npm run mock` shows it.
