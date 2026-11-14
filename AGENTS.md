# Agent instructions

## Project scope

- Ox is a hobby project with one user. The desktop app is the normal
  interface. Agents use `oxctl` to test the daemon and handle occasional tasks
  on the user's behalf. Custom daemon sockets, including `--socket` and `OX_SOCKET`,
  are development and test tools. A CLI option alone does not establish a
  user workflow or a compatibility promise.
- Prioritize failures the user can encounter in the desktop and in the workflows
  the user asks agents to run. Rank findings by likelihood in those workflows and
  the cost of failure. Protect work and preserve required guarantees with
  the simplest mechanism that covers the demonstrated problem.
- Defer unusual inputs, unlikely combinations of failures, and recovery
  machinery until there is evidence of a practical need. A short, local
  TODO may record a credible concern and the condition that would make it
  worth revisiting.

## Writing code

- Wait on events. A process announces readiness, state changes publish to a
  channel or watch, and code that needs to know a process exited waits on it
  or on a pipe it holds. Timers serve only as timeouts on a wait, or where the
  passage of time is itself the behavior, such as an idle period.
  - `oxd` prints `ready` once it listens; `connect_or_start` reads that line.
  - Idle exit watches a `watch::Sender<()>` signalled whenever clients,
    sessions, or a session's state change, then starts its idle timer.
- Change an interface by updating every caller in the repository.
- Versions stay at 1 until the project is in production. The client–daemon
  `hello` handshake checks the protocol version, but no change bumps it, and
  there are no backwards-compatibility paths.
  - `session.prompt` changed from `text` to content blocks, and every caller
    changed with it.
- Fix causes. A fix that needs a grace period, a retry loop, or a change to
  process-wide state points at a structural cause; find it and fix that.
  - After `claude` exits, everything it wrote is already in the stderr pipe,
    so the bridge reads what is buffered instead of waiting a grace period.
  - The socket's mode is set with `chmod` after `bind`, and the peer-uid
    check is what keeps other users out, so the process umask stays as is.
- Use what the language and libraries provide: derived serialization, `match`
  on enums, the library's ordering and lifecycle guarantees. Read the
  library's documentation for the guarantee before relying on it or working
  around its absence.
  - `ServerMessage` derives `Deserialize`; `oxctl` names a `StopReason` with
    a `match`.
  - The ACP library dispatches a notification before the response that
    follows it, so the response handler knows whether a notification already
    reported a config change.
- Take a plan's goals as the requirement and its mechanisms as suggestions.
  When a mechanism in the plan is weak, use a better one and say so.
  - The plan said `connect_or_start` "retries until it accepts"; the goal is
    connecting to a running daemon, which the `ready` line provides.
- A capability the code advertises is implemented in full.
  - An agent that advertises the baseline session methods replays the
    conversation on `session/resume` with `replayFrom: {"type": "start"}`.
  - The bridge advertises the `image` prompt capability and sends images to
    Claude.
- The UI is idiomatic React (`ui/src/`) with icons from `lucide-react`.
  The user designs the app's look: UI changes add only the CSS a layout needs
  to work, such as a scrolling region or `white-space: pre-wrap`, and styling
  only where the user asks for it or where it makes an element match its peers.
- Elements of the same kind look the same everywhere. A new element takes its
  class and tokens from the existing elements of its kind, and the CSS states
  each shared value once, in one rule for the kind. Elements in neighbouring
  regions that nearly line up share an edge or a centre line exactly.
- UI changes go through the shared abstractions: a class per kind of element,
  tokens for sizes and colours, and shared layout components for regions that
  line up. A requested change that would make one element differ from its
  peers, or that adjusts a value by a few pixels to make things line up,
  points at a missing or wrong abstraction. Say so, propose the abstraction,
  and build it before making the change, so the change lands in the shared
  rule rather than as a one-off. Hold that position through follow-up
  requests; a value that has to be nudged into place is a structural problem
  to solve first.
  - Every icon button has the `icon-button` class, which sets its size, its
    icon's size, and its colour; a panel toggle that is off is muted.
  - Icon buttons in a group sit flush, like the panel toggles.
  - The sidebar and content headers share the `pane-header` class, so their
    buttons share a centre line; only the content header has a bottom border.
  - Workspace and session actions sit `--row-action-inset` from the row's
    end, and the sidebar header uses the same inset, so all three line up.

## Testing

Read [.agents/testing.md](.agents/testing.md) for test design, fixtures, and
required verification commands before writing tests or running checks.

## Reporting

- Name every shortcut, workaround, and unimplemented part in the report, in
  plain terms.
  - "`session/resume` ignores `replayFrom` although the bridge advertises the
    session methods."
  - "`prompt_text` exists only so tests can send one string."
