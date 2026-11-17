# Queued and steered messages

## Problem

While an agent works, Send is disabled. A user who thinks of the next request
or a correction has to wait for the turn to end, or stop the turn. Claude
Code and Pi can both take a message in the middle of a turn, and nothing in Ox
lets the user line up messages to send after it.

## Outcome

Both apply to Claude and Pi sessions.

- While the agent works, Enter queues the composer's message and Cmd+Enter
  steers it: the message goes to the agent now and is taken in at the agent's
  next opportunity within the current turn.
- Queued messages stack directly above the composer. Each row shows its text,
  with Edit, Send now, and Delete on hover. Edit loads the message back into
  the composer, and saving puts it back in its place. Send now steers it.
- When a turn ends normally, the first queued message starts the next turn.
  Messages go one per turn, in the order they were queued.
- After Stop, or a turn that ends with an error, the queue stays visible but
  nothing is sent automatically. Sending resumes once the user sends a message
  or uses Send now on a queued item.
- A steered message appears in the conversation at once, dimmed until the
  agent takes it in, then shown where it landed. It cannot be edited or
  deleted. If the turn ends before the agent takes it, it starts the next turn
  ahead of the queue.
- Queued messages belong to the session and are kept by the daemon. They are
  sent when a turn ends even while the app is closed, and survive restarts.

## Decisions

- **Enter queues, Cmd+Enter steers.** Cursor's pattern. The default never
  interrupts the agent's plan, and steering is a deliberate act. Turned down:
  Enter steers and Tab queues (Codex CLI's pattern), and steering only from
  the queue list.
- **The daemon keeps the queue.** The queue must keep sending when the turn
  ends whether or not the app is open, and must survive restarts. Turned down:
  keeping it in the desktop app, which sends nothing while the app is closed.
- **One message per turn, in order.** Each message gets the agent's full
  attention and its own reply. Turned down: joining everything queued into one
  prompt.
- **Stop and errors hold the queue.** Stop usually means "change course", and
  sending into a failing agent usually fails again. Turned down: continuing to
  send, and discarding the queue on Stop.
- **The queue list follows Cursor and Claude Code's desktop app**, above the
  composer, without reordering. Turned down: drag to reorder.
- **Steered messages cannot be edited or deleted once sent.** Claude Code
  cannot take a message back once it has it. Turned down: holding steered
  messages in an editable list until the agent takes them in, which Claude's
  interface does not support cleanly.

## Left to the plan

- Whether and how `oxctl` exposes queueing and steering; it serves agents and
  testing only.
