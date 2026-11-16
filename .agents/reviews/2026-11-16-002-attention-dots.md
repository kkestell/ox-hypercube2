# Status dots show what needs the user

## Scope and coverage

The staged changes for `.agents/plans/2026-11-16-002-attention-dots.md`: the `Focus` method and `Unseen` state in `ox-protocol`, their storage in `store.rs`, setting and clearing them in `state.rs`, `session.rs`, `run.rs`, `daemon.rs` and `conn.rs`, focus forwarding in the desktop core (`crates/ox/src/lib.rs`), `useFocus`, the sidebar and run view dot rules, the CSS class vocabulary, the daemon and e2e tests, the mock scenario, and the documentation. Also read the paths the change depends on: session shutdown, deletion and archive (`Daemon::close_all`, `delete_session`, `delete_run`, `Phase::after` for `Closed`, `Exited` and `Parked`), run restoration in `Daemon::new`, and the shared client and watch replacement in the desktop core. Checked against the brief `.agents/briefs/2026-11-16-002-attention-dots.md`.

Gaps:

- The e2e suite was not rerun; the work log records a full passing run.
- The mock `sidebar` scenario was not viewed.

## Summary

| ID | Severity | Category    | Fix       | Title                                                      |
| -- | -------- | ----------- | --------- | ---------------------------------------------------------- |
| F1 | Low      | Performance | Not fixed | Focusing a run rewrites and re-sends it when nothing changed |

## F1. Focusing a run rewrites and re-sends it when nothing changed

Low · Performance · Not fixed · `crates/oxd/src/run.rs:605`

`Run::focus` goes through `Run::update`, which always writes the run's row (state, error, steps JSON and `run_unseen`) and bumps the run's version, so every watcher is sent the run again. With a run view open, each time the Ox window comes to the front, oxd rewrites the run and every client re-renders it, even when the run had no unseen outcome. `Session::focus` avoids this: `Session::save` writes and signals only when `SessionInfo` changed. The cost is one small write and one listing change per window switch, with no wrong behavior.

### Evidence

`Run::update` (`crates/oxd/src/run.rs:238`) calls `store.update_run` and `changed.send_replace(())` unconditionally; `Run::focus` calls it with a change that only clears `unseen`.

### Fix

Not fixed, as a Low finding.

## Checks run

- `cargo test -p oxd` — passed (75 unit, 66 daemon, 18 run, 43 other integration tests).

## Verdict

The change delivers the brief's outcome and keeps its decisions: the five dot colours, green only on runs, red on both a run and its failed child, clearing on focus with the window in front, nothing set by cancel, park or archive, and unseen state kept by oxd through restarts. Focus counting and clearing are ordered correctly against concurrent turn and run ends. F1 is a minor wasted write and listing update, left unfixed.
