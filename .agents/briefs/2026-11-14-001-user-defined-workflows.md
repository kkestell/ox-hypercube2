# User-defined workflows

## Problem

Ox lets users work with individual agent sessions. A task can span several
fresh sessions in the same checkout. Users need one place to start the task,
follow its progress, and answer questions from the agents doing the work.

## Outcome

A user can define a workflow in a file and launch it manually from any
workspace. Workflow definitions are shared across workspaces. Each run
creates its own worktree. The run shows its steps, live agent activity,
check results, and pending questions. Its history can be reopened after the
run ends.

## V1 workflow

The workflow for making a change runs each step in a separate agent session.
Plan, Work, and Review each write a file in the run's worktree. The workflow
checks that each file is staged before advancing.

1. **Plan** writes the implementation plan.
2. **Work** implements the plan and writes a work log covering the changes
   and verification.
3. **Review** reviews the implementation and writes a record of its findings.
4. **Commit** uses a lower-cost model to write a commit message describing
   the change and create one commit containing the implementation and all
   three records. The workflow checks that the commit was created and the
   checkout is clean.

### Example definition

The launch input describes the task. Work and Review find the durable
artifacts among the staged files in the run's worktree. The example starts
with a clean checkout and records the starting commit in Git's metadata so
the final check can verify that the workflow made one commit.

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

## Decisions

- A workflow is a linear sequence of agent and shell steps. Each agent step
  specifies its harness, model, effort, and prompt. It can set a permission
  mode when the harness exposes one. A shell step runs a command in the checkout.
- The run owns its worktree and all steps share it. Worktree cleanup belongs
  to the run. Child sessions can be removed independently.
- The launch input goes to the first step, and each successful step's output
  goes to the next step. An agent's final reply or a shell step's standard
  output can serve as its text output.
  Shell commands can read the launch input and preceding step's output as
  plain text supplied separately from the command.
- Preconditions and postconditions are shell commands. Exit zero passes the
  check. A failed precondition stops the run and shows the command output.
  If an agent step fails a postcondition, the same agent gets one repair turn
  with the failure details. Ox runs the checks again and stops the run if any
  still fail. If the agent explains why it cannot proceed, the reason is
  visible alongside the check result.
- Each agent step opens a child session. The workflow run presents the child
  sessions' conversations and check results in one timeline. The user can
  open a child session at any time and answer its questions and permission
  requests from either view. The workflow sends the prompts while the run is
  active. The run retains its worktree after it ends so the user can continue
  any child session.
- Closing the app leaves the run going. Cancelling stops the active step,
  prevents later steps from starting, and leaves checkout changes available
  for inspection. An interrupted run's history remains readable after a
  restart.

## Delivery sequence

1. **Separate worktree ownership from agent execution.** Move worktree
   creation, rollback, startup cleanup, branch comparison, and removal out
   of the session lifecycle. Consolidate the unsaved-work check used by
   delete and archive. Use New Task to verify creation, comparison, and
   cleanup through the existing user flow. If the ownership change requires
   a SQLite schema change, reset the database. Existing records are test data.
2. **Run two agent steps through `oxctl`.** The daemon loads a named app-wide
   YAML definition, creates the run's worktree, and advances real child
   sessions on turn completion. Verify plain-text handoff, cancellation,
   continued execution after a client disconnect, and history after a restart.
   A run can be removed using the existing unsaved-work and branch cleanup
   rules. Removing a child leaves the run's worktree available to the others.
3. **Add shell steps and checks.** Show command failures, support stopping a
   running command, and give an agent one repair turn after a failed
   postcondition. Exercise the full workflow definition with all intended
   changes staged before Review. Verify that the final check catches a
   missing commit and a dirty worktree.
4. **Add the desktop run view.** Make workflows available in each workspace.
   Show each step's child conversation and checks in the run timeline. Verify
   that users can answer the same request from either view and continue a
   child session after the run ends.

New Task and workflow runs share worktree operations, with one branch
comparison per worktree. Ordinary launches and workflow steps share harness
and bridge selection. Child sessions own their transcripts, and the run
records step boundaries, check results, and references to those sessions.
Both views reuse the conversation and request controls. The runner advances
through the ordered steps, and the definition supplies the prompts and Git
commands. Review each change for the existing code it replaces or consolidates.

## Out of scope

- Branching and loops.
- Sending free-form messages to agents during a run and resuming an
  interrupted run automatically.
- Schema-backed step output. V1 handoffs use plain text.
