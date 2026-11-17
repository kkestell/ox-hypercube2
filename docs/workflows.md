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
display name. The desktop lists every definition, each time a New Session menu
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
      permission_mode: auto
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

## Example workflows

These workflows carry a change through the `kplan`, `kwork`, and `kreview`
skills and commit it. Each skill step stages a new file, and its
postcondition checks that the file is staged. The first step's precondition
requires a clean checkout and records the starting commit in Git's metadata.
Commit's precondition requires staged changes outside the skills' artifacts,
and its postcondition checks that the run made one commit and left the
checkout clean.

`plan-work-review-commit.yaml` plans a change from a request:

```yaml
name: Plan, Work, Review, Commit

# The run's input is the change to plan: a small, clear request.
# Each step runs one of the kplan, kwork, and kreview skills, which keep their
# artifacts in .agents/ if the repository has it, otherwise in agents/.

steps:
  - name: Plan
    agent: &claude
      harness: claude
      model: claude-opus-5-5
      effort: high
      permission_mode: auto
    preconditions:
      - shell: |
          set -eu
          status=$(git status --porcelain --untracked-files=all)
          test -z "$status"
          start_head=$(git rev-parse HEAD)
          start_head_file=$(git rev-parse --git-path ox-workflow-start-head)
          printf '%s\n' "$start_head" > "$start_head_file"
    prompt: |
      Use the kplan skill with your input as its argument.

      This turn is one step of an Ox workflow: the step ends when your turn ends, and your
      last message becomes the next step's input. Ask the user each decision the skill needs
      with the AskUserQuestion tool, which waits for the answer within this turn. Never end
      your turn to wait for an answer. Present the Check your assumptions entries this way,
      one question per entry, offering the decision you made and its alternative, in as many
      calls as the list needs.

      When the plan is staged, reply with kplan's final response.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/plans/ agents/plans/)"

  - name: Work
    agent: *claude
    prompt: |
      Your input is kplan's final response. Use the kwork skill with the plan path it names
      as its argument.

      This turn is one step of an Ox workflow: the step ends when your turn ends, and your
      last message becomes the next step's input. Ask the user each decision the skill needs
      with the AskUserQuestion tool, which waits for the answer within this turn. Never end
      your turn to wait for an answer.

      When the work is staged, reply with kwork's final response.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/work/ agents/work/)"

  - name: Review
    agent: *claude
    prompt: |
      Your input is kwork's final response. Use the kreview skill with the review scope its
      Next line names as its argument. Fix the High and Medium findings. Record each Low
      finding in the review with the fix label `Not fixed` and leave its code unchanged.

      This turn is one step of an Ox workflow: the step ends when your turn ends, and your
      last message becomes the next step's input. Ask the user each decision a High or Medium
      fix needs with the AskUserQuestion tool, which waits for the answer within this turn.
      Never end your turn to wait for an answer.

      When the review is staged, reply with kreview's final response.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/reviews/ agents/reviews/)"

  - name: Commit
    agent:
      harness: claude
      model: claude-sonnet-5-5
      effort: low
      permission_mode: auto
    preconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only -- . ':(exclude).agents' ':(exclude)agents')"
    prompt: |
      Your input is the review's final response. Commit everything staged in one commit.
      The subject is the plan's title: the first line of the staged plan, without its leading
      `# `. The body is two or three sentences on what changed, taken from your input and the
      staged work log. Reply with the commit's short hash and subject.
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

`work-review-commit.yaml` implements a plan that already exists:

```yaml
name: Work, Review, Commit

# The run's input identifies a committed plan by its path or slug, or is the
# plan's full text. Each step runs one of the kwork and kreview skills, which
# keep their artifacts in .agents/ if the repository has it, otherwise in agents/.

steps:
  - name: Work
    agent: &claude
      harness: claude
      model: claude-opus-5-5
      effort: high
      permission_mode: auto
    preconditions:
      - shell: |
          set -eu
          status=$(git status --porcelain --untracked-files=all)
          test -z "$status"
          start_head=$(git rev-parse HEAD)
          start_head_file=$(git rev-parse --git-path ox-workflow-start-head)
          printf '%s\n' "$start_head" > "$start_head_file"
    prompt: |
      Use the kwork skill with your input as its argument. If your input is a plan's full
      text rather than its path or slug, first save it as a new plan, named as kplan names
      plans, and stage it; then use the kwork skill with its path.

      This turn is one step of an Ox workflow: the step ends when your turn ends, and your
      last message becomes the next step's input. Ask the user each decision the skill needs
      with the AskUserQuestion tool, which waits for the answer within this turn. Never end
      your turn to wait for an answer.

      When the work is staged, reply with kwork's final response.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/work/ agents/work/)"

  - name: Review
    agent: *claude
    prompt: |
      Your input is kwork's final response. Use the kreview skill with the review scope its
      Next line names as its argument. Fix the High and Medium findings. Record each Low
      finding in the review with the fix label `Not fixed` and leave its code unchanged.

      This turn is one step of an Ox workflow: the step ends when your turn ends, and your
      last message becomes the next step's input. Ask the user each decision a High or Medium
      fix needs with the AskUserQuestion tool, which waits for the answer within this turn.
      Never end your turn to wait for an answer.

      When the review is staged, reply with kreview's final response.
    postconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only --diff-filter=A -- .agents/reviews/ agents/reviews/)"

  - name: Commit
    agent:
      harness: claude
      model: claude-haiku-5-5
      effort: low
      permission_mode: auto
    preconditions:
      - shell: >-
          test -n "$(git diff --cached --name-only -- . ':(exclude).agents' ':(exclude)agents')"
    prompt: |
      Your input is the review's final response. Commit everything staged in one commit.
      The subject is the plan's title: the first line of the plan named in the staged work
      log, without its leading `# `. The body is two or three sentences on what changed, taken
      from your input and the staged work log. Reply with the commit's short hash and subject.
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

In the desktop, the New Session menu of a workspace in a Git working tree lists
the workflows below Session in Worktree and Session in Checkout. Choosing one asks for
the run's input. Run, or Command-Return, starts the run and opens it.

The run's row is titled with its input, or with the workflow's name when the
input is blank, and is listed among the workspace's sessions, newest first by
creation time. The chevron after the run's title, shown while the row is
hovered, expands and collapses its steps, as does double-clicking the row; a
run starts collapsed. Each step is listed beneath the run's row, titled with
its name; an agent step's row is its child session once the step starts, and
the other rows select the run. The run's row shows its worktree's branch icon
as a [Session in Worktree](worktrees.md#comparing-a-sessions-branch) row does. Each row's dot shows what it needs from
the user, as a session's [status dot](../spec/sessions.md#status-dots) does:

- The run's row is orange while a child awaits an answer, red while the
  run's failure or interruption or a child's error is unseen, blue while the
  run runs, green while its success is unseen, and gray otherwise.
- A step's row is orange while its child awaits an answer, red while its
  child's error is unseen or, once the step failed or was interrupted, while
  the run's failure is unseen, blue while it runs, and gray otherwise. A step
  never turns green.
- Viewing the run clears the run's and its children's red and green; viewing
  a child clears only the child's. A run or child that ends while it is in
  view turns nothing red or green.

Each row's tooltip names its state, such as Succeeded or Failed, and the run
view shows each step's outcome. A pending step's row is dimmed.

Selecting a run opens the run view. Its header shows how long the run has
been running, or when it started once it has ended, the worktree's branch
and how it compares with its base, and the workflow's name when it fits.
Below it is every step, in order, with its start time once it has started and
the messages its agent wrote, oldest first. A step's dot there is green once it
succeeded and red once it failed. A failed or interrupted run's reason shows
under the step it stopped at. The steps fill the view's height. Clicking the
name of a step with messages collapses or expands them; a step shows its
messages until it has succeeded and a later step shows its first message or
the run's failure, unless you have expanded or collapsed it.
Expanded steps share the height the collapsed steps leave, each taking no more
than its messages need, and a step whose messages do not fit scrolls them. Each
step's messages stay scrolled to their end as they grow, until you scroll away
from the end. A view too short for every step scrolls. The oldest
permission request or question awaiting an answer among the children shows
below the steps. Answering it there or in the child's own view resolves it in
both. The right sidebar shows the run's worktree.

Selecting a child opens its session view. While the run is running, the
child's text entry and attachments are disabled, and Stop cancels the run.
Once the run has ended, the child takes prompts like any other session.

`oxctl run start --workspace <workspace id> <workflow> <input>` starts a run
and prints its ID. The run's worktree and branch are created as for
[Session in Worktree](worktrees.md#choosing-a-checkout).
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

In the desktop, Cancel Run in a running run's context menu, or Cancel in the
run view's header, cancels the run. Delete Run, in the context menu of a run that
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
