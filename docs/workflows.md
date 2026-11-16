# Workflows

A workflow is a named sequence of agent steps and shell steps. A run of a
workflow gets its own worktree, and every step runs in it: an agent step in a
new child session, a shell step as a command. The output of each step, an
agent's final reply or a command's standard output, becomes the input of the
next. Any step can have checks: shell commands that must pass before and
after it.

## Defining a workflow

Each workflow is a YAML file in `$XDG_CONFIG_HOME/ox/workflows/`, by default
`~/.config/ox/workflows/`. Every workspace and every daemon of the user shares
these definitions. A run is started by the file name without `.yaml`, such as
`plan-review` for `plan-review.yaml`. The file's `name` is the workflow's
display name. The desktop lists every definition, each time a New Task menu
opens. A definition that does not parse is listed disabled, with its error as
its tooltip.

```yaml
name: Plan and review

steps:
  - name: Plan
    agent: &claude
      harness: claude
      model: claude-opus-5-5
      effort: high
      permission_mode: manual
    prompt: |
      Write an implementation plan for the task in your input.

  - name: Review
    agent: *claude
    prompt: |
      Review the plan in your input and list its weaknesses.
```

YAML anchors and aliases let steps share an agent. The daemon reads a
definition when a run starts, and refuses a definition that does not parse,
has no steps, or has a field this page does not list.

Each step has a `name`, and either `agent` and `prompt` for an agent step, or
`shell` for a shell step. Either kind can have `preconditions` and
`postconditions`.

## Agent steps

An agent step has a `prompt` and an `agent`:

| Field | Value |
| --- | --- |
| `harness` | `claude` or `pi` |
| `model` | The harness's model option |
| `effort` | The harness's thought-level option: Claude's effort or Pi's thinking |
| `permission_mode` | Claude's permission mode: `auto`, `manual`, `acceptEdits` or `plan`. Pi has no mode option, so a Pi step that sets it fails the run |

Each value is the ID the harness's bridge offers, such as `claude-opus-5-5`,
`xhigh`, and `manual` for Claude, or `openai-codex/gpt-6.1-terra` and `high`
for Pi. The step's session is set to each value it gives before the prompt is
sent. A value the harness has no option for, or refuses, fails the run at
that step with the reason.

A step's agent can post [status updates](dev/glossary.md) through the
`ox` tool `post_status_update`.

## Shell steps

A shell step runs its `shell` command with `/bin/sh -c` in the run's
checkout, with stdin closed:

```yaml
  - name: Stage
    shell: git add -A && git diff --cached --name-only
```

Commands run with the environment of the client that started the run over
the daemon's, as agents do. `OX_RUN_INPUT` holds the run's input, and
`OX_STEP_INPUT` the step's input: the previous step's output, or the run's
input for the first step. A command reads them as `"$OX_STEP_INPUT"`. The
command's standard output is the step's output. A non-zero exit fails the run
with the exit status.

## Checks

`preconditions` and `postconditions` are lists of checks, each a `shell`
command that passes when it exits with status 0. Checks run in the run's
checkout with the same environment and variables as shell steps.

```yaml
    preconditions:
      - shell: test -z "$(git status --porcelain)"
    postconditions:
      - shell: test -f token.txt
```

The preconditions run in order before the step, and the first one that fails
fails the run; an agent step's session is not started. The postconditions run
in order after the step, stopping at the first that fails. A failed
postcondition of a shell step fails the run.

When a postcondition of an agent step fails, the same session gets one repair
turn with this prompt:

```text
A postcondition of this step failed after your turn. Fix what it checks so that it passes, or reply with the reason you cannot.

Command:
<command>

Result: <exited with status n | was killed by signal n | could not start>

Output:
<output>
```

The `Output:` section is left out when the command printed nothing. After the
repair turn, every postcondition runs again from the first. If one still
fails, the run fails, and the step keeps the repair turn's reply as its
output, so the agent's reason appears beside the failed check. Otherwise the
step succeeds, and the repair turn's reply is its output.

Each step lists the commands it ran in `commands`, with their kind, result,
exit code, and output: standard output and standard error interleaved as they
arrived. A command's output is recorded when it ends, and only its last
64 KiB is kept. A shell step hands its whole standard output to the next step.

## A full workflow

This workflow plans, implements, reviews, and commits a change. Plan, Work,
and Review each stage a new file, and their postconditions check that it is
staged. Plan's precondition requires a clean checkout and records the
starting commit in Git's metadata, and Commit's postcondition checks that the
run made one commit and left the checkout clean.

```yaml
name: Plan, Work, Review, Commit

steps:
  - name: Plan
    agent: &claude
      harness: claude
      model: claude-opus-5-5
      effort: high
      permission_mode: manual
    preconditions:
      - shell: |
          set -eu
          status=$(git status --porcelain --untracked-files=all)
          test -z "$status"
          start_head=$(git rev-parse HEAD)
          start_head_file=$(git rev-parse --git-path ox-workflow-start-head)
          printf '%s\n' "$start_head" > "$start_head_file"
    prompt: |
      Write an implementation plan for the task in your input.
      Save it as a new file under .agents/plans/ and stage it.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/plans/)"

  - name: Work
    agent:
      harness: pi
      model: openai-codex/gpt-6.1-terra
      effort: high
    prompt: |
      Implement the newly staged plan under .agents/plans/.
      Write a new work log under .agents/work/ describing the changes and verification.
      Stage the implementation and work log for the final commit.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/work/)"

  - name: Review
    agent: *claude
    prompt: |
      Review the staged implementation against the staged plan under .agents/plans/.
      Read the staged work log under .agents/work/ for verification details.
      Save your findings as a new file under .agents/reviews/ and stage it.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/reviews/)"

  - name: Commit
    agent:
      harness: claude
      model: claude-haiku-5-5
      effort: low
      permission_mode: manual
    prompt: |
      Read the staged diff, work log, and review record.
      Write a commit message that describes what changed and why.
      Commit the staged implementation and all three records together.
    postconditions:
      - shell: |
          set -eu
          start_head_file=$(git rev-parse --git-path ox-workflow-start-head)
          start_head=$(cat "$start_head_file")
          test "$(git rev-parse HEAD^ 2>/dev/null)" = "$start_head"
          status=$(git status --porcelain --untracked-files=all)
          test -z "$status"
          rm "$start_head_file"
```

## Running a workflow

In the desktop, the New Task menu of a workspace in a Git working tree lists
the workflows below New Task and Session in Checkout. Choosing one asks for
the run's input. Run, or Command-Return, starts the run and opens it.

The run's row is titled with its input, or with the workflow's name when the
input is blank, and sits in the workspace's Workflows group, below its
sessions, newest first by creation time. Every step is listed beneath the
run's row, titled with its name; an agent step's row is its child session once
the step starts, and the other rows select the run. The chevron after the
run's title, shown while the row is hovered, collapses and expands its steps,
as does double-clicking the row. Each row's status is a blue breathing dot
while it runs, an amber dot while a permission request or question awaits an
answer, a green dot once it succeeded, a red dot once it failed, and a
hollow dot while it is pending or once it was cancelled or interrupted.

Selecting a run opens the run view. Its header shows how long the run has
been running, or when it started once it has ended, the worktree's branch
and how it compares with its base, and the workflow's name. Below it is
every step, in order, with its start time once it has started and the
status updates its agent posted, newest last. While the run is active, the
view scrolls as needed to show the active step's latest status update. A failed
or interrupted run's reason shows under the step it stopped at. The oldest
permission request or question awaiting an answer among the children shows
below the steps. Answering it there or in the child's own view resolves it in
both. The right sidebar shows the run's worktree.

Selecting a child opens its session view. While the run is running, the
child's text entry and attachments are disabled, and Stop cancels the run.
Once the run has ended, the child takes prompts like any other session.

`oxctl run start --workspace <workspace id> <workflow> <input>` starts a run
and prints its ID. The run's worktree and branch are created as for
[New Task](worktrees.md#choosing-a-checkout).
`oxctl run wait <run id>` prints the run as JSON once it ends, and unless
the run succeeded fails with its state and error, such as
`the run failed: a postcondition exited with status 1 after the repair turn`.
`oxctl run ls` lists each run's ID, state and workflow name.

The run continues when every client disconnects. Each child session is an
ordinary session of the workspace: a client can follow it and answer its
permission requests and questions.

## Input and output

A step's prompt and its input go to the agent as two text blocks, prompt
first. The first step's input is the text the run was started with; an
empty input sends the prompt alone.

An agent step's turn succeeds when it ends with the `end_turn` stop reason.
Its output is the text of the last agent message of that turn, which becomes
the next step's input. A turn that ends any other way fails the run with its error or
stop reason, and the later steps stay pending.

## Cancelling, removing, and restarting

In the desktop, Cancel Run, in a running run's context menu and in the run
view's header, cancels the run. Delete Run, in the context menu of a run that
has ended, removes it, and asks before discarding unsaved work.

`oxctl run cancel <run id>` cancels the active step's turn, or stops its
running command: the command's process group gets `SIGTERM`, and `SIGKILL`
if the command has not exited 5 seconds later. The step and the run end
`cancelled`, and later steps stay pending. The child session's agent
stays running, so the session can be prompted, and the worktree keeps its
changes.

`oxctl run rm <run id>` removes a run that has ended. It stops the child
sessions' agents, deletes the child sessions, and removes the worktree and
its branch under the [deletion rules](worktrees.md#deleting-and-archiving).
`--discard` removes a worktree that has unsaved work.

Deleting or archiving a child session leaves the run's worktree and its
other child sessions. The step keeps its state and output.

A run that is running when the daemon stops ends `interrupted`, with its
running step and command `interrupted`. The daemon kills a running command's
process group as it stops. It keeps its record and its child sessions'
history, and its child sessions can be loaded.
