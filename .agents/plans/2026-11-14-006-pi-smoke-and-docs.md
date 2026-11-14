# Pi smoke check and docs

## Goal

Deliver [task 6, Smoke check and docs](../scratch/pi.md#6-smoke-check-and-docs). `scripts/smoke-acp.py` runs its live checks against either bridge, chosen with `--agent`. `make smoke-pi` runs it against `pi-acp` on the Pi smoke model, and `make smoke-claude` runs the same Claude checks as today. The ACP support chart gains a `pi-acp` column. Architecture and README describe the Pi bridge. The testing guide's required checks include `make smoke-pi`.

## Related code

- `scripts/smoke-acp.py` — one `SmokeRun` class hard-wired to Claude:
  - `__init__` (228–246) writes `.claude/skills/ox-command-probe`, launches `claude-code-acp`, and sets `CLAUDE_ACP_STORE_DIR`.
  - `run` (341–392) checks the command probe on the first watch listing (356–366), then calls the checks in a fixed order.
  - `check_parking` (696–776) builds a second `SmokeRun(...)`.
  - `verify_model` (871–891) reads Claude's transcript through `claudeSessionId`.
  - `main` (924–963) checks `claude` is on `PATH` and builds `claude-code-acp`.
- `Makefile` — `smoke-claude`, `CLAUDE_MODEL`, `.PHONY`, and the `help` text.
- `crates/pi-acp/src/` — the facts for the new column:
  - `main.rs` handles `initialize`, `session/new`, `resume`, `close`, `set_config_option`, `prompt`, and `cancel`, and nothing else.
  - `bridge.rs`: capabilities (image prompts only, no MCP), MCP servers rejected, the title taken from the first prompt, `describe` / `publish_description`, `set_config`, and `start_pi`.
  - `pi.rs`: prompt acceptance, `followUp` queuing, `abort_on_start`, stop reasons in `settled`, exit handling, and dialogs.
  - `updates.rs`: states, `background: []`, and `requires_action` while asking.
  - Also `options.rs` (`model` as `provider/id`, and `thinking`), `commands.rs`, `content.rs`, `tools.rs`, `history.rs`, `dialogs.rs`, and `config.rs` (`PI_ACP_STORE_DIR`).
- `crates/oxctl/src/main.rs` — `config` prints the option's new current value. `answer` takes the form content as JSON.
- Docs: `docs/dev/acp-v2.md`, `docs/dev/architecture.md`, `docs/dev/testing.md` ("Required commands"), and `README.md`. The scratch's `docs/acp-v2.md` and `docs/architecture.md` are these `docs/dev/` files. `AGENTS.md` names no commands; it defers to `docs/dev/testing.md#running-checks`, which is the authoritative home for check instructions.
- Pi facts, verified locally against installed Pi 1.4.0 (`/opt/homebrew/bin/pi`):
  - `set_model` matches `provider` and `modelId` exactly. Installed Pi offline RPC confirms `openrouter/deepseek/deepseek-v4.1-flash-1017` is available and offers `off`, `low`, `high`, and `max` thinking levels.
  - Session files are `<agentDir>/sessions/<encoded-cwd>/<timestamp>_<sessionId>.jsonl`. `<agentDir>` is `$PI_CODING_AGENT_DIR`, or `~/.pi/agent` by default. Assistant `message` entries carry `provider` and `model`.
  - Project skills come from `.pi/skills/<name>/SKILL.md` and are invoked as `/skill:name args`; Pi appends the argument text after the skill body. Project extensions come from `.pi/extensions/*.ts`. A `tool_call` handler may await `ctx.ui.select` and change `event.input`; see `examples/extensions/permission-gate.ts`.

## Decisions

- **One agent class per bridge.** `SmokeRun` keeps everything that does not depend on the agent:
  - process and attachment handling, `run`, `cleanup`, `session_row`, and `remember_agents`;
  - `check_reconnect`, `check_load_after_restart`, and `check_archive`;
  - a new `check_cancel_generation`: the second half of today's `check_cancellation`, with the report `cancel_running_generation`.

  `ClaudeRun(SmokeRun)` and `PiRun(SmokeRun)` provide these class attributes:
  - `harness`: `claude` / `pi`;
  - `bridge`: `claude-code-acp` / `pi-acp`;
  - `store_variable`: `CLAUDE_ACP_STORE_DIR` / `PI_ACP_STORE_DIR`;
  - `probe`: `ox-command-probe` / `skill:ox-command-probe`.

  Each subclass also provides these methods:
  - `write_project(self)`: writes the probe skill and, for Pi, the extension;
  - `check_probe(self, command)`: what the catalog entry must hold;
  - `checks(self)`: the ordered list of check methods;
  - `verify_model(self)`.

  `AGENTS = {"claude": ClaudeRun, "pi": PiRun}`. `check_parking` builds its second run with `type(self)(...)`.
- **Claude checks keep their order and behavior.** `ClaudeRun.checks` returns this order: `check_reconnect`, `check_effort`, `check_commands`, `check_permission`, `check_question`, `check_background_wake`, `check_cancel_permission`, `check_cancel_generation`, `verify_model`, `check_title_tool`, `check_load_after_restart`, `check_parking`, and `check_archive`. `check_cancel_permission` is the first half of today's `check_cancellation`, with the report `cancel_pending_permission`. The Claude-only methods move unchanged into `ClaudeRun`.
- **Pi checks.** `PiRun.checks` returns this order: `check_reconnect`, `check_thinking`, `check_commands`, `check_dialog`, `check_cancel_generation`, `verify_model`, `check_load_after_restart`, `check_parking`, and `check_archive`.
  - Pi has no permission requests, background work, or `ox` MCP server, so there are no checks for them. `pi-acp` titles the session from its first prompt, which satisfies `check_load_after_restart`'s title precondition.
  - `check_thinking`: `config <session> thinking low` prints `low`. The selected DeepSeek model offers this level. Report `thinking_set_on_running_pi`.
  - `check_commands`: the prompt `/skill:ox-command-probe ARGUMENTS` replies exactly `OX_NATIVE_COMMAND_ARGUMENTS`. Report `native_skill_invoked_with_arguments`.
    - Pi has no counterpart of Claude's `/model` local command in RPC mode, so that half has no Pi version.
    - The skill body, at `.pi/skills/ox-command-probe/SKILL.md`, tells the model to reply with `OX_NATIVE_COMMAND_` immediately followed by the argument text given after the skill, and nothing else, without tools. Its frontmatter has the `name`, the description `Return the smoke command marker`, and `disable-model-invocation: true`.
  - `check_probe`: Claude keeps today's `input` hint and description assertions. Pi asserts that `smoke command marker` is in the description.
  - `check_dialog`: a live check of the extension-dialog path, the counterpart of `check_question`.
    - `write_project` writes `.pi/extensions/ox-color.ts`. Its `tool_call` handler acts only on a `bash` call whose command contains `ox-color`. It awaits `ctx.ui.select("Which color?", ["Red", "Blue"])` and sets `event.input.command` to `` `echo ox-color-${color}` ``.
    - The prompt asks the model to run exactly `echo ox-color` with the bash tool, then reply with only the command's output.
    - The check waits for `elicitation_requested` and asserts `awaiting_input` in `oxctl ls`. It detaches and reattaches at `seq - 1`, and checks that the replayed elicitation equals the original. `Blue` must be in `requested_schema.properties.answer.oneOf[].const`.
    - It answers `{"answer": "Blue"}` with `oxctl answer`, finishes the turn, asserts that an `elicitation_resolved` was recorded, and asserts that `ox-color-Blue` is in the reply. Report `extension_dialog_answered_as_a_form`.
  - Pi's `check_parking` covers an idle session with no background work:
    - In the parking daemon, the session is prompted `Reply with only ready. Do not use tools.` and the turn finishes.
    - The client detaches and a `Watch` waits until `exited`.
    - The client reattaches at 0 and reads until `agent_parked`. Report `idle_session_parks`.

    Claude keeps its background-work version. A shared `SmokeRun.parking_run(self, delay)` holds the setup both versions repeat: the directory, `type(self)(...)`, `start_daemon`, the workspace, `new`, `config model`, `remember_agents`, and `attach(0)`. It returns the run, and the caller still runs `run.cleanup()` in its `finally`.
  - Pi's `verify_model` reads `piSessionId` from `bridge-store/acp-sessions.json`. It finds the session file under `$PI_CODING_AGENT_DIR` (or `~/.pi/agent`) at `sessions/*/*_<piSessionId>.jsonl`, and asserts that the set of `provider/model` across assistant message entries equals `{args.model}`.
- **The command catalog is waited for.** `run` reads watch listing changes until the session's `available_commands` contains `probe`, bounded by `--timeout`, and then calls `check_probe`. `pi-acp` publishes its catalog asynchronously after Pi starts, so the first listing can predate it.
- **Arguments and build.**
  - `--agent {claude,pi}` selects the bridge and defaults to `claude`, preserving existing direct invocations. Both Make targets pass their agent explicitly; `--model` and `--timeout` keep their existing meanings.
  - `main` checks that `harness` is on `PATH`, and builds `oxd`, `ox-mcp`, `oxctl`, and the class's `bridge`.
  - The module docstring says the script runs smoke checks against an ACP bridge through `oxd` and `oxctl`.
  - The artifact directory and report format are unchanged.
- **Makefile.**
  - Add `PI_MODEL ?= openrouter/deepseek/deepseek-v4.1-flash-1017`, as selected by the user; keep it overridable.
  - `smoke-claude` passes `--agent claude --model "$(CLAUDE_MODEL)"`.
  - A new `smoke-pi` passes `--agent pi --model "$(PI_MODEL)"`.
  - Add `smoke-pi` to `.PHONY` and add `make smoke-pi        Run live Pi ACP smoke checks` to `help`.

## Check your assumptions

1. **The Pi smoke provider and model — accepted user decision.**
   - **Decided:** The user selected OpenRouter with `deepseek/deepseek-v4.1-flash-1017`. The exact ACP model option value and Make default are `openrouter/deepseek/deepseek-v4.1-flash-1017`.
   - **Consequences:** Live Pi checks use that model through the user's configured OpenRouter credentials. This supersedes the scratch's GPT-6 Luna smoke model; no alternate provider or model is implied.
   - **Verified:** Installed Pi 1.4.0 offline RPC offers the exact selected model with thinking levels `off`, `low`, `high`, and `max`; the planned `thinking low` check applies. Report unavailable credentials clearly without adding a fallback.
2. **What `smoke-pi` covers.**
   - **Decided:** `smoke-pi` covers the following: reconnect and replay, conversation continuity, thinking level, a project skill with arguments, an extension dialog answered as a form, cancelling a running generation, the model check, history and resume after a daemon restart, idle parking, and archive and unarchive. These are `smoke-claude`'s checks wherever Pi has the feature.
   - **Consequences:** Permissions, background work, and the `ox` MCP title tool stay Claude-only, because Pi has none of them.
   - **Alternative:** Run only the agent-independent checks for Pi: reconnect, cancellation, load, and archive.
   - **Trade-offs:** The fuller set is the only live check of tasks 3 through 5 against real Pi, at the cost of a longer run and three Pi-specific checks.
3. **The live extension-dialog check depends on the model calling bash.**
   - **Decided:** A project extension's `tool_call` handler asks the question and rewrites the command, following Pi's `permission-gate.ts` example.
   - **Consequences:** The check depends on the model calling `bash` as told, as `check_question` depends on Claude calling AskUserQuestion. The smoke project gains a `.pi/extensions` file.
   - **Alternative:** Skip a live dialog check and rely on `crates/pi-acp/tests/protocol.rs` and its fake Pi.
   - **Trade-offs:** The live check proves the real `extension_ui_request` shape and the answer's round trip. Skipping it shortens the run and removes a model-dependent step.
4. **Which changes require `smoke-pi`.**
   - **Decided:** The testing guide requires `make smoke-pi` after changing `oxd`, `ox-protocol`, `oxctl`, `pi-acp`, or `scripts/smoke-acp.py`. `make smoke-claude` stays required for the same shared crates and the script, plus `claude-code-acp`.
   - **Consequences:** Every change to `oxd`, `ox-protocol`, `oxctl`, or the smoke script now needs two live runs and both harnesses authenticated.
   - **Alternative:** Require `smoke-pi` only for changes to `pi-acp`.
   - **Trade-offs:** Running both catches daemon regressions that only one bridge exposes. Running one keeps shared-crate changes to a single live run.

## Test plan

- `make check-rust` and `make check-scripts` pass, as required by the scratch task's checks for tasks 2 through 6.
- `make smoke-claude` passes with the same reported checks as before, except that `cancel_pending_permission` and `cancel_running_generation` now come from two methods.
- `make smoke-pi` passes all Pi checks and ends with `passed`. Report the artifact directory. Without credentials for the selected provider, report it as a skipped live check.
- Inspect the doc diffs for the facts listed below.

## Implementation plan

1. `scripts/smoke-acp.py`:
   - Split `SmokeRun` into the base class and `ClaudeRun` / `PiRun` as decided.
   - Split `check_cancellation`.
   - Add `parking_run`, the catalog wait, `--agent`, and the Pi project files and checks.
2. `Makefile`: `PI_MODEL`, the `smoke-claude` arguments, the `smoke-pi` target, `.PHONY`, and `help`.
3. Run `make check-rust`, `make check-scripts`, `make smoke-claude`, and `make smoke-pi`.
4. Update the docs below.

## Documentation updates

- `docs/dev/acp-v2.md` — add a `pi-acp` column after `claude-code-acp` in every table.
  - Update the intro paragraph that lists the columns.
  - Caveats that apply to one agent take a `pi-acp:` prefix, as `claude-code-acp:` caveats do. Prefix any existing unprefixed caveat that no longer holds for every column. For example, `resource_link` becomes "Sent to the harness as a Markdown link, `[@name](uri)`."
  - The `pi-acp` cells and caveats:
  - **Client methods:**
    - `initialize`: supported. Answers with the client's protocol version, and notes whether the client advertises `elicitation.form`.
    - `session/new`: supported. Answers, reports the session idle, then starts `pi --mode rpc --approve` on a new Pi session ID. The config options and command catalog follow as `config_option_update` and `available_commands_update`. A Pi that fails to start is started again by the next prompt. MCP servers are rejected with `-32602`.
    - `session/list`: unsupported.
    - `session/resume`: supported.
      - Starts Pi if needed; a spawn failure fails the request.
      - With `replayFrom: start`, it replays the message entries of Pi's session file in file order: user messages, text and thinking as whole messages, and tool calls with their results and `edit`/`write` diffs.
      - It sends the state and answers with the config options and command catalog.
      - Any other cursor, a different `cwd`, or MCP servers are rejected with `-32602`.
    - `session/close`: supported. Aborts the run, stops Pi, and answers once Pi has exited; the open turn ends `cancelled`. The session stays resumable. Closing a session that is not open succeeds.
    - `session/prompt`: supported.
      - Answers with the user message's `messageId` once Pi accepts the prompt.
      - A prompt sent during a run is queued by Pi as a follow-up and answered when queued.
      - A prompt an extension command handles without starting a run ends its turn at once.
      - Pi's refusal is the error.
    - `session/set_config_option`: supported. Options `model`, with values of the form `provider/id`, and `thinking`, offered when the model has more than one level. Both are selects.
    - Every other row: unsupported.
  - **Client notifications:** `session/cancel` is supported. It sends Pi's `abort`, and aborts again once Pi accepts a prompt that was still waiting. Open dialogs are answered cancelled and their elicitations withdrawn. Every other row: unsupported.
  - **Agent methods:**
    - `session/request_permission`: unsupported. Pi runs tools without asking.
    - `elicitation/create`: supported.
      - Pi extension `select`, `confirm`, `input`, and `editor` dialogs become a form with the message `Pi has a question` and one field, `answer`, titled with the dialog's title:
        - `select`: a required `oneOf` of its options;
        - `confirm`: a required Yes/No choice described by its message;
        - `input`: optional text described by its placeholder;
        - `editor`: optional text defaulting to its prefill.
      - Declining or cancelling answers Pi cancelled.
      - A client without `elicitation.form` gets no elicitations; dialogs are answered cancelled.
  - **Agent notifications:**
    - `session/update`: supported.
    - `$/cancel_request`: supported. Withdraws an elicitation when the dialog's `timeout` elapses, on cancel or close, when an aborted run settles, or when Pi exits.
  - **Session updates:**
    - `user_message`: supported. Sent when Pi accepts a prompt, and on replay.
    - `agent_message` and `agent_thought`: supported. Sent for text Pi did not stream, and on replay.
    - `agent_message_chunk` and `agent_thought_chunk`: supported. Sent while Pi streams.
    - `state_update`: supported.
    - `tool_call_update`: supported.
      - The first update has the name, title, kind, `pending`, input, and file location (with the line for `read`), followed by `in_progress` when Pi runs the tool.
      - The result sets `content`: a diff for `edit` and `write`, then the result's text and images.
      - Calls that end without a result are failed.
    - `available_commands_update`: supported. Pi's extension commands, prompt templates, and skills (as `skill:name`) with descriptions, sent each time Pi starts and included in resume responses.
    - `config_option_update`: supported. Sent when Pi starts and after a config change.
    - `session_info_update`: supported. `title` from the first prompt.
    - Every other row: unsupported.
  - **State updates:**
    - `running`: supported. Sent on Pi's `agent_start`, when Pi accepts a prompt, and when a form closes during a run.
    - `idle`: supported. Sent on `agent_settled` with its stop reason. A Pi that exits during a turn ends it with `error`, or with `cancelled` when the session is closing.
    - `requires_action`: supported. Sent while a dialog's form is open during a run.
    - `background`: supported. Always `[]`, so idle Pi sessions can park.
    - Every other row: unsupported.
  - **Stop reasons:**
    - `end_turn`: supported.
    - `max_tokens`: supported, from Pi's `length`.
    - `cancelled`: supported. The run was aborted, or the session closed.
    - `error`: supported. Pi's error message, or why Pi exited, with its stderr.
    - Every other row: unsupported.
  - **Prompt content blocks:**
    - `text`: supported.
    - `resource_link`: supported, as a Markdown link.
    - `image`: supported, sent as a Pi image.
    - `resource` and `audio`: unsupported, rejected with `-32602`.
  - **Agent capabilities:**
    - `session`: supported.
    - `session.prompt.image`: supported.
    - Every other row: unsupported. Without `session.mcp.stdio`, Pi sessions get no `ox` MCP server.
  - **Client capabilities:** `elicitation.form` is supported. It decides whether Pi's dialogs become forms.
- `docs/dev/architecture.md`:
  - The diagram: `oxd` owns `claude-code-acp` → `claude` → `ox-mcp`, and `pi-acp` → `pi`, each labelled one per session.
  - "Daemon and agents" describes both bundled bridges. Add `pi-acp`, which runs one `pi --mode rpc --approve` process per session, so that project settings, skills, and extensions load. It exchanges JSONL commands and events with that process, uses the providers and credentials configured in Pi, and can restart an exited Pi on the same Pi session ID.
  - "Persistence and recovery": continuing a Pi session requires its Pi session file, and each bridge keeps its session records separately from the daemon's database.
- `docs/dev/testing.md`, "Required commands":
  - Add the `make smoke-pi` requirement (assumption 4) beside `make smoke-claude`.
  - State the default `openrouter/deepseek/deepseek-v4.1-flash-1017` and that an installed Pi authenticated for OpenRouter is required.
  - Describe the coverage of each run.
  - Name `PI_MODEL` beside `CLAUDE_MODEL`.
  - Give the direct invocation with `--agent`.
- `README.md`:
  - Add a Features bullet for the Pi bridge: model and thinking controls, skills and extension commands, and extension dialogs as forms.
  - Add a sentence under Installation that Pi must be installed, with a model provider authenticated, to use the Pi bridge.
  - Correct the `make dev` paragraph to "the Claude and Pi bridges".
  - Correct the `oxctl` row to "live smoke tests".
