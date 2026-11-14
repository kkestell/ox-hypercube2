# ACP v2 support

This chart lists the draft [Agent Client Protocol](https://agentclientprotocol.com/)
v2 and what Ox supports of it. It is built from `agent-client-protocol-schema`
1.17.0, the version `agent-client-protocol` 5.2.0 pins, with the
`unstable_protocol_v2` feature. Rows marked *unstable* sit behind an
`unstable_*` feature of the schema and may change in any release. Bumping
`agent-client-protocol` updates these rows in the same change.

The columns are the feature, `oxd` as the client, `claude-code-acp` and
`pi-acp` as agents, and caveats. Each cell is ✅ supported, ⚠️ partial, or ❌ unsupported;
a partial cell says in the caveats what works and what does not.
`oxd-fake-agent` is a test fixture and has no column.

`oxd` records and relays live session updates unchanged and in order. During
resume it keeps the existing conversation when it has one and discards the
agent's replay. ✅ in the `oxd` column means `oxd` records and
relays the update or acts on it as the protocol requires.

## Methods the client sends

| Feature | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `initialize` | ✅ | ✅ | ✅ | Ox requires protocol v2 and the `session` capability. |
| `auth/login` | ❌ | ❌ | ❌ | |
| `auth/logout` | ❌ | ❌ | ❌ | |
| `auth/status` (unstable) | ❌ | ❌ | ❌ | |
| `providers/list` (unstable) | ❌ | ❌ | ❌ | |
| `providers/set` (unstable) | ❌ | ❌ | ❌ | |
| `providers/disable` (unstable) | ❌ | ❌ | ❌ | |
| `session/new` | ✅ | ✅ | ✅ | Both bridges start their harness asynchronously; a successful response does not guarantee the harness has started. See `session.mcp.stdio` for MCP support. |
| `session/list` | ❌ | ✅ | ❌ | Claude includes native transcripts. Ox uses its own session list. |
| `session/resume` | ✅ | ✅ | ✅ | Both bridges support only `replayFrom: start`. Pi also rejects a changed `cwd` or MCP servers. See [Resume and replay](#resume-and-replay). |
| `session/close` | ❌ | ✅ | ✅ | Both bridges leave the session resumable. Ox deletes sessions by stopping the agent process. |
| `session/prompt` | ✅ | ✅ | ✅ | During a turn, Claude waits to accept the next prompt; Pi queues it as a follow-up. |
| `session/set_config_option` | ✅ | ✅ | ✅ | Claude: `model`, `effort`, `mode`. Pi: `model` (`provider/id`) and `thinking` when available. Ox supports select and boolean options. |
| `session/delete` | ❌ | ❌ | ❌ | |
| `session/fork` (unstable) | ❌ | ❌ | ❌ | |
| `session/archive` (unstable) | ❌ | ❌ | ❌ | |
| `session/unarchive` (unstable) | ❌ | ❌ | ❌ | |
| `session/status` (unstable) | ❌ | ❌ | ❌ | |
| `session/options` (unstable) | ❌ | ❌ | ❌ | |
| `session/set_title` (unstable) | ❌ | ❌ | ❌ | Both bridges derive titles from the first prompt. |
| `tool_call/cancel` | ❌ | ❌ | ❌ | |
| `terminal/cancel` | ❌ | ❌ | ❌ | |
| `mcp/message` (unstable) | ❌ | ❌ | ❌ | |
| Extension requests (`_`-prefixed) | ❌ | ❌ | ❌ | |

## Notifications the client sends

| Feature | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `session/cancel` | ✅ | ✅ | ✅ | Claude also drops prompts it has not accepted. Ox closes permission requests, but elicitations remain open until the agent withdraws them. |
| `$/cancel_request` | ❌ | ❌ | ❌ | |
| `mcp/message` (unstable) | ❌ | ❌ | ❌ | |
| Extension notifications (`_`-prefixed) | ❌ | ❌ | ❌ | |

## Methods the agent sends

| Feature | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `session/request_permission` | ✅ | ✅ | ❌ | Pi runs tools without asking. Claude's AskUserQuestion uses `elicitation/create` instead. |
| `elicitation/create` | ✅ | ✅ | ✅ | Claude: AskUserQuestion forms. Pi: extension dialogs. Ox supports session-scoped forms only; URL mode and request scope are unsupported. Schema `pattern` and `format` are not validated; unknown required field types and fields combining `enum` with `oneOf` are rejected. |
| `mcp/message` (unstable) | ❌ | ❌ | ❌ | |
| Extension requests (`_`-prefixed) | ❌ | ❌ | ❌ | |

## Notifications the agent sends

| Feature | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `session/update` | ✅ | ✅ | ✅ | |
| `elicitation/complete` | ❌ | ❌ | ❌ | |
| `auth/status_update` (unstable) | ❌ | ❌ | ❌ | |
| `$/cancel_request` | ⚠️ | ✅ | ✅ | Ox closes withdrawn elicitations. Withdrawn permission requests remain pending until answered or the turn is cancelled. |
| Extension notifications (`_`-prefixed) | ❌ | ❌ | ❌ | |

## Session updates

| `sessionUpdate` | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `user_message` | ✅ | ✅ | ✅ | |
| `user_message_chunk` | ✅ | ❌ | ❌ | |
| `agent_message` | ✅ | ✅ | ✅ | Both bridges use whole messages for replay and text that was not streamed. |
| `agent_message_chunk` | ✅ | ✅ | ✅ | |
| `agent_thought` | ✅ | ✅ | ✅ | Both bridges use whole messages for replay and thinking that was not streamed. |
| `agent_thought_chunk` | ✅ | ✅ | ✅ | |
| `state_update` | ✅ | ✅ | ✅ | |
| `tool_call_update` | ✅ | ✅ | ✅ | Claude includes subagent activity as text. Pi includes diffs for `edit` and `write`. |
| `tool_call_content_chunk` | ✅ | ❌ | ❌ | |
| `terminal_update` | ✅ | ❌ | ❌ | |
| `terminal_output_chunk` | ✅ | ❌ | ❌ | |
| `plan_update` | ✅ | ❌ | ❌ | |
| `plan_removed` (unstable) | ❌ | ❌ | ❌ | |
| `available_commands_update` | ✅ | ✅ | ✅ | Claude: native commands, available after harness initialization. Pi: extension commands, prompt templates, and skills (`skill:name`). |
| `config_option_update` | ✅ | ✅ | ✅ | |
| `session_info_update` | ✅ | ✅ | ✅ | Both bridges supply titles. Only Claude supplies `updatedAt`; Ox only records it. |
| `usage_update` | ✅ | ❌ | ❌ | |
| `notice` | ✅ | ❌ | ❌ | |
| `compaction_update` | ✅ | ❌ | ❌ | |
| `compaction_summary_chunk` | ✅ | ❌ | ❌ | |
| `subagent_update` (unstable) | ❌ | ❌ | ❌ | Claude reports subagent activity within tool calls instead. |
| `session_message` (unstable) | ❌ | ❌ | ❌ | |
| `session_message_chunk` (unstable) | ❌ | ❌ | ❌ | |
| Extension updates (`_`-prefixed) | ✅ | ❌ | ❌ | |

The app displays messages, thoughts, tool text and images, and command
completion. It does not display tool diffs, terminals, or other update types;
`oxd` still records them.

## State updates

| `state` | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `running` | ✅ | ✅ | ✅ | |
| `idle` | ✅ | ✅ | ✅ | |
| `requires_action` | ✅ | ✅ | ✅ | Claude: permission requests. Pi: extension dialogs during a run. |
| `unknown` (unstable) | ❌ | ❌ | ❌ | |
| `background` work lists | ✅ | ✅ | ✅ | Claude reports live tasks and inferred schedules, excluding durable jobs from other sessions. Recurring schedules expire from the list after seven days. Pi always reports an empty list. See [parking rules](architecture.md#loading-and-stopping-agents). |
| Other states | ✅ | ❌ | ❌ | Ox treats unrecognized states as active work. |

## Stop reasons

| `stopReason` | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `end_turn` | ✅ | ✅ | ✅ | |
| `max_tokens` | ✅ | ❌ | ✅ | |
| `max_turn_requests` | ✅ | ✅ | ❌ | |
| `refusal` | ✅ | ❌ | ❌ | |
| `cancelled` | ✅ | ✅ | ✅ | |
| `error` | ✅ | ✅ | ✅ | |
| Other stop reasons | ✅ | ❌ | ❌ | |

## Content blocks in prompts

| Block | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `text` | ✅ | ✅ | ✅ | |
| `resource_link` | ✅ | ✅ | ✅ | Passed to the harness as a Markdown link. |
| `image` | ✅ | ✅ | ✅ | Claude accepts PNG, JPEG, GIF, and WebP. |
| `resource` | ✅ | ✅ | ❌ | Claude accepts embedded text, PDFs, and images; other blobs are rejected. |
| `audio` | ✅ | ❌ | ❌ | |
| Other blocks | ❌ | ❌ | ❌ | |

## Agent capabilities

| Capability | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `session` | ✅ | ✅ | ✅ | |
| `session.prompt.image` | ✅ | ✅ | ✅ | |
| `session.prompt.embeddedContext` | ✅ | ✅ | ❌ | |
| `session.prompt.audio` | ✅ | ❌ | ❌ | |
| `session.mcp.stdio` | ✅ | ✅ | ❌ | Ox supplies the [`ox` MCP server](architecture.md#ox-mcp) to Claude, which allows its tools without permission prompts. Pi sessions have no `ox` MCP server. |
| `session.mcp.http` | ❌ | ❌ | ❌ | |
| `session.mcp.acp` (unstable) | ❌ | ❌ | ❌ | |
| `session.delete` | ❌ | ❌ | ❌ | |
| `session.additionalDirectories` | ❌ | ❌ | ❌ | |
| `session.systemPrompt` | ❌ | ❌ | ❌ | |
| `session.fork` (unstable) | ❌ | ❌ | ❌ | |
| `session.archive` (unstable) | ❌ | ❌ | ❌ | |
| `session.status` (unstable) | ❌ | ❌ | ❌ | |
| `session.options` (unstable) | ❌ | ❌ | ❌ | |
| `session.title` (unstable) | ❌ | ❌ | ❌ | |
| `auth` | ❌ | ❌ | ❌ | |
| `providers` (unstable) | ❌ | ❌ | ❌ | |

## Client capabilities

| Capability | oxd | claude-code-acp | pi-acp | Caveats |
| --- | --- | --- | --- | --- |
| `auth.terminal` | ❌ | ❌ | ❌ | |
| `elicitation.form` | ✅ | ✅ | ✅ | Without this capability, Claude asks questions in plain text; Pi cancels extension dialogs. |
| `elicitation.url` | ❌ | ❌ | ❌ | |

## Resume and replay

`oxd` requests `replayFrom: start` on `session/resume` only when its saved
history lacks an `agent_started` event. The agent sends any requested replay,
then reports its state before answering. The daemon records the replay once
the agent starts, so a load that fails partway saves no replay.

An agent that reports it is working leaves the session `running` until its
`idle`, with no `turn_started` or `turn_ended` for work the history did not
see start.
