# Queued and steered prompts

## Plan

`.agents/plans/2026-11-16-005-queued-and-steered-messages.md`

## Summary

While an agent works, Enter queues the composer's prompt in `oxd` and Cmd+Enter steers it into the running turn. The daemon keeps each session's queue in `session_queues` and starts the first queued prompt each time the agent is idle with nothing pending. A cancelled turn holds the queue, and so does any turn that ends other than with `end_turn`; it stays held across restarts until a prompt is sent. Both bridges steer: Claude through `priority: "next"`, and Pi through `streamingBehavior: "steer"` with a FIFO of held prompts. The plan's goal is met.

## Departures from the plan

- **Pausing:** `Phase::after` does not take `&mut Queue`. `State::record` pauses the queue after `Change::Cancelled`, or when the events a change records include an abnormal `TurnEnded`. This catches every end in one place instead of in each arm.
- **`request()` return value:** `State::request` returns the prompt to send, with its seq, as `Option<(Vec<ContentBlock>, u64)>`. `Session::prompt` and `queue_send` share `send_prompt`.
- **`fill`:** when it ends a turn that a crash cut off, it also pauses the queue. That turn ends with an error, so the plan's pausing rule applies.
- **Holding prompts while busy:** `queue_add` and `queue_send` take the same `refuse_moving` check as `session.prompt`, under the registry lock, through `Daemon::send_unmoved`.
- **Edits that can no longer be saved:** an edit whose prompt has left the queue (the drain sent it while the user was editing) is sent as a new message. Enter queues it, or sends it on an idle session, and then brings back the held draft. Without this, the error would repeat on every Enter with no way out of edit mode. A failed save while the prompt is still queued keeps the edit, as planned. `useDrafts` also keeps an editing draft whose text is empty. The first `make e2e` run showed why: clearing the box dropped edit mode, and the save then queued a duplicate.
- **Pi bridge:** the bridge agent added an `Output::starting` counter. A user `message_start` for a prompt Pi started then does not answer a held steer that Pi queued before the run's opening message. While a cancel is in flight, `abort_on_start` stays set until Pi has answered every prompt. Steers still held when Pi settles are left alone, because Pi delivers them before settling.
- **Smoke ordering:** `check_steer` and `check_queue` run before `check_cancel_generation`. Their prompts begin "New task:" because Pi's model treated a cancelled "Do not use tools" request still in its context as contradicting the shell task. The tool wait accepts `pending` as well as `in_progress`, because Claude's bridge reports Bash as `pending` then `completed`.

## Decisions

- `fake-agent`'s `take` waits until a prompt is held, so tests synchronize on the steer instead of on timing.
- Prompts held when a turn ends are inserted in the agent's own turns, under the turn lock. This leaves no gap for a new prompt to slip in unheld.
- `QueueAdd` and `QueueEdit` check prompt capabilities only while the agent is live. A restored session's saved capabilities are empty, so outside a live agent they check only for an empty prompt.
- The queue list has the request panel's geometry, in a shared rule, with the regular border colour. Its row actions are hidden until the row is hovered or focused.
- Escape is a window `keydown` listener added while the session or its parent run is running. It ignores events that were already default-prevented, and Escape while a popover is open: the model menu is a native `popover="auto"`, which closes on Escape without preventing the default.
- `oxctl queue ls` joins a prompt's text blocks with spaces and flattens newlines, so each prompt stays on one line.

## Checks run

- `make check-rust`: passed.
- `make check-ui`: passed (66 tests), including after the popover change.
- `make check-scripts`: passed.
- `make e2e`: passed (4 spec files), including the new queue case. The first run failed on the edit-mode bug above; it was fixed and rerun.
- `make smoke-claude`: two runs. The first failed in `check_steer`: the script waited for `in_progress`, which Claude's bridge never sends for Bash. In the second, `steered_prompt_taken_into_turn` ("done steered") and `queued_prompt_starts_next_turn` ("queued") passed, along with every check before them. It then failed in `check_workflow`. Claude's transcript shows step 2 got the prompt and the token `ox-acp-b4tam0q9-workflow`, and Haiku answered "Smoke handoff". That is a model error, not a regression.
- `make smoke-pi`: four runs; the fourth passed in full, including both new checks. The first failed in `check_steer`, where the model refused the shell task because of the cancelled request still in its context; the checks were reordered as described above. The second failed in `check_commands`: the model refused the skill probe as a prompt injection. The third failed in `check_dialog`: the form was answered and resolved, but the model replied "ox-color" instead of "ox-color-Blue". Neither of those checks involves steering or the queue.

## Manual verification

1. The queue list and a dimmed steered prompt render in mock mode.

   ```sh
   cd ui && npm run mock   # open ?scenario=sidebar, view "Fix sidebar colors", hover a queued row
   ```

   The two queued rows rise from behind the composer's box, with Edit, Send Now and Delete on the hovered row. The attachment name `tokens.css` is muted. The steered prompt is dimmed at the end of the transcript.

## Follow-up work

- The drain does not check `refuse_moving` for a branch move in progress (a `TODO` in `State::start_queued`).
- When a client cancels just as Claude preempts a turn, the turn ends cleanly only if Claude sends another `result`. The bridge does not handle this case.
- That Escape with the model menu open closes the menu without stopping the turn is verified by inspection only, since jsdom does not open native popovers. Check it in `make dev`.
