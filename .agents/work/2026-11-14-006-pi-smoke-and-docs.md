# Pi smoke check and docs

## Plan

[Pi smoke check and docs](../plans/2026-11-14-006-pi-smoke-and-docs.md)

## Summary

The plan's goal is met. The smoke script selects Claude or Pi while preserving
Claude's checks. Pi coverage includes thinking, skills with arguments, extension
dialogs answered as forms, cancellation, model verification, and session
lifecycle. The ACP chart describes both bridges; architecture, README, and the
authoritative testing guide describe Pi and its required live check.

Both Make targets explicitly select their agent; direct script invocations
default to Claude. The user-selected Pi default is
`openrouter/deepseek/deepseek-v4.1-flash-1017`.

## Departures from the plan

- The first live Pi run returned `OX_NATIVE_COMMAND_` instead of the expected
  marker. Pi's native transcript showed the skill expansion correctly appended
  `ARGUMENTS`, but DeepSeek interpreted that word as a label with an empty value.
  Clarified the skill to treat all text after `</skill>` as the value. The
  argument round-trip assertion remains unchanged, and the next full run passed.

## Checks run

- `make check-rust` — passed: formatting, clippy with warnings denied, `ox-mcp`
  build, and workspace tests, including eight Pi unit and 25 Pi protocol tests.
- `make check-scripts` — passed after Ruff formatting, and passed again after
  clarifying the Pi skill fixture. Initial lint passed; initial formatting
  differences were corrected.
- `make smoke-claude` — passed with `claude-haiku-5-5`, preserving all existing
  reports. Artifacts: `/private/tmp/ox-acp-mocbo6gk`.
- `make smoke-pi` — passed with the selected OpenRouter DeepSeek model after the
  fixture wording fix. Artifacts: `/private/tmp/ox-acp-rfp43rt2`. The run verified
  native skill arguments, form disconnect/replay/answer, cancellation, the exact
  transcript model, history and conversation after restart, idle parking,
  archive/unarchive, contiguous event sequences, and process/socket cleanup.
  Initial failed-run artifacts: `/private/tmp/ox-acp-1eeou8ew`.
- `python3 scripts/smoke-acp.py --help` — lists `--agent {claude,pi}`.
- `git diff --check` — passed. Inspected the documentation diff and all ten ACP
  tables; each includes the Pi column.

Each smoke artifact directory contains `results.json`, `events.jsonl`,
`attach.stderr.log`, `oxd.log`, bridge session records, and test projects.

## Manual verification

1. Queried installed Pi 1.4.0 in offline RPC mode, without a model request:

   ```sh
   pi --mode rpc --offline --no-session --no-extensions --no-context-files \
     --no-mcp --no-skills --no-prompt-templates --provider openrouter \
     --model deepseek/deepseek-v4.1-flash-1017
   ```

   Sent `get_available_models`, `get_available_thinking_levels`, and `get_state`
   RPC commands. The exact selected model is in the catalog and offers `off`,
   `low`, `high`, and `max`; the live `thinking low` check is supported.
