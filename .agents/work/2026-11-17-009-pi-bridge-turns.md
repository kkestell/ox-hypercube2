# Pi bridge turn coordination

## Plan

`.agents/plans/2026-11-17-010-pi-bridge-turns.md`

## Summary

`Turns` owns prompt and config dispatch and tracks Pi's outstanding work.
Prompts wait for preceding config changes and cancelled work to finish.
The bridge withdraws open dialogs and declines new dialogs during cancellation.
The Structure item and both related bugs are checked off.

## Decisions

- Send each form and register its task under the same lock that guards
  cancellation. Cancellation can find every form the bridge has issued.
- Clear the count of starting user messages when Pi settles. That count belongs
  to the run that just ended.

## Checks run

- `make check-rust` passed, including 13 Pi unit tests and 31 Pi protocol tests.
- `make smoke-pi` stopped at `hello`. The tracked smoke script sends the obsolete
  integer protocol version. The daemon expects a string build hash.
- The live smoke run with the current handshake passed every phase on Pi 1.5.2
  and OpenRouter, including selecting the expected model, changing effort,
  answering an extension dialog, steering, and cancelling generation.
  Results are in `/private/tmp/ox-acp-jywe8ouj/results.json`.

## Manual verification

1. Copy only the updated `crates/pi-acp/tests/protocol.rs` into a detached
   checkout of `2cc5006` and run there.

   ```sh
   cargo test -p pi-acp --test protocol a_cancel_fails_a_prompt_waiting_for_a_model_change -- --exact
   cargo test -p pi-acp --test protocol cancel_withdraws_open_dialogs_and_refuses_later_ones -- --exact
   ```

   Both failed on `2cc5006` and passed with the change. The first fake received
   a prompt where it expected `clear_queue`. The second test received an
   unexpected second `elicitation/create`.

2. The live check used the smoke script with only the handshake value replaced
   in memory. The hash came from the protocol crate's Cargo build output.

   ```sh
   python3 - --agent pi --model openrouter/deepseek/deepseek-v4.1-flash-1017 <<'PY'
   from pathlib import Path
   import sys

   script = Path("scripts/smoke-acp.py").resolve()
   sys.argv[0] = str(script)
   source = script.read_text().replace("protocol_version=1", 'protocol_version="3fe66f1fbe2c5910"', 1)
   exec(compile(source, str(script), "exec"), {"__file__": str(script), "__name__": "__main__"})
   PY
   ```

   The run ended with `{"check": "passed"}`. The protocol regressions above
   exercise the model and dialog interleavings directly.

## Structural simplification after review

Replaced the generic `Turns<P>` and its action interpreter with one concrete
`Turns` owner for dispatch, prompt replies, and dialogs. State transitions and
their effects run under the same lock. Config blocking derives from pending
changes, removing the separately maintained `changing` flag. Shared prompt
completion and request failure handling replace repeated response plumbing.

The three coordinator unit tests were replaced by assertions and controlled
interleavings in existing protocol tests. FIFO gates hold Pi's answers until a
later ACP response proves the bridge has registered the queued work; child
processes open those gates under a timeout, so no runtime thread blocks opening
a FIFO. Coverage includes model/effort/prompt ordering, idle cancellation,
cancellation before a handled or started prompt's reply, and a subsequent prompt
surviving the earlier cancellation.

- `make check-rust` passed, including 10 Pi unit tests and 32 protocol tests.
- `make smoke-pi` still failed at the existing obsolete handshake.
- With the current handshake substituted in memory as above, the live run
  passed model verification, effort changes, forms, steering, queueing, and
  cancellation. It then failed the title-tool check: the model asked which
  instruction to follow rather than invoking `set_title`. The event log shows
  its text response and no title-tool call. The full live suite therefore did
  not pass on this revision; results are in
  `/private/tmp/ox-acp-s9bgmj7_/results.json`.
