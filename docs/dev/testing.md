# Testing

Test the behavior people rely on. Keep the suite understandable and the effort
proportionate to the consequence of being wrong. The scope and edge-case policy
in [Code style](code-style.md#implementation) applies to tests too.

## What to test

- Cover required behavior, everyday use, useful errors, and reproduced bugs.
  Choose representative cases that establish the rule being tested.
- Do not add tests for straightforward, reversible changes that inspection
  sufficiently verifies. A new test should catch a meaningful failure.
- Test observable results through the closest stable boundary. Use unit tests
  for tricky logic and integration or end-to-end tests for behavior that depends
  on components working together.
- Repeat an assertion at different layers only when those layers have distinct
  failure modes. A broader test should prove something the focused test cannot.
- Do not freeze internal structures, private call sequences, or incidental
  formatting unless they are part of the actual contract. Refactoring should
  not require rewriting tests whose observable behavior still holds.
- Avoid exhaustive permutations, hypothetical fault matrices, and coverage
  targets that reward test volume. Deferred edge cases do not become testing
  requirements.

## Writing tests

- Give each test a clear behavioral purpose and name the expected outcome.
  Split unrelated assertions; keep assertions that establish one result
  together.
- Use table-driven cases for varied inputs to the same rule. Identify the
  failing case and show useful expected and actual values.
- Keep setup proportional to the behavior. Extract shared fixtures when they
  clarify repeated setup; do not build a test framework around a few cases.
- Use realistic local data and real components where practical. Substitute
  external systems at their boundary, rather than mocking every internal step.
- Control time, randomness, and concurrency when they affect the result. Wait
  for observable conditions with bounded waits instead of guessing sleep times.
- Give tests their own temporary files, configuration, and state. Clean up the
  resources they create without interfering with the user's running processes
  or other tests.
- Capture enough output to explain failures. Tests should run independently
  without relying on execution order or leftovers from another test.

## Maintaining the suite

- Treat tests as maintained code. Rewrite or remove tests when their behavior
  changes or disappears; do not preserve obsolete expectations merely to keep
  the suite green.
- Add meaningful regression coverage for reproduced bugs when practical. A
  regression test should fail for the original behavior and pass with the fix.
- Inspect the complete test diff. Check that removed or rewritten tests have
  not accidentally lost required coverage, and explain material coverage gaps.
- A failing test is evidence to investigate. Determine whether the code, the
  expectation, or the environment is wrong before changing it.

## Ox tests and fixtures

- Drive the same application paths the desktop uses. Socket selection supports
  isolated runs; fixtures belong with the tests. Reuse application operations
  instead of adding test-only production APIs.
  - Delete a session blocked in startup with `session.delete`.
  - Build text prompts with a helper in the test file; `Client` has one `prompt`
    method.
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
  bug. Do not change application behavior solely to accommodate the driver.
- When a native boundary cannot be driven, simulate only that boundary in
  test code, preserve the real application path around it, and restore any
  interception after use. Document and report what the test cannot verify.
- `ui/e2e/specs/stress.e2e.ts` holds the app to time budgets with hundreds of
  sessions and a transcript of thousands of long Markdown replies, far beyond
  everyday use. It measures inside the webview and waits on the daemon through
  `oxctl`, because finding thousands of elements through the driver occupies
  the webview's main thread. A change whose work grows with the session list
  or the transcript fails it. [Performance](performance.md) describes the
  design it checks.
- Isolate a failure with the smallest relevant test before rerunning a broad
  suite. Distinguish fixture and driver failures from product failures; fix
  the cause rather than adding timeouts, retries, or recovery machinery.
  Rerun checks after fixes that invalidate their results, and finish with the
  required checks passing for the final change.

## Running checks

- Use focused tests during development. Before delivery, run the project's
  required checks and the checks needed for the affected behavior. Broaden
  verification only when a concrete risk or failure warrants it.
- For documentation, comments, and filename-only changes, inspect the relevant
  text and diff. Do not run full tests, builds, linters, or formatters unless
  explicitly required. A normal filesystem rename needs no content hashing.
- Use documented commands directly. Once the appropriate checks pass, stop;
  repeat them only after a new change or to resolve an outstanding concern.
- Read the results before claiming success. Report checks that failed or could
  not run, their reasons, and the resulting verification limits.

### Required commands

Run commands from the repository root for the parts that changed:

| Changed area | Required check |
| --- | --- |
| Rust | `make check-rust` — formatting, clippy with warnings as errors, and workspace tests |
| UI | `make check-ui` — Biome and TypeScript checks |
| `scripts/` | `make check-scripts` — ruff lint and formatting checks |
| All three | `make check` |

`make fix` applies the formatters and safe lint fixes.

`make check-rust` builds `ox-mcp` before running the tests because the daemon
integration tests launch it beside `oxd`. Before running those tests directly
with `cargo test`, run `cargo build -p ox-mcp`.

After changing behavior in `crates/ox` or the UI's TypeScript, also run
`make e2e` and report its result. It builds the app with its embedded WebDriver
server and drives the tests in `ui/e2e` against sessions of `oxd-fake-agent`
that the tests start with `oxctl`. Tests that create sessions through the
desktop start the Claude bridge without sending it prompts. The app runs with
a stub login shell and a `PATH` of the system directories and a directory of
harness stubs, which holds `claude` unless a test changes it with
`setHarnesses`. A change only to `ui/src/styles/` requires `make check-ui`.

After changing `oxd`, `ox-protocol`, `oxctl`, `claude-code-acp`, or
`scripts/smoke-acp.py`, also run `make smoke-claude` and report its result.
After changing `oxd`, `ox-protocol`, `oxctl`, `pi-acp`, or
`scripts/smoke-acp.py`, also run `make smoke-pi` and report its result.
These live checks run real models through `oxd` and `oxctl`, separately from
`cargo test`. Claude uses Haiku and requires an installed, authenticated
Claude Code CLI. Pi uses `openrouter/deepseek/deepseek-v4.1-flash-1017` and
requires an installed Pi CLI authenticated with OpenRouter. See
[External systems and live checks](#external-systems-and-live-checks) for
interpreting and reporting results.

Both runs cover streaming, reconnect and replay, conversation continuity,
native skills with arguments, cancellation, model verification, history and
resume after a daemon restart, idle parking, archive and unarchive, and process
cleanup. Claude also covers effort changes, native local commands, permissions,
AskUserQuestion forms, background work and wake, and the `ox` MCP title tool.
Pi also covers thinking changes and extension dialogs answered as forms.

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
- State what each layer proves. Passing tests against a fake service do not
  establish compatibility with the real service.
- Report missing credentials or unavailable services as skipped live checks.
  Distinguish an environmental failure from a product regression using evidence.
