# Git tasks

Users can isolate a session's work in a separate checkout, compare its branch with a target, and fast-forward a completed task into a local branch without losing unrelated work.

## Workflows

### Start a task

1. In a workspace within a Git working tree, choose Session in Worktree.
2. Ox starts a worktree session on a new `ox/<first eight characters of the session ID>` branch from the workspace's current `HEAD`, compared with the branch checked out there.
3. If the workspace is not on a branch, Ox reports <workspace> is not on a branch and creates no task. Outside Git, Session in Worktree is not offered; Session in Checkout remains available.

Uncommitted changes in the workspace are not copied to a new worktree. If the workspace is a subdirectory of a repository, the session runs in the corresponding subdirectory of the new worktree.

### Choose another worktree branch

The command line offers these alternatives:

| Choice | Starting point | Upstream |
| --- | --- | --- |
| Automatic session branch | Workspace `HEAD` | None |
| Named new branch | Workspace `HEAD`, or the supplied starting commit | None |
| Existing local branch | That branch's tip | Unchanged |
| Remote branch without an existing corresponding local branch | Remote branch's tip, creating a local tracking branch | Selected remote branch |

Creating a worktree is refused if the workspace is outside a usable Git working tree, the repository has no commits, the requested branch name or starting commit is invalid, or the corresponding workspace subdirectory does not exist at that commit.

- An existing local branch must not already be checked out anywhere, including in the workspace checkout.
- An existing branch must exist locally or on a remote. An explicit remote/branch selection is refused if its local branch already exists.
- When an unqualified name exists on several remotes, `checkout.defaultRemote` must select one of them; otherwise the name is ambiguous and creation fails.
- A named new branch must not collide with a local branch, a full remote branch name, or an unqualified branch name on any remote.
- A worktree directory already occupying the intended location prevents creation. A failed setup does not silently reuse it or an existing branch.

### Compare a session branch

1. Supply a comparison ref when creating a worktree session, or set one later from the command line. The ref must resolve to a commit. Sessions outside an Ox-managed worktree cannot set one.
2. Ox reports the session branch's commit, the comparison ref's commit, and how many commits the branch is ahead and behind.
3. Comparisons refresh after creation or service restart, at the end of a turn, after a branch move or session deletion in that repository, and after a fetch whether it succeeds or fails. They can also be refreshed explicitly.
4. A missing or invalidated branch or ref reports a comparison failure with its reason. Clearing the comparison ref removes the comparison.

In the desktop, a non-archived session with a comparison shows a branch indicator when it is ahead: ordinary at one commit ahead and none behind, attention-colored for multiple commits or divergence. Failed comparisons also show an attention indicator. Hovering shows the counts or failure. With no commits ahead, there is no branch indicator.

### Merge a completed task locally

1. View a non-archived session with a comparison. The session's subheader shows its branch and comparison, and offers Merge into <ref> as a Merge button once a comparison is available.
2. The button is enabled only at exactly one commit ahead and none behind. Otherwise its tooltip explains No commits, asks for a rebase when behind, asks for a squash when multiple commits are ahead, or shows the comparison failure.
3. Merge fast-forwards the target local branch from the commit used in the comparison to the session branch's compared commit. It does not create a merge commit, squash commits, rebase, or push.
4. On success, choose Keep Session or Delete Session. Keeping leaves the session and worktree available; deletion follows [session deletion](sessions.md#delete-a-session).

### Move a branch explicitly

The command line can move a local branch to a supplied commit, requiring the branch to be at the supplied expected commit. A stale expectation fails without making the requested move. A non-fast-forward move requires `--force`.

If a checkout has the target branch, its tracked files and index move with it. The move is refused while tracked files have uncommitted changes, if it would overwrite an untracked file, or if an Ox session in that checkout is Starting, Running, Awaiting permission, or Awaiting input. Idle sessions and unrelated untracked files do not prevent a move. Submodule checkouts are not updated. A refusal leaves existing work intact.

### Inspect branches through a connected client

Branch listing distinguishes local and remote branches, orders each list by most recent commit, identifies local branches checked out elsewhere, and identifies the workspace's current branch when it has one. Symbolic aliases such as `origin/HEAD` are excluded.

A client may request a fetch before listing. Fetch covers all remotes and follows the user's pruning settings. A failed fetch, including one stopped after 60 seconds, is reported alongside the branches as they stand. Creating a session does not fetch or wait for a fetch.

## Rules

### Protect work during cleanup

Deleting or archiving a worktree session first checks for:

- Uncommitted tracked changes and untracked files; ignored files do not count as unsaved work.
- Commits in the worktree that no branch, remote branch, or tag retains.
- Initialized submodule commits that no remote branch retains.

If any would be lost, Ox refuses and lists the reason unless the user explicitly discards the work. Removal deletes the whole managed worktree, including ignored files. If the repository is missing or no longer recognizes the worktree and safety cannot be checked, discarding is required. A worktree directory already removed outside Ox can still be cleaned up from Ox.

Discarding authorizes removal of that worktree's unsaved work; it does not authorize deleting arbitrary workspace files or bypass the branch-retention rule below. A failure during cleanup is reported rather than hidden. Cleanup does not require the original workspace directory still to exist.

### Retain or delete the branch

- Archive always retains the session's branch.
- Delete never deletes a branch that existed locally before the session.
- A branch created by Ox is deleted only when its commits since the session's starting point are retained by another branch, remote branch, or tag and no checkout still has it checked out. Otherwise it is retained and named to the user. If its safety cannot be checked, it is retained.
- Unarchive restores the worktree on its retained branch; it does not create a fresh replacement branch.

## Related areas

- [Sessions](sessions.md): session lifecycle and discard dialogs.
- [Command-line access](cli.md): branch-selection, comparison, and branch-move syntax.
