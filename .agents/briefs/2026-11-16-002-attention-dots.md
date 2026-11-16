# Status dots show what needs the user

## Problem

The sidebar's status dots mix two things. A session's dot shows what its agent
is doing, while a run's dot shows the workflow's outcome for good: green for
succeeded, red for failed, hollow for cancelled or interrupted. Child rows mix
both. Nothing shows that work finished while the user was looking elsewhere.
When an agent in a background session delivers its answer, the session turns
gray and looks like every other idle session. A session that hits an error
shows no error at all.

## Outcome

Every dot, for sessions, runs and child rows alike, answers one question:
what does this need from the user right now?

- **Blue:** working.
- **Orange:** blocked on the user's answer to a permission request or a form.
- **Red:** something went wrong that the user hasn't seen.
- **Green:** finished, and the user hasn't seen it.
- **Gray:** nothing to do.

A session whose agent finishes its answer while the user is elsewhere turns
green and turns gray once the user views it. A run that finishes does the
same. Green and red stay through restarts of the app and the daemon, and every
client sees the same state.

## Decisions

- **The dot has one meaning.** Run dots stop showing the outcome. A succeeded
  run the user has seen is gray, like an idle session they have seen. The
  outcome stays in the run view and the dot's tooltip. Turned down: keeping
  the outcome in run dots and adding attention on top, which would leave
  green meaning both "succeeded" and "unseen".
- **Orange and red show on both a child row and its run. Green shows only on
  the run.** A blocked or failed step blocks or stops the whole run, and the
  run row may be collapsed. A finished step only means the run moves on, so
  green on each step would leave dots to click through. A child row is
  otherwise blue while its step runs and gray after.
- **Orange clears by itself** when the user answers.
- **Green and red clear when the user views the session or run.** Many
  errors can be fixed in place, for example by sending the session another
  message, so a red that lasted until deletion would stay over sessions that
  work again. Turned down: keeping red until the session is deleted or the
  run is resumed or deleted.
- **Viewing needs the Ox window in the foreground.** A session counts as
  viewed while it is selected and Ox is in front. A turn that ends in the
  session the user is looking at turns it gray. If the user is in another
  app, it turns green and clears when they return. A run counts as viewed
  while its run view is open and in front, which also clears red on its
  child rows. Opening a child session clears only that child. Turned down:
  counting a selected session as viewed even when Ox is in the background,
  which loses the signal for work that finished while the user was in
  another app.
- **What turns a dot green:** any turn that ends normally while the session
  isn't being viewed. That includes turns the agent starts by itself, and
  turns that stop because the agent refused or reached a length limit, since
  its reply explains why. For runs: a run that succeeds.
- **What turns a dot red:** a turn that ends with an error, such as the agent
  rejecting the prompt or crashing partway through, and an agent that exits
  with an error or never starts. For runs: a run that fails, or one
  interrupted by a daemon restart, since the user didn't choose that stop.
- **What sets neither:** anything the user did themselves. Cancelling a turn
  or a run, parking and archiving all leave the dot gray.
- **The daemon keeps attention.** It survives restarts, and `oxctl` sees the
  same state. The daemon already owns shared session and run state, and
  already tracks which sessions a client is viewing. Turned down: keeping it
  in the app's memory, where a restart would wipe every green and red dot.
- **No hollow dots.** Exited or parked sessions, pending steps, and cancelled
  or interrupted steps and runs show gray. Exited sessions and pending steps
  keep their dimmed rows. Turned down: keeping hollow as a "not running"
  style, which would put agent activity back into the dot.
