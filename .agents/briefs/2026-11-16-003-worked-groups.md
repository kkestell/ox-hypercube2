# Fold a finished turn's work under "Worked"

## Problem

A session's chat shows every thought and tool call of every turn, expanded in
line. In a long session the agent's answers are buried among the work it did
to reach them, and the user has to scroll through tool calls to find what the
agent said. Hypercube solves this by folding a finished turn's work under a
"Worked for 10m 23s" toggle, so the chat reads as prompts and answers.

## Outcome

While a turn runs, its thoughts, tool calls and messages show as they arrive,
as they do now. When the turn finishes with a reply, everything the agent did
before its final reply folds into one closed group labelled "Worked" with a
chevron. The user's prompt stays above the group and the final reply stays
below it. Opening the group shows the turn's work as it looked before it
folded. Saved conversations show their finished turns folded the same way.

## Decisions

- **The label reads "Worked", with no duration.** Ox saves no times with its
  conversation, so a duration would need the daemon to start recording them.
  That is a separate piece of work, recorded in the to-do list along with
  times for the run view's timeline. Turned down: showing "Worked for
  10m 23s" now, which pulls daemon and saved-data changes into this change.
- **Only the final reply stays outside the group.** Messages the agent writes
  partway through a turn, such as "Now I'll check the tests", fold away with
  the thoughts and tool calls. Turned down: keeping every agent message
  visible, which leaves several small groups per turn and keeps the clutter.
- **Turns that don't finish cleanly stay expanded.** A turn that ends in an
  error, is cancelled, ends when the agent exits, or ends without a reply
  shows everything in place, so the user sees what happened where it went
  wrong. Turned down: folding those too with only the error notice visible.
- **It follows Hypercube's behavior.** The user asked for the same grouping
  Hypercube has; where this brief is silent, Hypercube is the reference.

## Out of scope

- Showing how long a turn worked, and recording event times.
- The run view, which shows only the agents' messages.

## Left to the plan

- How the group looks within Ox's chat styles, following Hypercube's label,
  chevron and rule.
- Whether an opened group stays open when the user switches sessions or
  restarts the app.
