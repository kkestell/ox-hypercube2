# Database migrations and separate app and daemon reinstalls

## Scope and coverage

The staged changes for `.agents/plans/2026-11-17-004-migrations-and-reinstall.md`:
the versioned schema and `schema-1.sql` fixture in `crates/oxd`, the derived
protocol version (`crates/ox-protocol/build.rs`), `daemon.stop` and
`oxd --stop`, the app's mismatch message, the `install` and `install-ui` make
targets, and the docs. Reviewed against the plan and its work log; the plan
links no brief.

Gaps:

- `make install` and `make install-ui` were not run against the real install,
  since both quit the Ox app this review runs in. Their shell was checked with
  `make -n` and by running the app-detection command on its own.
- Whether `osascript -e 'tell application "Ox" to quit'` reaches the installed
  app and the app honours it was not exercised.

## Summary

| ID | Severity | Category | Fix   | Title                                                         |
| -- | -------- | -------- | ----- | ------------------------------------------------------------- |
| F1 | High     | Correctness | Fixed | Install targets miss the app when run from an Ox session   |
| F2 | Low      | Simplicity  | Fixed | `REPLACE_OX` spells out the installed app path again       |

## F1. Install targets miss the app when run from an Ox session

High · Correctness · Fixed · `Makefile:30`

`QUIT_OX` detects the running app with `pgrep -qf`. macOS `pgrep` leaves out
its own ancestors unless given `-a`, and a shell an Ox agent runs descends from
the app (`ox` → `oxd` → bridge → agent → shell). Run from an Ox session,
`make install-ui` sees no app, so it does not quit it, replaces the bundle
under the running app, and `open` only brings the old process forward: the
target reports `Installed` while the old UI keeps running. `make install`
likewise skips the quit and the relaunch. Agents in Ox sessions are how this
repository is worked on, so this is the normal case.

### Evidence

From this session's shell, whose ancestry is `zsh` → `claude` →
`claude-code-acp` → `oxd` → `ox` (pid 44736):

- `pgrep -qf "$HOME/Applications/Ox.app/Contents/MacOS/ox$"` exits 1, while
  `ps` lists `/Users/kyle/Applications/Ox.app/Contents/MacOS/ox`.
- `pgrep -qaf` with the same pattern exits 0.
- `man pgrep`: "By default, the current pgrep or pkill process and all of its
  ancestors are excluded."

`oxd` is started with `setsid`, so it outlives the app's quit and the make
recipe, running under `oxd`, carries on to reopen the app.

### Fix

`QUIT_OX` uses `pgrep -qaf` in both checks, with a comment naming the case.

## F2. `REPLACE_OX` spells out the installed app path again

Low · Simplicity · Fixed · `Makefile:37`

`REPLACE_OX` wrote `$(HOME)/Applications/Ox.app` three times beside the new
`INSTALLED_APP` variable that holds the same path.

### Fix

`REPLACE_OX` uses `$(INSTALLED_APP)` and `$(dir $(INSTALLED_APP))`.

## Checks run

- Traced `daemon.stop` through `conn::serve`, `Daemon::stop`, and `run`'s
  shutdown: the answer is queued before the stop signal is seen, and the
  connection closes only after the socket is removed and events are flushed,
  so `oxd --stop` returns once the old daemon has committed everything.
- Compared `EventKind`'s 15 variants with the `type`s in `schema-1.sql`: every
  variant is present.
- `pgrep -qf` and `pgrep -qaf` against the running app, as above.
- `make -n install` and `make -n install-ui` after the fix: the expanded
  commands quit, replace, and open the installed app as intended.

## Verdict

The migrations, the derived protocol version, and `daemon.stop` deliver the
plan. F1 made both install targets skip quitting and relaunching the app
whenever they run from an Ox session; with it and F2 fixed, the change is
ready, pending the plan's hand checks of the two targets on the real install.
