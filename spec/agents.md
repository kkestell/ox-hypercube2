# Agents and settings

Users can run compatible coding agents and change the session controls those agents expose. The desktop starts Claude; the command line can start a custom agent command.

## Workflows

### Choose a compatible agent

1. Select Claude when starting a desktop session, or supply an agent command after `--` to `oxctl new`.
2. Ox opens a session in its selected directory. The agent must support ACP draft protocol version 2 and advertise baseline session support; an incompatible agent fails startup with a reason.
3. The agent supplies its available commands, prompt capabilities, configuration controls, and session title. Those can change while the session is open and are reflected in connected clients.

An agent is started with the environment of the client creating or resuming the session. Resuming uses that client's current environment, not a saved copy from session creation. Sessions cannot be switched to another agent command after creation.

### Change session settings

1. The desktop shows the session's reported configuration controls below the composer.
2. A model control opens a searchable picker. Search matches model names without case sensitivity; Up and Down select a result and Enter or a click chooses it. The menu remains open after a choice so effort can be changed next.
3. When a model and thought-effort control are both supplied, the picker also shows effort and offers its ordered levels on a slider. A pending effort selection remains visible until accepted or rejected.
4. Other selection controls appear as dropdowns, retaining any agent-supplied groups. Boolean controls appear as checkboxes. Unsupported control kinds are not shown.
5. Accepted changes update the session's controls for every client. A refusal displays the agent's error rather than claiming success. The command line can inspect and set the same options.

Changing settings is refused while Starting or Exited. Whether a change is allowed during a turn, and its meaning and retention, are determined by the agent. Claude's choices and defaults are specified in [the bridge](claude.md#models-effort-and-modes).

## Rules

### Prompt content

- Prompts must contain at least one content block. Text and resource links do not need optional agent capabilities.
- Image, audio, and embedded-resource content are accepted only when the agent advertises support for that content type. Unsupported content fails before being sent to the agent and does not start a turn.
- An agent may impose additional restrictions, such as supported image formats or rejecting empty text. Those failures appear as prompt or turn errors.
- Ox requests session-scoped question forms from agents; supported forms and answer validation are described in [permissions and questions](requests.md).
- The desktop and command line do not offer MCP server configuration when starting an Ox session. Standalone bridge clients have the [MCP options](claude.md#mcp-servers) described there.

### Turn behavior

- A prompt accepted by Ox makes the session Running immediately. A second prompt is refused until the turn ends and no unanswered request prevents the session becoming Idle.
- The agent reports when work starts or finishes. Receiving text is not itself a promise that the turn is finished.
- A prompt the agent rejects before reporting work ends with that error, allowing a corrected prompt. A repeated old Idle report does not prematurely end a newly submitted prompt.
- An agent can report background work while Idle. Idle means another prompt is allowed, not that all background activity has ended.

## Related areas

- [Conversations](conversations.md): the content shown in the desktop.
- [Sessions](sessions.md): starting, resuming, and resource management.
- [Claude Code bridge](claude.md): the included agent's additional behavior.
