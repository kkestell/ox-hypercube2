# Testing

Test the behavior people rely on. Keep the suite understandable and the effort
proportionate to the consequence of being wrong. The scope and rare-case rules
in [Code style](code-style.md) apply to tests too.

## What to test

- Cover required behavior, everyday use, useful errors, and reproduced bugs.
  Choose representative cases that establish the rule being tested.
- Verify straightforward, reversible changes by inspection. Add a test when it
  would catch a meaningful failure.
- Test observable results through the closest stable boundary, at the lowest
  layer of [Test layers](#test-layers) that can observe them.
- Repeat an assertion at different layers only when those layers have distinct
  failure modes. A broader test should prove something the focused test cannot.
- Assert on the actual contract, so a refactor that keeps observable behavior
  keeps its tests passing.
- Cover each rule with the cases that establish it.

## Test layers

| Layer | Where | Proves |
| --- | --- | --- |
| Rust unit tests | beside the code in `crates/` | Logic in one module, including durations on tokio's paused clock |
| Daemon integration tests | `crates/*/tests` | `oxd` and the bridges over their real protocols, with `oxd-fake-agent` and real git |
| UI tests | `ui/src/**/*.test.ts(x)` | The UI's behavior: the real `App` in jsdom against a fake desktop core |
| End-to-end tests | `ui/e2e/specs` | The built app, its desktop core and `oxd` working together in the real webview |
| Live checks | `scripts/smoke-acp.py` | Compatibility with the real Claude and Pi agents |

- UI tests render `App` with `renderApp` from `ui/src/test/core.tsx`, which
  fakes the desktop core at the Tauri IPC boundary. A test sends listing
  changes and conversation updates through the channels the app opened,
  answers commands with `core.on`, and asserts on the commands the app
  invoked with `core.invoked`. Native menus and dialogs are answered at the
  same boundary.
- Keep the end-to-end suite to representative flows through the built app
  and behavior that needs a real webview. It starts a session
  from the UI and follows the conversation across a daemon restart, watches
  live changes to a directory and open file, and loads an agent's tool image
  under the webview's content security policy.
- Add an end-to-end case when it catches a specific failure that the UI and
  daemon suites cannot see. For a new Tauri command, the UI test can assert the
  command and its arguments, and the daemon integration test can establish the
  operation's behavior.
- Give each end-to-end case a fresh workspace and use the app for the flow
  under test. The fixture adds the workspace through the desktop command to
  bypass the native folder picker. The run has its own daemon socket and
  stops that daemon when it finishes.
- Check layout, colour, spacing and alignment by looking at the running app,
  as [Ox tests and fixtures](#ox-tests-and-fixtures) describes. Tests assert
  on behavior.

## Writing tests

- Give each test a clear behavioral purpose and name the expected outcome.
  Split unrelated assertions; keep assertions that establish one result
  together.
- Use table-driven cases for varied inputs to the same rule. Identify the
  failing case and show useful expected and actual values.
- Keep setup proportional to the behavior. Extract shared fixtures when they
  clarify repeated setup.
- Use realistic local data and real components where practical. Substitute
  external systems at their boundary and use real internals.
- Control time, randomness, and concurrency when they affect the result, as
  [Time and concurrency](#time-and-concurrency) describes.
- Give tests their own temporary files, configuration, and state. Clean up the
  resources they create without interfering with the user's running processes
  or other tests.
- Capture enough output to explain failures. Each test runs independently of
  execution order and of other tests' state.

## Time and concurrency

A test passes or fails on the behavior under test alone, at any machine speed
and load. A test whose result depends on timing is a failing test, whether or
not it failed today.

- Synchronize every step on the observable event that proves the previous
  action took effect: a response, a session event, a listing change, a log
  line, or a file or FIFO a fixture writes. A bounded wait (`TIMEOUT` in Rust
  tests, `waitforTimeout` in end-to-end tests) is only the limit on waiting
  for something that must happen.
- Hold a fixture open on something the test releases when the test acts while
  the fixture runs: a permission request, `wait`, background work
  (`--background-running`), or the `blocked` and `release` FIFOs of
  `mkfifos`. Fixture delays (`say` and `stream`) and shell `sleep` serve only
  to generate load.
- Use a process that never ends on its own, such as `tail -f /dev/null`, for
  work the test expects to be stopped, so a missing stop fails the test.
- Test durations (delays, timeouts, grace periods) in process on tokio's
  paused clock, `#[tokio::test(start_paused = true)]`, with the timing in a
  function that takes its delay and the channels it watches. Integration
  tests set the duration to zero or out of reach and test the wiring through
  events: test daemons run without an idle exit unless the test is about
  idle exit, and parking tests use `park_after_ms: Some(0)`.
- Prove that something did not happen with a later event it would have
  preceded: the turn's end, or the answer to a later request on the same
  connection, which the daemon reads in order. Where the system offers no
  such event, test the decision as a unit and keep the integration test to
  what does happen.
- Assert an order only where the system orders it. Output from independent
  tasks, processes, pipes, or file watchers arrives in either order, so a test
  checks each item's presence, or orders them through an event.
- In end-to-end tests, wait for the element or attribute that shows the
  expected state.

Clippy rejects `tokio::time::sleep`, `sleep_until`, and `std::thread::sleep`
(`clippy.toml`), and Biome rejects `browser.pause`, `setTimeout`, and
`setInterval` in `ui/e2e` (`ui/biome.json`). A delay that is itself the
behavior, such as the park delay, sits in a small function marked with an
`expect` that gives the reason.

## Maintaining the suite

- Treat tests as maintained code. A change that replaces behavior replaces the
  tests of the old behavior with tests of the new. Remove tests whose behavior
  disappears.
- Add meaningful regression coverage for reproduced bugs when practical.
- A new test fails on the code before the change and passes with it. To check,
  run it against a detached worktree of `HEAD`.
- A case that an existing test's setup already reaches is an assertion in that
  test.
- Inspect the complete test diff. Check that removed or rewritten tests have
  kept required coverage, and explain material coverage gaps.
- A failing test is evidence to investigate. Determine whether the code, the
  expectation, or the environment is wrong before changing it.

## Ox tests and fixtures

- Drive the same application paths the desktop uses. Socket selection supports
  isolated runs; fixtures belong with the tests. Set up and tear down through
  application operations.
  - Delete a session blocked in startup with `session.delete`.
- Confirm the event a test waits for actually happened before checking its
  consequences.
  - `delete_while_starting` waits for the agent to open its FIFO before deleting
    the session, then waits for the FIFO to reach end of file.
  - `cancellation_stops_a_pending_permission` waits for the prompt's `cancelled`
    response before answering the withdrawn permission request.
- Before finishing a UI change, compare affected elements with their peers on
  screen for size, colour, spacing, and alignment against the user's design.
- Inspect existing tests and helpers before adding a fixture or interception.
  Check the installed tool's behavior at the boundary the test depends on:
  readiness, events, native default actions, and whether an API can actually
  be replaced. Prove an unfamiliar mechanism in a focused test before using
  it throughout a suite.
- Distinguish waiting for a process to become ready from observing it while
  it starts. When testing a blocked state, leave the readiness wait pending,
  observe the published state or event, then release or cancel it through an
  existing application operation and settle the pending command.
- A driver may dispatch an event without performing the browser's native
  default action, or move a pointer without delivering entry events. Establish
  which behavior is missing before interpreting a failure as an application
  bug. Change application behavior only for users.
- When a native boundary cannot be driven, simulate only that boundary in
  test code, preserve the real application path around it, and restore any
  interception after use. Document and report what the test cannot verify.
- `ui/src/performance.test.tsx` holds the UI to
  [Performance](performance.md): with hundreds of sessions and replies, it
  counts the Markdown parses, Markdown block renders and session row renders
  each change causes. A change whose work grows with the session list or the
  transcript fails it by hundreds.
- Isolate a failure with the smallest relevant test before rerunning a broad
  suite. Distinguish fixture and driver failures from product failures, and
  fix the cause.
  Rerun checks after fixes that invalidate their results, and finish with the
  required checks passing for the final change.

## Running checks

- Use focused tests during development. Before delivery, run the project's
  required checks and the checks needed for the affected behavior. Broaden
  verification only when a concrete risk or failure warrants it.
- For documentation, comments, and filename-only changes, the check is
  inspecting the relevant text and diff.
- Use documented commands directly. Once the appropriate checks pass, stop;
  repeat them only after a new change or to resolve an outstanding concern.
- Read the results before claiming success. Report checks that failed or could
  not run, their reasons, and the resulting verification limits.

### Required commands

Run commands from the repository root for the parts that changed:

| Changed area | Required check |
| --- | --- |
| Rust | `make check-rust` — formatting, clippy with warnings as errors, and workspace tests |
| UI | `make check-ui` — Biome, TypeScript checks, and the UI tests |
| `scripts/` | `make check-scripts` — ruff lint and formatting checks |
| All three | `make check` |

`make fix` applies the formatters and safe lint fixes.

`make check-rust` builds `ox-mcp` before running the tests because the daemon
integration tests launch it beside `oxd`. Before running those tests directly
with `cargo test`, run `cargo build -p ox-mcp`.

After changing `crates/ox`, the app's Tauri configuration, a command the UI
invokes or the channel messages it reads, or `ui/e2e`, also run `make e2e`
and report its result. It drives the tests in `ui/e2e` against the built app
and the scripted `oxd-fake-agent`. `ui/e2e/wdio.conf.ts` and
`ui/e2e/helpers.ts` set up their environment. For other UI changes,
`make check-ui` is the evidence.

The live checks run real models through `oxd`, separately from
`cargo test`, and each run spends paid model requests. Run them only for
changes whose correctness depends on a real agent: after changing
`claude-code-acp`, run `make smoke-claude`; after changing `pi-acp`, run
`make smoke-pi`; after changing how `oxd` launches agents or exchanges ACP
messages with them, or changing `scripts/smoke-acp.py`, run both. Run each
check once and report its result. For other changes to `oxd`, `ox-protocol`,
the integration tests against `oxd-fake-agent` are the evidence.
The Claude check requires an installed, authenticated Claude Code CLI, and the
Pi check an installed Pi CLI authenticated with OpenRouter. The Makefile's
`CLAUDE_MODEL` and `PI_MODEL` set their models. See
[External systems and live checks](#external-systems-and-live-checks) for
interpreting and reporting results.

The `check_*` methods of `scripts/smoke-acp.py` define what each run covers.
Each run prints the directory with its results, events, logs, and test project.
Set `CLAUDE_MODEL` or `PI_MODEL` to override Make's default model; Pi model values
are provider-qualified (`provider/id`). The script defaults to the Claude agent.
To choose the agent and override the per-phase timeout directly, run
`python3 scripts/smoke-acp.py --agent claude --model claude-haiku-5-5 --timeout 180`.

## External systems and live checks

- Keep routine automated tests independent of personal credentials and live
  services where practical. Use scripted fixtures for repeatable responses.
- Use a real integration check when the change depends on behavior a fixture
  cannot establish, such as provider compatibility, process isolation, or
  terminal interaction. Exercise the affected user flow and inspect its output.
- State what each layer proves. Only a check against the real service
  establishes compatibility with it.
- Report missing credentials or unavailable services as skipped live checks.
  Distinguish an environmental failure from a product regression using evidence.
