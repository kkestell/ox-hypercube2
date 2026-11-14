# Harness detection and agent choice

## Plan

`.agents/plans/2026-11-14-001-harness-detection.md`

## Summary

The app's `agents` command reports Claude and Pi when `claude` and `pi` are executable files on the `PATH` sessions get, and the session list asks on mount and on each window `focus`. The New Task button is disabled with no agent, the Harness select shows only with two, and `new_session` starts each agent's bridge beside the app's executable. The e2e app runs with a stub login shell and a controlled `PATH`. The plan's goal is met.

## Decisions

- With no installed agent, the New Task popover is not rendered at all, so its menu items never see an undefined agent; the disabled button cannot open it anyway.
- `useAgents` keeps the previous array when a focus check returns the same agents, so focusing the window does not re-render the session list.
- The unit test's second-directory case uses an executable `codex`, since the second directory already holds the `pi` directory case.

## Checks run

- `cargo test -p ox --lib` — 16 passed, including `finds_executable_harnesses_on_path`.
- `make check-ui` — passed.
- `make check-rust` — passed.
- `make e2e` — 12 spec files passed, including `harness.e2e.ts`.

## Manual verification

1. The e2e app ignores the user's `PATH`. The machine has `claude` at `~/.local/bin/claude` and `pi` at `/opt/homebrew/bin/pi`, yet the spec "is disabled with no harness installed" passes, so the stub login shell kept the controlled `PATH`.

   ```sh
   which claude pi
   make e2e
   ```

   Both harnesses resolve on the shell `PATH`; `harness.e2e.ts` passes all three tests.

The real app was not opened by hand with Pi installed; choosing Pi starts `pi-acp`, which does not exist until task 2 of `.agents/scratch/pi.md`.

## Follow-up work

- Task 2 of `.agents/scratch/pi.md`: build `pi-acp` beside the app's executable in `make dev` and `make install`.
