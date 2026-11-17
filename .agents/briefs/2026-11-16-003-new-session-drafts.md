# New sessions start on first send

## Problem

Clicking New Session saves a session and starts its agent right away. A
worktree session also gets a branch and a worktree. A user who opens New
Session and never sends anything is left with an empty session in the
sidebar, a running agent that is parked later, and, for a worktree session,
a branch and a checkout on disk. Nothing needs to exist before the first
message.

## Outcome

- New Session opens a draft: a composer for the chosen workspace, agent, and
  checkout. Nothing is saved, started, or created in git until the first
  message is sent.
- Sending the first message shows it in the conversation immediately. The
  session is saved then, and its worktree and agent start in the background.
- Each draft is a row at the top of its workspace in the sidebar, labeled
  with the start of its typed text. A draft keeps its text while the user
  looks at other sessions and comes back to it.
- A draft left empty disappears when the user moves away from it.
- After sending, the row keeps showing the prompt text until the agent gives
  the session a title.
- Drafts, and unsent text in every session's composer, survive quitting and
  reopening Ox.

## Decisions

- **Create nothing until the first send.** Empty sessions piling up and
  parking serve no purpose. Turned down: starting the agent but saving the
  session only on the first prompt, and deleting empty sessions
  automatically. Both still create something for a session that may never be
  used.
- **Start the agent after the first message is shown, not ahead of time.**
  Showing the message and starting the agent in the background hides startup
  better than starting agents speculatively.
- **The composer's model, mode, and slash-command controls work in a draft
  without a running session.** Their contents can be loaded ahead of time or
  remembered from an earlier session; they do not justify starting an agent.
- **Drafts appear in the sidebar and keep their text.** Losing a carefully
  typed prompt because the user glanced at another session is unacceptable.
  Turned down: no row, with the draft discarded on leaving; and no row, with
  New Session restoring the text, which hides that a draft exists.
- **Each New Session click makes its own draft**, with its own agent and
  checkout. Turned down: one draft per workspace, which forces finishing or
  clearing one draft before starting another.
- **Draft rows are labeled with their typed text**, not "New session", so
  several drafts are distinguishable.
- **Unsent text is kept in the desktop app's local storage.** It survives
  restarts and is not shared with other clients of the daemon. Turned down:
  keeping it only while the window is open.

## Left to the plan

- Where a draft's model, mode, and slash-command lists come from, such as
  loading them when Ox starts or remembering them from the workspace's last
  session.
- What to do with empty sessions already saved in existing databases.
