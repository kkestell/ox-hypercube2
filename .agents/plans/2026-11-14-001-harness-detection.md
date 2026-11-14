# Harness detection and agent choice

## Goal

The app offers an agent for each harness the user has installed: Claude when
`claude` is on the `PATH` sessions get, Pi when `pi` is. With neither, the New
Task button is disabled. With one, the New Task menu has no Harness select.
With both, the select offers Claude and Pi. Installing or removing a harness
while the app runs shows up the next time the window gains focus.

Choosing Pi starts `pi-acp` from beside the app's executable. The bridge
itself is task 2 of `.agents/scratch/pi.md`; this change depends only on its
name.

## Related code

- `crates/ox/src/main.rs` — `fix_path_env::fix()` replaces `PATH` with the
  login shell's at startup, so an app opened from Finder sees the same `PATH`
  as a terminal.
- `crates/ox/src/lib.rs:75` — `Agent`, which has only `Claude`.
- `crates/ox/src/lib.rs:256` — `new_session` builds the bridge command from
  `Agent`.
- `crates/ox/src/lib.rs:463` — `utf8_env()`, the environment the app passes
  to sessions.
- `ui/src/features/sessions/SessionList.tsx:21` — the fixed `agents` list,
  the `agent` state, the New Task button, and the Harness select.
- `ui/e2e/wdio.conf.ts` — launches the one app instance all specs share,
  through `@wdio/tauri-service`, whose `env` option is merged over the
  launcher's environment for the app.
- `ui/e2e/specs/worktree.e2e.ts:15` — asserts the Harness select shows
  `claude`.
- `crates/claude-code-acp/src/runner.rs:170` — the bridge spawns `claude`
  from `PATH` only when the first prompt arrives, so e2e sessions created
  through the desktop never run it.

## Decisions

- **Harness:** the CLI a bridge drives: `claude` for Claude, `pi` for Pi.
  `Agent::harness()` and `Agent::bridge()` give each agent's executable names.
- **Detection** looks in each directory of the `PATH` from `utf8_env()` for a
  regular file with the harness's name and an execute bit. It does not run
  the harness.
- **The `agents` command** returns the installed agents in the order Claude,
  Pi, serialized as `"claude"` and `"pi"`, the same values `new_session`
  takes.
- **Re-detection** runs on the webview's `window` `focus` event, which
  WebKit fires when the app's window becomes key. The webview asks once on
  mount and again on each focus.
- **The chosen agent** is the one the user picked if it is still installed,
  otherwise the first installed agent. Removing Pi while it is chosen falls
  back to Claude without a reset step.
- **e2e `PATH`:** the app runs with `PATH` set to
  `<OX_E2E_DIR>/harnesses:/usr/bin:/bin:/usr/sbin:/sbin` and `SHELL` set to
  a stub login shell, `<OX_E2E_DIR>/login-shell`, that runs its `-c`
  command with `/bin/sh` and the inherited environment
  (`exec /bin/sh -c "$2"`). `fix_path_env` then keeps the controlled `PATH`
  instead of the user's. The stub stands in only for the user's shell
  configuration; the app's startup path is unchanged. `harnesses/` holds an
  executable `claude` stub by default, so existing specs see one agent.

## Check your assumptions

1. **What counts as installed.** Kept.
   - **Decided:** An executable file named `claude` or `pi` on the `PATH`, as
     the notes specify; the bridges spawn exactly that.
   - **Consequences:** An installed but unauthenticated Claude Code, or a Pi
     too old for RPC mode, is still offered, and its session fails when it
     starts.
   - **Alternative:** Run `claude --version` and `pi --version` and offer only
     harnesses that answer.
   - **Trade-offs:** The file check is instant on every focus. Running the
     harnesses catches broken installs but spawns two processes per focus and
     still says nothing about authentication.

2. **Which `PATH` re-detection reads.** Kept.
   - **Decided:** The `PATH` fixed at startup from the login shell; focus
     rescans its directories.
   - **Consequences:** Installing a harness into a directory already on
     `PATH` (Claude Code and Pi both install to `~/.local/bin`) shows up on
     focus. Installing into a directory newly added to the shell profile
     needs an app restart.
   - **Alternative:** Re-read the login shell's `PATH` on each focus.
   - **Trade-offs:** Re-reading catches profile edits but runs an interactive
     login shell on every focus, which can take a second, and changes the
     environment of the running app.

3. **Pi is offered before `pi-acp` exists.** Kept.
   - **Decided:** Pi is offered whenever `pi` is installed, as the notes
     specify. `make dev` and `make install` start building `pi-acp` in task 2.
   - **Consequences:** Until task 2 lands, choosing Pi shows "failed to start"
     with the missing `pi-acp` path.
   - **Alternative:** Also require the bridge beside the app's executable
     before offering an agent.
   - **Trade-offs:** Checking the bridge hides the broken choice in builds
     without it, at the cost of a second condition the bundled app always
     meets.

4. **How e2e controls what is installed.** Kept.
   - **Decided:** A stub login shell and a controlled `PATH`, as in Decisions;
     tests change `harnesses/` and dispatch `focus`.
   - **Consequences:** The e2e app no longer sees the user's `PATH`, so
     specs can use only system tools and the binaries they reference by
     absolute path. The worktree spec's Harness select assertions go, since
     one agent shows no select.
   - **Alternative:** Skip `fix_path_env` in builds with the `webdriver`
     feature and pass `PATH` directly.
   - **Trade-offs:** The stub keeps the app's startup identical in tests. The
     feature gate is one line but makes the tested app differ from the
     shipped one.

5. **No explanation on the disabled button.** Kept.
   - **Decided:** The New Task button keeps its "New Task" title when
     disabled.
   - **Consequences:** With neither harness installed, nothing in the app says
     why sessions cannot start.
   - **Alternative:** Change the disabled button's title to "Install Claude
     Code or Pi to start a session".
   - **Trade-offs:** The plain title follows the standard disabled control;
     the tooltip tells a first-time user what to do at the cost of one string.

## Test plan

- `crates/ox/src/lib.rs` unit test, `finds_executable_harnesses_on_path`,
  table-driven over a temporary directory: an executable `claude` is found;
  a non-executable file named `pi` and a directory named `pi` are not; a
  name in the second `PATH` directory is found.
- `ui/e2e/specs/harness.e2e.ts`, each test in a workspace from
  `startSession()`:
  - with only `claude`, the New Task menu has no Harness select and
    Session in Checkout starts a session;
  - with neither, after `focus`, the New Task button is disabled;
  - with both, after `focus`, the Harness select offers Claude and Pi; after
    removing `pi` and `focus` again, the select is gone.
  - `afterEach` restores `harnesses/` to `claude` only and dispatches
    `focus`.
- `ui/e2e/specs/worktree.e2e.ts`: the first test drops the Harness select
  lines and keeps its other assertions.

## Implementation plan

1. `crates/ox/src/lib.rs`:
   - `Agent` derives `Clone`, `Copy`, `Serialize` as well as `Deserialize`,
     and gains `Pi`, `const ALL: [Agent; 2]`, `harness()`, and `bridge()`.
   - `new_session` builds the command from `agent.bridge()` beside
     `current_exe()`.
   - `on_path(path: &OsStr, name: &str) -> bool` over
     `std::env::split_paths`, checking `is_file()` and
     `PermissionsExt::mode() & 0o111 != 0`.
   - `#[tauri::command] fn agents() -> Vec<Agent>` filters `Agent::ALL` by
     `on_path` against `utf8_env()`'s `PATH`; an absent `PATH` finds none.
     Register it in `generate_handler!`.
   - The unit test. `crates/ox/Cargo.toml` gains `tempfile = "3"` under
     `[dev-dependencies]`.
2. `ui/src/features/sessions/useAgents.ts` (new): `type AgentId = "claude" |
   "pi"`, an `agentNames` record (`Claude`, `Pi`), and `useAgents()`, which
   invokes `agents` on mount and on `window` `focus`, returns the list
   (empty until the first answer), and reports failures with `showError`.
3. `ui/src/features/sessions/SessionList.tsx`: replace the fixed `agents`
   list with `useAgents()`. Keep the user's pick in state and derive `agent`
   from it as in Decisions. Disable the New Task button when the list is
   empty. Render the `.menu-field` with the Harness select only when the list
   has more than one agent.
4. `ui/e2e/wdio.conf.ts`: in `onPrepare`, create `harnesses/` with an
   executable `claude` stub and the `login-shell` stub; pass `PATH` and
   `SHELL` in the service's `env`.
5. `ui/e2e/helpers.ts`: `setHarnesses(...names: ("claude" | "pi")[])`, which
   makes `harnesses/` hold exactly those stubs and dispatches `focus` on the
   webview's `window`.
6. The e2e specs from the test plan.

## Documentation updates

- `docs/dev/architecture.md`, under **Desktop and command-line clients**: the
  app offers an agent for each harness on the `PATH` it passes to sessions,
  and checks again when its window gains focus.
- `docs/dev/glossary.md`: add **Harness**: the coding-agent CLI a bridge
  drives, `claude` or `pi`.
- `docs/dev/testing.md`, in the `make e2e` paragraph: the app runs with a
  stub login shell and a `PATH` of the system directories and a directory of
  harness stubs, holding `claude` unless a test changes it with
  `setHarnesses`.
