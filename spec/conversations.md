# Conversations

Users send prompts to a session, follow the agent's streaming responses, and inspect its reported thinking and tool activity.

## Workflows

### Compose and send

1. Enter a message. Enter sends it; Shift+Enter inserts a newline. Text composition is not submitted while an input method is composing characters.
2. Choose Attach Files to select one or more local files. Selected names appear above the message; hover for the full path, or remove an attachment before sending. Selecting the same path again does not duplicate it in the message.
3. Send is enabled only while the session is Idle and the message has non-whitespace text or at least one attachment. Leading and trailing whitespace is removed from submitted message text.
4. A successful submission clears the message and attachment list. A failed submission shows its error and leaves them available to correct or retry.
5. The prompt appears in the conversation with attachment labels, and the session becomes Running. Sending accepts the prompt for processing; it does not wait for the agent's completed reply.

The message editor and attachment button are disabled for an Exited session. During a turn the user can compose a message but cannot send another prompt. The send button becomes Stop; see [stopping work](requests.md#stop-work).

Each session keeps its unsent message and attachments while other sessions are viewed and across restarts of the app.

Desktop attachments, including images and PDFs, give the agent references to local files it can open; their contents are not embedded in the prompt. The referenced paths must still exist when sending, or submission fails. The conversation shows attachment labels rather than previews of the attached files.

### Complete a slash command

1. At the start of a message, optionally after whitespace, type `/` while the caret is in the command name. Matching commands appear with their descriptions and any text argument hints.
2. Matching is a case-insensitive name prefix search over the selected session's current command catalog.
3. Use Up and Down to choose a suggestion, then Enter or Tab to insert it; clicking a suggestion also inserts it. Completion preserves existing arguments and leaves the editor focused. It does not submit the prompt.
4. Add arguments and submit normally. Escape dismisses suggestions without changing the message; editing the message allows suggestions again.

No completion appears for ordinary prose, an unmatched command, or a caret in the command's arguments. Unknown command text can still be submitted. Command catalogs belong to individual sessions and can be replaced or cleared by the agent.

### Follow the conversation

- User messages, agent replies, and nonempty thinking are shown in conversation order. Replies and thinking update as text arrives; a whole-message revision replaces the previous content for that message.
- Thinking starts collapsed under Thinking and can be expanded.
- Prompts, replies, and thinking render Markdown, including headings, lists, quotes, fenced code, tables, task lists, and strikethrough. Code keeps its whitespace; wide code and tables scroll within their messages rather than widening the conversation.
- Raw HTML is not interpreted as active content. Only HTTP, HTTPS, and email links are actionable; these open outside the conversation through the system's opener. Local paths, relative links, and unsafe or unsupported link destinations remain inert.
- The view follows new output while at the bottom. Scrolling up stops automatic following; returning to the bottom restores it. Opening another session starts at the end of that conversation.

### Inspect a tool call

1. A reported tool call appears with its title and Running, Completed, or Failed status. If no title is supplied, its name is used, otherwise Tool call.
2. Expand a tool call that has arguments or output to see Arguments and Output. Arguments are displayed as formatted JSON; textual output is literal text, not rendered Markdown. Image output is displayed inline.
3. Later reports update the same tool call. Incremental output appends, while a replacement output replaces its previous contents.

Tool-call titles support inline code. Separate terminal views and diff output are not displayed. Plans, usage, notices, and compaction updates have no dedicated desktop presentation, although the session's event stream retains them.

## Rules

- Reopening or reconnecting to a conversation shows its retained history without repeating messages already present.
- An agent failure is shown in the conversation with the reported reason. Token limits, per-turn request limits, and refusals are identified separately. An ordinary completed or cancelled turn adds no error message.
- An agent exit shows The agent exited and any reported error. Parking does not add an exit warning.
- Questions and permission requests appear above the composer while unanswered, not as question panels within the conversation; see [permissions and questions](requests.md).
- Available content depends on the [agent's capabilities](agents.md#prompt-content).

## Related areas

- [Sessions](sessions.md): retained history and session availability.
- [Command-line access](cli.md): prompting, image uploads, and following events without the app.
- [Claude Code bridge](claude.md): Claude-specific replay and subagent activity.
