# Workflows

A workflow is a named sequence of agent steps. A run of a workflow gets its
own worktree, and each step runs in a new child session in that worktree.
The final reply of each step becomes the input of the next.

## Defining a workflow

Each workflow is a YAML file in `$XDG_CONFIG_HOME/ox/workflows/`, by default
`~/.config/ox/workflows/`. Every workspace and every daemon of the user shares
these definitions. A run is started by the file name without `.yaml`, such as
`plan-review` for `plan-review.yaml`. The file's `name` is the workflow's
display name.

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

## Agent steps

Each step has a `name`, a `prompt`, and an `agent`:

| Field | Value |
| --- | --- |
| `harness` | `claude` or `pi` |
| `model` | The harness's model option |
| `effort` | The harness's thought-level option: Claude's effort or Pi's thinking |
| `permission_mode` | The harness's mode option: Claude's permission mode |

Each value is the ID the harness's bridge offers, such as `claude-opus-5-5`,
`xhigh`, and `manual` for Claude, or `openai-codex/gpt-6.1-terra` and `high`
for Pi. The step's session is set to each value it gives before the prompt is
sent. A value the harness has no option for, or refuses, fails the run at
that step with the reason.

## Running a workflow

`oxctl run start --workspace <workspace id> <workflow> <input>` starts a run
and prints its ID. The run's worktree is on a new branch
`ox/<first 8 digits of the worktree's ID>` that starts at the workspace
checkout's `HEAD`, as for [New Task](worktrees.md#choosing-a-checkout).
`oxctl run wait <run id>` prints the run as JSON once it ends, and fails
unless the run succeeded.

The run continues when every client disconnects. Each child session is an
ordinary session of the workspace: a client can follow it and answer its
permission requests and questions.

## Input and output

A step's prompt and its input go to the agent as two text blocks, prompt
first. The first step's input is the text the run was started with; an
empty input sends the prompt alone.

A step succeeds when its turn ends with the `end_turn` stop reason. Its output
is the text of the last agent message of that turn, which becomes the next
step's input. A turn that ends any other way fails the run with its error or
stop reason, and the later steps stay pending.

## Cancelling, removing, and restarting

`oxctl run cancel <run id>` cancels the active step's turn. The step and the
run end `cancelled`, and later steps stay pending. The child session's agent
stays running, so the session can be prompted, and the worktree keeps its
changes.

`oxctl run rm <run id>` removes a run that has ended. It stops the child
sessions' agents, deletes the child sessions, and removes the worktree and
its branch under the [deletion rules](worktrees.md#deleting-and-archiving).
`--discard` removes a worktree that has unsaved work.

Deleting or archiving a child session leaves the run's worktree and its
other child sessions. The step keeps its state and output.

A run that is running when the daemon stops ends `interrupted`, with its
running step `interrupted`. It keeps its record and its child sessions'
history, and its child sessions can be loaded.
