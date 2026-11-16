# Status dots show what needs the user

## Goal

Every sidebar dot answers what the session, run or child needs from the user
now: blue working, orange blocked, red unseen error, green unseen finish, gray
nothing. The daemon records the unseen finish or error, keeps it through
restarts, and clears it when a client shows the session or run in front of
the user.

**Brief:** `.agents/briefs/2026-11-16-002-attention-dots.md`

## Related code

- `crates/oxd/src/session.rs` — `Session::update` is the one place every
  state change passes: it takes the new events, recomputes `SessionInfo`,
  saves differences (`save`) and signals the listing. `Viewer`/`viewers`
  count event subscriptions for parking; they are not "viewing" in the
  brief's sense, since the run view subscribes to every child and a
  background window keeps its subscriptions.
- `crates/oxd/src/state.rs` — `Phase::after` turns each `Change` into
  events. A turn ends as `TurnEnded { stop_reason, error }`; a park or
  archive emits `TurnEnded { error }` followed by `AgentParked`; `fill`
  ends a crash-interrupted turn when history is first read.
- `crates/oxd/src/run.rs` — `Run::end` sets the final `RunState`;
  `Run::update` saves the record and signals.
- `crates/oxd/src/daemon.rs` — `Daemon::new` restores sessions and runs
  and marks running runs `Interrupted`; `close_all` closes sessions at
  shutdown; `add_step_session`/`save_session` create child sessions.
- `crates/oxd/src/store.rs` — schema without migrations; `archived` is a
  separate table so old databases keep working.
- `crates/oxd/src/conn.rs` — per-connection state (subscriptions, watches).
- `crates/ox-protocol/src/messages.rs`, `client.rs` — `Method`,
  `SessionInfo`, `RunInfo`, the client API.
- `crates/ox/src/lib.rs` — Tauri commands; the `watch` command's task
  owns the connection's lifetime from the webview's point of view.
- `ui/src/App.tsx` — `viewed` holds the selected session or run ID.
- `ui/src/features/sessions/SessionList.tsx` — `runStatus`, `childStatus`,
  `StatusIcon`, session/run/step rows.
- `ui/src/features/runs/RunStatus.tsx` — run view steps, using
  `childStatus`.
- `ui/src/styles/controls.css` — `.status` dot classes, including the
  hollow rule.

## Decisions

- **Names.** `Unseen` (`finished` | `error`) is the unseen outcome a
  session or run carries; `SessionInfo.unseen` and `RunInfo.unseen` are
  `Option<Unseen>`. **Focus** is the session or run a client shows in front
  of the user: `focus` is a new per-connection method taking
  `Option<Focus>`, `Focus::Session { session_id }` or
  `Focus::Run { run_id }`. A connection has at most one focus; a new
  `focus` replaces it, `null` clears it, and disconnecting drops it. An
  unknown ID is refused with `NOT_FOUND` and leaves the connection
  unfocused. The answer is `null`. Add both terms to the glossary.
- **Focus counts, held by guards.** `Session` gets `focused: AtomicUsize`;
  `Run` gets `focused: Arc<AtomicUsize>`, and each child session holds a
  clone of its run's counter (`run_focused: Option<Arc<AtomicUsize>>`),
  passed by `add_step_session` and, in `Daemon::new`, by matching the
  session's worktree to a run's. A session is focused when its own count
  or its run's is above zero; a session with `run_focused` is a child.
  `Session::focus` and `Run::focus` return guards that decrement on drop;
  `Daemon::focus(Focus) -> Result<FocusGuard, ErrorBody>` wraps them, and
  `conn.rs` keeps the connection's `Option<FocusGuard>`.
- **Clearing.** Focusing a session increments its count and clears its
  `unseen`, both inside `Session::update`. Focusing a run increments the
  run's count and clears the run's `unseen` under its record lock, then
  clears `unseen` on each of `run.session_ids()`. Increment before clear,
  so a turn ending concurrently either sees the focus or is cleared after.
- **What sets `unseen` on a session.** In `Session::update`, when the
  session is not focused, a pure function `unseen(events) ->
  Option<Unseen>` in `state.rs` reads the new events:
  - a batch with `AgentParked` sets nothing (park and archive);
  - `TurnEnded` with `error: Some` or `stop_reason: Error`, or
    `AgentExited` with `error: Some`, gives `Error`;
  - any other `TurnEnded` except `stop_reason: Cancelled` gives
    `Finished`.
  `Error` replaces `Finished`; `Finished` never replaces `Error`. A child
  session never takes `Finished`. Deleting stops the agent with
  `Exited(None)`, which may set `Error` on a session about to be removed;
  that is harmless.
- **What sets `unseen` on a run.** `Run::end`, when the run is not
  focused: `Succeeded` gives `Finished`, `Failed` gives `Error`,
  `Cancelled` nothing. `Daemon::new` gives a run it marks `Interrupted`
  `Error`.
- **Storage.** Two new tables keep existing databases working, as
  `archived` does: `session_unseen (session_id PRIMARY KEY REFERENCES
  sessions(id) ON DELETE CASCADE, unseen TEXT NOT NULL)` and `run_unseen
  (run_id PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE, unseen TEXT NOT
  NULL)`. `Store::sessions` and `Store::runs` read them into
  `SavedSession.unseen` and `SavedRun.unseen`. `Session::save` writes a
  changed `unseen` with `Store::set_session_unseen`; `Store::update_run`
  also writes the run's `unseen`. `None` deletes the row.
- **Desktop core.** `App` gets `focus: tokio::sync::watch::Sender<Option<Focus>>`.
  A synchronous `focus` command sets it. The `watch` command's task, once
  its connection is up, sends the current focus and then each change, so
  the last value always wins and a reconnect restores the focus without the
  webview's help.
- **Foreground.** `useFocus(focus, connection)` in
  `ui/src/features/sessions/useFocus.ts` tracks foreground from
  `document.hasFocus()` and the window's `focus` and `blur` events, and
  invokes `focus` with the selected session or run while in front and
  `null` otherwise. `App` calls it with `run`/`session` from `viewed`.
- **Dot classes.** One vocabulary of colour roles in `controls.css`:
  `running` (blue, existing animation), `attention` (orange, shared with
  the branch icon as now), `error` (red), `finished` (green), and no class
  for gray. The session-state classes, `succeeded`, `failed` and the hollow
  rule go. `StatusIcon`'s `Status.className` is one of these.
- **Dot rules** in `SessionList.tsx`, highest first:
  - Session row: awaiting → `attention` (state label); `unseen` error →
    `error` ("Error"); starting or running → `running`; `unseen` finished
    → `finished` ("Finished"); else gray with the state label.
  - Child row (session or step-only), given the step, its session if any,
    and `runError` (`run.unseen === "error"`): session awaiting →
    `attention`; session `unseen` error, or step `failed`/`interrupted`
    with `runError` → `error`; step running or session starting/running →
    `running`; else gray. The label is the step's state label, as now.
  - Run row: a child awaiting → `attention`; run or any child `unseen`
    error → `error`; running → `running`; run `unseen` finished →
    `finished`; else gray. The label is the run's state label (the
    outcome), or the awaiting label.
  Red outranks blue because an unseen failure is not superseded by new
  work; green yields to blue because new work supersedes the finish.
- **Run view steps** keep showing each step's outcome:
  `stepOutcome(step, session)` in `RunStatus.tsx` gives `attention`,
  `running`, `finished` for succeeded, `error` for failed, and gray for
  pending, cancelled and interrupted. See Check your assumptions 1.
- **Unseen is cleared only by focus.** Prompting from `oxctl` or answering
  a request does not clear it.

## Check your assumptions

1. **What the run view's step dots show.** Kept.
   - **Decided:** They keep the step's outcome (green succeeded, red failed,
     gray otherwise), since the brief says the outcome stays in the run
     view, and today the step dots are where it shows.
   - **Consequences:** The run view's dots mean something different from
     the sidebar's: a seen, succeeded step is green in the run view and
     gray in the sidebar.
   - **Alternative:** Run view step dots use the sidebar's child rule, so
     they are gray once the run view is open; the outcome shows only in
     tooltips and the failure notice.
   - **Trade-offs:** Keeping outcomes shows at a glance which step failed or
     succeeded but gives dots two meanings across views; one rule
     everywhere is consistent but leaves succeeded and cancelled steps
     unmarked in the run view except by tooltip.

2. **Whether a child session can ever turn green.** Kept.
   - **Decided:** Never. The daemon gives child sessions no `Finished`,
     because the brief shows green only on the run.
   - **Consequences:** After a run ends, a turn the user starts in a child
     session and leaves running in the background ends gray, unlike the
     same turn in an ordinary session.
   - **Alternative:** Suppress green on a child only while its run is
     running.
   - **Trade-offs:** Never is one simple rule; the alternative signals
     follow-up work in children at the cost of a rule that depends on run
     state.

3. **A turn cut off by oxd stopping.** Kept.
   - **Decided:** It turns red. Shutdown closes sessions, which ends an
     open turn with an error, and the brief treats an interrupted run as an
     error the user did not choose.
   - **Consequences:** After oxd restarts (an upgrade, a kill), every
     session that was mid-turn is red, and so is its run. A crash, unlike a
     shutdown, leaves no red until the session's history is first read;
     the code gets a `TODO` for that case.
   - **Alternative:** Leave turns ended by shutdown gray, and only runs red.
   - **Trade-offs:** Red flags work that stopped unfinished; gray avoids a
     burst of red after a routine daemon upgrade.

4. **`oxctl` output.** Kept.
   - **Decided:** `oxctl ls` and `oxctl run ls` stay as they are; `unseen`
     is in the protocol's `SessionInfo` and `RunInfo`, which `oxctl run
     wait` prints as JSON.
   - **Consequences:** Scripts parsing the tab-separated output are
     unaffected, and the terminal listing shows no unseen state.
   - **Alternative:** Append an `unseen` column to both listings.
   - **Trade-offs:** Leaving them avoids an output change nobody asked for;
     a column makes the state visible from the terminal.

## Test plan

- `state.rs` unit test, table-driven, for `unseen(events)`: `EndTurn`,
  `MaxTokens`/`Refusal` and no stop reason give `Finished`; `Cancelled`
  gives nothing; `Error` stop reason, `TurnEnded` with an error, and
  `AgentExited` with an error give `Error`; `AgentExited` without one gives
  nothing; a batch with `AgentParked` gives nothing.
- `crates/oxd/tests/daemon.rs`, synchronizing on the `turn_ended` or
  `agent_exited` event and then reading a new watch's first listing:
  - an unfocused session's turn leaves `unseen: finished`; focusing it
    clears it; a turn ending while focused leaves none; after a
    `focus(null)` on the same connection a turn sets it again;
  - a cancelled turn leaves none; a `reject` turn and an `exit 1` agent
    leave `error`; a later finished turn keeps `error`;
  - closing the focusing connection ends its focus: a turn after it sets
    `finished`;
  - `unseen` survives `daemon.restart()`;
  - focusing an unknown session is refused with `NOT_FOUND`.
- `crates/oxd/tests/runs.rs`:
  - a succeeded unfocused run is `finished`, its children have no
    `unseen`; focusing the run clears it;
  - a failed run (`fail` directive in a step) is `error`, its child is
    `error`; focusing the run clears both; focusing only the child clears
    only the child;
  - a run focused while it fails ends with no `unseen` on it or its child;
  - a cancelled run has none;
  - extend `runs_survive_a_restart_and_running_ones_are_interrupted`: the
    interrupted run is `error`.
- e2e (`ui/e2e/specs/attention.e2e.ts`, plus updates below). First check
  whether `document.hasFocus()` is true under the driver; if not, dispatch
  `focus` on `window` in `wdio.conf.ts`'s `before` and after `reload`. The
  driver cannot switch apps, so tests dispatch `blur` and `focus` on
  `window` to simulate it; note that in the spec.
  - a session whose turn ends while another is selected shows `finished`,
    and gray once selected;
  - with `blur` dispatched, a turn ending in the selected session shows
    `finished`; dispatching `focus` turns it gray;
  - a failing run's row and failed step row are `error` while another
    session is selected, and gray once the run view opens.
- Update `run.e2e.ts`: the run row and child rows are gray (no class) once
  the run, viewed in front, succeeded; run view step dots keep their
  labels with `finished`/`error` classes. Update `model.e2e.ts`'s
  `.status.starting` selector to `.status[aria-label="Starting"]`.
- Update the `sidebar` scenario in `ui/mock/scenarios.ts` to show each
  dot: a green and a red session, a green and a red run, a red child.

## Implementation plan

1. `ox-protocol`: add `Unseen`, `Focus`, `Method::Focus { focus }`
   (`"focus"`), `unseen` on `SessionInfo` and `RunInfo`, and
   `Client::focus`. Document each.
2. `store.rs`: the two tables, `unseen` in `SavedSession`/`SavedRun` and
   their reads, `set_session_unseen`, and `update_run` writing the run's
   `unseen`.
3. `state.rs`: `unseen(events)` and its unit test.
4. `session.rs`: `unseen`, `focused` and `run_focused` fields; `unseen` in
   `info_of` and `save`; set it in `update` per Decisions; `Session::focus`
   and its guard; `Session::new` takes the restored `unseen` and
   `run_focused`.
5. `run.rs`: `unseen` in `RunRecord` and `info_of`, set in `end`;
   `focused` counter, `Run::focus` and its guard, clearing the run.
6. `daemon.rs`: pass `run_focused` in `save_session` for step sessions and
   in `Daemon::new`; set `Error` on runs marked `Interrupted`;
   `Daemon::focus` and `FocusGuard`, clearing children for a run.
7. `conn.rs`: handle `Method::Focus` with the connection's
   `Option<FocusGuard>`.
8. Daemon integration tests.
9. `crates/ox/src/lib.rs`: the `focus` watch channel in `App`, the `focus`
   command, and sending it from the `watch` task.
10. `ui/src/lib/protocol.ts`: `Unseen`, `unseen` fields.
11. `useFocus.ts`, called from `App.tsx`.
12. `SessionList.tsx`: the dot rules, `runError` on child rows;
    `RunStatus.tsx`: `stepOutcome`; `controls.css`: the class vocabulary,
    no hollow rule.
13. Mock scenario and e2e tests.
14. Documentation.

## Documentation updates

- `spec/sessions.md`: a "Status dots" rule under Rules giving the five
  colours and when green and red clear; in "History and service restarts",
  that unseen finishes and errors survive restarts.
- `docs/workflows.md` "Running a workflow": replace the dot paragraph with
  the run and child rules (orange and red on both, green only on the run,
  outcome in the tooltip and run view).
- `docs/dev/architecture.md`: under Ownership boundaries, that the daemon
  keeps each client's focus and the unseen state; under Persistence, the
  unseen tables.
- `docs/dev/glossary.md`: **Focus** and **Unseen**.
