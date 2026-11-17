# Claude Code bridge

The Claude Code bridge lets Ox and other ACP clients use an installed, authenticated Claude Code through the user's Claude Code subscription. Standalone clients select the `claude-code-acp` command as their agent.

## Workflows

### Connect and authenticate

1. Install and authenticate Claude Code before using the bridge. Authentication is performed with Claude Code, not through a bridge login screen.
2. Configure the ACP client's agent command to the bridge executable. The bridge supports baseline draft-v2 session operations and supplies image, embedded-context, and stdio MCP capabilities.
3. Create a session in an existing absolute directory. A relative or missing directory is rejected.
4. The session becomes available with its configuration controls before native command discovery finishes. A later command catalog arrives when Claude initializes; discovery sends no user prompt. A native startup failure is reported on the next prompt.

Inherited `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` do not override the subscription authentication path. The bridge does not offer ACP login/logout, provider switching, or audio prompts.

### Describe a new session

- `claude-code-acp --describe <dir>` prints one JSON object, `{"configOptions": [...], "availableCommands": [...]}` in ACP's schema, for a new session in `<dir>`, and exits. `<dir>` is an existing absolute directory.
- The options are the [models, effort, and modes](#models-effort-and-modes) at their defaults. The commands are the catalog Claude reports when it initializes in `<dir>`.
- Claude starts without saving a conversation and stops once it has answered. No prompt is sent.
- A Claude that fails to start or refuses to initialize makes the bridge exit non-zero with the error on standard error.

### Discover and open conversations

- A standalone client can list bridge-created sessions and native Claude Code conversations, optionally restricted to a working directory. Results are ordered by most recent update, with session ID breaking ties, and contain up to 50 entries per page. An invalid continuation cursor is rejected.
- A bridge conversation continuing a native conversation appears once, under its bridge session ID. Every listed ID can be used to request a resume.
- New sessions start fresh. Continuing requires an explicit resume with the session ID and its original directory. A mismatched directory is rejected; a missing conversation is reported as not found.
- Close stops the session's Claude work and waits until it has stopped, but leaves the conversation resumable. Closing an already-closed session succeeds. A closed session must be resumed before accepting another prompt.
- Multiple standalone bridge clients using the same configured session collection preserve each other's sessions. Failures reading or updating that collection are reported as internal errors rather than silently starting over.

`CLAUDE_ACP_STORE_DIR` selects the bridge session collection; its default is `$XDG_DATA_HOME/claude-code-acp` for an absolute `XDG_DATA_HOME`, otherwise `~/.local/share/claude-code-acp`. `CLAUDE_CONFIG_DIR` selects where native conversations are discovered, defaulting to `~/.claude`. These choices do not change which Ox workspace a session belongs to.

### Continue and replay conversations

- Resume can request the conversation from its start. Replay includes user messages, replies, thinking, tool arguments, tool results and images, and reported subagent activity. Tool calls without a result are marked Failed.
- Replay is delivered before the current session state and resume acknowledgement. Without a replay request, no conversation is replayed. Cursors other than the start are rejected.
- Resource links and embedded text replay as the text Claude received; an embedded PDF replays as a document resource.
- A failed resumed prompt reports the failure; it is not retried in a fresh conversation.
- Continuing requires Claude Code still to retain its conversation. Claude Code's retention setting, normally 30 days, can prevent continuation even while [Ox's history](sessions.md#history-and-service-restarts) remains readable.

### Models, effort, and modes

The bridge exposes these per-session choices:

| Control | Choices | Default |
| --- | --- | --- |
| Model | Fable 5.2 (`claude-fable-5-2`), Opus 5.5 (`claude-opus-5-5`), Sonnet 5.5 (`claude-sonnet-5-5`), Haiku 5.5 (`claude-haiku-5-5`) | Opus 5.5 |
| Effort | Low (`low`), Medium (`medium`), High (`high`), XHigh (`xhigh`) | High |
| Mode | Auto (`auto`), Manual (`manual`), Accept edits (`acceptEdits`), Plan (`plan`) | Manual |

Auto lets Claude assess actions automatically; Manual asks before edits and commands; Accept edits allows file edits automatically; Plan explores and plans without editing files. Claude applies the selected mode's permission policy.

Changes are sent to active Claude work. A refusal reports Claude's error and does not replace the accepted setting. Accepted model, effort, and mode selections survive later resuming. Unknown options, values, or non-selection values are rejected.

### Send content and commands

- Text, resource links, embedded text, PNG, JPEG, GIF, WebP, and embedded PDFs are supported, retaining their prompt order. Other images, embedded binary types, audio, unknown content, and prompts consisting only of whitespace text are rejected.
- Resource links give Claude a link it can open with its tools; embedded text gives it the text directly, and embedded images and PDFs give it image or document content.
- Native commands and user-invoked skills run as ordinary prompt text, such as `/review path`. Their available names, descriptions, and argument hints follow Claude's current command catalog, including later replacements or clearing it. The bridge does not force skill discovery to reload when files change.
- The first nonempty text prompt titles an untitled session by collapsing whitespace and taking up to 120 characters. Attachment-only prompts do not supply a title.

### Claude questions and permissions

- When Claude asks for tool permission, the client is offered Allow once and Reject once, with Allow always when Claude supplies a rule it can save. The request identifies the tool and its input.
- For a client supporting forms, Claude's questions appear as a form rather than a generic tool permission. Each question offers its supplied single or multiple choices and an Other text field; all fields are optional.
- A single-choice answer uses Other text when present, otherwise the chosen option. A multiple-choice answer joins selections in offered order followed by Other text. Blank questions have no answer. Decline or Cancel denies the question tool.
- Without client support for forms, Claude's structured question tool is unavailable and questions are asked in plain text.

### Work and interruption

- A standalone ACP prompt is acknowledged with its user-message ID when Claude takes it into the conversation, not when the turn finishes. Its output and completion arrive separately.
- A standalone prompt submitted during another turn waits for Claude to take it; [Ox sessions](agents.md#turn-behavior) instead refuse a second prompt while busy.
- Streaming replies and thinking appear as they arrive. Tool calls report a name, title, arguments, and status, then text and image output. Unfinished tool calls are marked Failed when a turn ends.
- A subagent's tool activity appears within the tool call that started it, ahead of the subagent's final result. A subagent tool requiring permission can appear separately so it can be answered.
- Interrupt stops the current turn and its running tools, sends withdrawals for open permission requests and forms, and drops prompts Claude has not yet taken. Dropped prompts are answered as cancelled. Further prompts can continue the conversation. Ox's handling of withdrawals is described in [permissions and questions](requests.md#answer-a-permission-request).
- Ordinary completion, cancellation, a per-turn request limit, or an error is reported as the corresponding turn ending. Failures include Claude's reported reason or its exit error.
- Claude remains available between turns. If it exits, the next prompt continues the saved conversation when one exists; if no conversation was saved yet, it starts fresh.

### Background work and schedules

- The bridge reports Claude's live background tasks and scheduled prompts separately from whether a turn is running. A task's tool call remains in progress, or pending for a schedule, until that work ends; finishing its creating turn does not finish the task.
- A completed background task can start another turn, whose output and requests appear normally without a new user prompt. Reported background work prevents [automatic parking](sessions.md#leave-a-session-running).
- A recurring schedule stays reported until deleted or until a turn beginning at least seven days after creation finishes.
- A one-shot schedule or wakeup stays reported until a turn starting at or after its due time finishes. A wakeup turn without another explicit wakeup leaves one fallback wakeup 20 minutes later, with no further fallback after that. Stopping wakeups removes all wakeups.
- A scheduling result the bridge cannot interpret stays reported until Claude exits. After resume, recurring schedules younger than seven days and one-shot schedules not yet due are reported again.
- Durable schedules belonging to another session in the same directory are not included in this session's reported background work.

### MCP servers

Standalone ACP clients can supply stdio MCP servers when opening a session. Each command must be an absolute path; arguments and environment entries are supported. HTTP and ACP server transports are rejected. These options are not exposed when creating a session through Ox.

On resume, a nonempty supplied server list replaces the session's previous list and is used by Claude. An empty list retains the previous servers rather than clearing them.

## Related areas

- [Agents and settings](agents.md): compatibility and generic controls in Ox.
- [Conversations](conversations.md): how the desktop presents messages and tools.
- [Permissions and questions](requests.md): shared request presentation and validation.
