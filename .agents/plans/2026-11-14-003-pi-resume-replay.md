# Pi resume and replay

## Goal

Deliver [task 3, Resume and replay](../scratch/pi.md#3-resume-and-replay).

## Related code

- `crates/pi-acp/src/bridge.rs` — keeps session metadata and Pi handles together in memory; `start_pi` already reuses the native session ID after exit.
- `crates/pi-acp/src/pi.rs` — command correlation, process ownership, output translation, and turn state.
- `crates/pi-acp/src/updates.rs`, `content.rs`, and `tools.rs` — existing ACP message, image, thought, and tool translations.
- `crates/claude-code-acp/src/store.rs` and `config.rs` — bridge registry and storage-directory convention to follow.
- `crates/claude-code-acp/src/bridge.rs::resume_session` — replay/state/response ordering and supported replay cursor.
- `crates/oxd/src/state.rs::load` — restores saved Ox events when an `AgentStarted` event exists and otherwise uses native replay. Ordinary parking/reloading retains Ox's original displayed conversation.
- Installed Pi 1.4.0 under `/opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent`: `dist/modes/rpc/rpc-mode.js` exposes the actual session file and native ID through `get_state`; `dist/core/session-manager.js` persists message entries and writes session files lazily. This available install matches the task-2 plan; the scratch note's 1.5.2 install is unavailable.

## Decisions

- Keep this within `pi-acp`; native session import/listing, models/commands, extension elicitations, and smoke-script changes remain outside task 3.
- Add a small Pi-specific registry following Claude's locked read-modify-write and atomic replacement pattern. Persist ACP ID, `cwd`, native Pi ID, and title. Process handles and open-session membership remain in memory. Closing stops Pi and removes its open session while retaining the registry entry.
- Use `PI_ACP_STORE_DIR`, otherwise the existing absolute-XDG/home/temp convention under `pi-acp`, with `acp-sessions.json` and its lock. Save creation before answering `session/new` so the returned session can be resumed; save title changes with session metadata. Propagate registry I/O failures.
- Keep the existing `--session-id` launch behavior. Restoring the saved native ID is enough for Pi to continue its own conversation after an exit or a new bridge starts following parking.
- Obtain the actual native transcript path through a correlated Pi `get_state` request when replay needs it. This respects Pi's session-directory settings without reproducing its path discovery. Extend the existing pending-command map with this result; resolve it on response or process exit.
- Follow Claude's resume contract: validate the request, load the registered session, start Pi if needed, replay when `replayFrom: start`, send session state, then answer. Omitted replay sends no conversation; other cursors are rejected. Unknown ACP IDs are session-not-found. Use the existing state tracking. Ox starts a replacement bridge when loading a parked session.
- Native replay is the recovery source when Ox has no stored conversation. Replay recorded message entries in order, including messages before compaction, and omit Pi bookkeeping.
- Translate user content, assistant text/thoughts, and recorded tool calls/results using existing ACP conversions. Use native entry IDs, content-index suffixes for separate assistant blocks, and native tool-call IDs. Replay saved edit patches where present; historical writes retain available arguments, status, locations, and output without inferring creation from today's filesystem.
- A native file not yet written for an empty session yields empty replay. Propagate actual read/parse errors rather than claiming successful recovery.

## Check your assumptions

None.

## Test plan

- Give fake-Pi protocol tests isolated bridge storage and `get_state` replies naming temporary JSONL transcripts.
- Create and prompt a session, stop the bridge, start another bridge using the same storage, resume the ACP ID, and prove the same native ID is launched and the next prompt continues that session. This covers the bridge replacement used by parking. Also verify close retains resumability and existing after-exit restart behavior remains intact.
- Verify `replayFrom: start` emits representative user text/images, assistant text/thoughts, tool calls/results and saved edit patches, then state, then response. Include compaction bookkeeping and confirm earlier messages remain; historical tool presentation must depend on recorded data. Verify omitted replay and an empty session without a file.
- Verify unknown session IDs and unsupported replay cursors fail clearly, and a pending `get_state` fails if Pi exits. Check registry writes retain other sessions.

## Implementation plan

1. Add `src/config.rs` and `src/store.rs`; add necessary direct dependencies in `crates/pi-acp/Cargo.toml` and update `Cargo.lock`. Limit the registry API to get/create/update. Separate persisted metadata from live Pi handles in `bridge.rs`, preserving new/prompt/cancel/close behavior.
2. Add the correlated `get_state` result in `pi.rs`, settling it through existing response and exit paths. Expose the existing ACP running/idle state snapshot from `updates.rs` for the resume notification.
3. Add `src/history.rs` to read the native file and replay recorded messages through `updates.rs`. Keep saved-history conversion separate from streaming event state, reusing content/tool conversions when their inputs have the same meaning. Adjust `tools.rs` so historical writes can omit creation-specific presentation without inspecting current files.
4. Implement `Bridge::resume_session` and register its asynchronous handler in `main.rs`, advertising resume support through the schema's session capability. Restore saved metadata/native identity and send replay, state, and response in that order.
5. Extend `tests/protocol.rs` and focused history/store tests with the behavior above. Protocol chart and broader bridge documentation remain task 6; this change requires no new documentation contract beyond the existing separation of Ox history and native conversation.
