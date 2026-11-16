# Permissions and questions

Users can decide whether an agent may act, answer structured questions, and stop a turn. A request remains available across client disconnections until answered or cancelled.

## Workflows

### Answer a permission request

1. The oldest unanswered request appears above the composer. A permission request shows its title and the answer buttons the agent offers.
2. Choose an offered answer. Buttons are disabled while the answer is being sent; a failure shows an error and allows another attempt.
3. The answered request disappears, and the next unanswered request is shown, if any. The agent decides what work follows the answer.

A session with any unanswered permission request is Awaiting permission, even if question forms are also open. Another client may answer the same request first; an answer to a request no longer pending fails. An answer must identify one of that request's offered options.

An agent's withdrawal of a permission request does not remove it from Ox: it remains pending until a client answers it, the turn is cancelled, or the agent or session is stopped. Agent-withdrawn forms, by contrast, are cancelled and removed.

### Answer a question form

1. A form shows the agent's message, field titles, descriptions, and any supplied defaults. A session waiting only for forms is Awaiting input.
2. Fill in text, numbers, integers, checkboxes, single choices, or multiple choices as requested. Choices can have descriptions.
3. Choose Submit to accept the form or Decline to decline it without supplying answers. Submission controls are disabled while sending. Invalid answers leave the form open for correction.
4. Empty optional text and number fields and empty optional multiple choices are omitted. Boolean fields send true or false. Multiple choices are sent in the order offered, not the order clicked.

The desktop uses appropriate controls for email, URL, date, and date-time fields. Date-time defaults are shown in local time and submitted as the corresponding instant. Unknown optional field types have no desktop control. Forms with unknown required field types or required fields without a description of their type are refused rather than presented as answerable.

### Stop work

1. Choose Stop while a session is Running, Awaiting permission, or Awaiting input, or cancel it from the command line.
2. Ox asks the agent to interrupt the current turn and cancels pending permission requests. The turn ends when the agent reports it stopped.
3. A form withdrawn by the agent disappears and ends as cancelled. Until the agent withdraws it or a client answers it, a form remains pending.

Stopping also applies to a turn the agent started without a user prompt. It is not deletion or archiving. For Claude-specific interruption and background-work behavior, see [the bridge](claude.md#work-and-interruption).

## Rules

- The desktop presents one request at a time in arrival order, regardless of whether it is a permission request or form. Session state prioritizes permission requests over forms, independently of presentation order.
- Accepted form answers must use declared field names, satisfy required fields, have the right value types, use offered choices, and meet declared text-length, number-range, and choice-count limits. Rejected answers do not resolve the request.
- The desktop additionally checks text patterns and supported formats before submission. A pattern matches anywhere in the value unless the pattern itself requires a full match. An invalid pattern is not enforced. Command-line form answers do not enforce text patterns or formats, but must satisfy the shared answer checks above.
- Declining or cancelling a form does not require filling required fields. The command line can explicitly cancel a form as well as decline it.
- Only session-scoped forms are supported, not URL-based or request-scoped questions. A form that gives a text field both plain and titled alternatives is refused.
- Agent exit, parking, session closure, and recovery after a service interruption cancel unresolved requests. A cancelled or already-answered request cannot be answered again.
- Request identities remain distinct through resuming and service restarts, so an old answer cannot resolve a new request.

## Related areas

- [Agents and settings](agents.md): agents determine permission options and question contents.
- [Claude Code bridge](claude.md#claude-questions-and-permissions): Claude's tool decisions and question forms.
- [Command-line access](cli.md): request identities, permission answers, and JSON form answers.
