# Where sessions run

A session runs either in its workspace's own checkout or in a new Git worktree
of the workspace's repository.

## Choosing a checkout

In the workspace's **+** menu, **Session in Checkout** and **Session in
Worktree** open a draft. A Session in Checkout runs in the workspace's
directory. When its draft's first message is sent, a Session in Worktree
creates a worktree on a new branch `ox/<first 8 digits of the worktree's ID>`,
starting at the workspace checkout's `HEAD`. Session in Worktree is offered
for workspaces in a Git working tree and requires a checked-out branch with a
commit when the message is sent.

## Worktrees

Each worktree is the directory `worktrees/<checkout name>-<first 8 digits of
the worktree's ID>` beside the daemon socket. By default that is in Ox's data
directory. When the workspace is a subdirectory of its checkout, the session
runs in the same subdirectory of the worktree.

A Session in Worktree branch has no upstream. A workspace subdirectory must exist at the
starting commit. The worktree contains committed files only; uncommitted
changes stay in the workspace's checkout.

## Comparing a session's branch

Session in Worktree compares the session's branch with the branch the workspace's
checkout was on when the session started. When the session has commits ahead,
the Git branch icon's tooltip shows the counts. A failed comparison shows
its error in the tooltip. The session view's subheader shows the worktree's
branch and its comparison, and **Merge** at its end. A session in the
workspace's checkout shows the checkout's directory instead.

## Moving a branch

**Merge** fast-forwards the comparison branch to the session's
commit when the session is one commit ahead and none behind. Both branches
must still be at the commits Ox used for the comparison.

When the branch is checked out, its files and index move with it. Ox checks
for uncommitted tracked changes, untracked files the move would overwrite
(including ignored files), and agents starting, working, or waiting for an
answer. While the move runs, Ox refuses requests to start, load, or prompt a
session in that checkout.

Submodule checkouts stay at their current commits when the branch moves.

## Deleting and archiving

| Action | Worktree | Branch |
|---|---|---|
| Delete | Removed | Deleted when it passes the branch cleanup rule below |
| Archive | Removed | Kept |
| Unarchive | Added again, on the session's branch | — |

The sidebar hides archived sessions until **Show Archived** is checked in the
workspace name's context menu.

Ox creates the branch for every worktree session. Delete removes the branch
when no worktree has it checked out and every commit on it outside the
ancestry of the session's starting commit is reachable from another branch, a
remote branch, or a tag.
If those conditions fail or the repository is missing, Ox keeps the branch
and reports its name.

Delete and archive refuse to remove a worktree that has
uncommitted changes, commits on no branch, remote branch or tag, or submodule
commits on no remote branch. Ox asks whether to discard the unsaved work
before removing the worktree.

A [workflow run](workflows.md#cancelling-removing-and-restarting) owns its
worktree.
