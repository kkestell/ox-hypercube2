import type {
  ConversationUpdate,
  ListingChange,
  RunInfo,
  RunState,
  SessionInfo,
  SessionState,
  StepState,
  WorkflowInfo,
  WorkspaceInfo,
  WorktreeInfo,
} from "../src/lib/protocol";

/** A fixed state of the daemon, as the app's mocked backend reports it. */
export interface Scenario {
  /** The listing's changes: workspaces, then worktrees, sessions and runs. */
  changes: ListingChange[];
  /** Each session's conversation, by session ID. */
  conversations?: Record<string, ConversationUpdate[]>;
  workflows?: WorkflowInfo[];
}

/** Builds a scenario's listing in one workspace, oldest entry first. */
class Listing {
  readonly workspace: WorkspaceInfo = {
    id: "w",
    path: "/Users/dev/src/project",
    git_working_tree: true,
  };
  readonly worktrees: WorktreeInfo[] = [];
  readonly sessions: SessionInfo[] = [];
  readonly runs: RunInfo[] = [];
  private clock = Date.now() - 3_600_000;

  private tick() {
    this.clock += 60_000;
    return this.clock;
  }

  session(title: string, state: SessionState, worktreeId: string | null = null) {
    const session: SessionInfo = {
      id: `session-${this.sessions.length}`,
      command: [],
      workspace_id: this.workspace.id,
      cwd: this.workspace.path,
      state,
      title,
      agent_session_id: state === "starting" ? null : "agent",
      config_options: [],
      available_commands: [],
      worktree_id: worktreeId,
      archived: false,
      created_at: this.tick(),
    };
    this.sessions.push(session);
    return session;
  }

  /** A run whose steps each have a session, in `sessionState`, unless pending. */
  run(input: string, state: RunState, steps: [string, StepState, SessionState?][]) {
    const id = `run-${this.runs.length}`;
    const worktree: WorktreeInfo = {
      id: `worktree-${id}`,
      path: `${this.workspace.path}/.ox/${id}`,
      branch: `ox/${id}`,
      base: "main",
      created_branch: true,
      compare: "main",
      comparison: null,
    };
    this.worktrees.push(worktree);
    const created_at = this.tick();
    const run: RunInfo = {
      id,
      workspace_id: this.workspace.id,
      workflow: "implement",
      name: "Implement",
      input,
      worktree_id: worktree.id,
      state,
      error: state === "failed" ? "The Implement step failed" : null,
      steps: steps.map(([name, stepState, sessionState]) => ({
        name,
        state: stepState,
        started_at: stepState === "pending" ? null : created_at,
        session_id:
          sessionState === undefined ? null : this.session(name, sessionState, worktree.id).id,
      })),
      created_at,
    };
    this.runs.push(run);
    return run;
  }

  changes(): ListingChange[] {
    return [
      { type: "workspace", workspace: this.workspace },
      ...this.worktrees.map((worktree) => ({ type: "worktree" as const, worktree })),
      ...this.sessions.map((session) => ({ type: "session" as const, session })),
      ...this.runs.map((run) => ({ type: "run" as const, run })),
    ];
  }
}

/** Every session and run status in the sidebar. */
function sidebar(): Scenario {
  const listing = new Listing();
  listing.run("Fix the replay cursor", "failed", [
    ["Plan", "succeeded", "exited"],
    ["Implement", "failed", "exited"],
    ["Review", "pending"],
  ]);
  listing.run("Rename the store module", "cancelled", [
    ["Plan", "succeeded", "exited"],
    ["Implement", "cancelled", "exited"],
  ]);
  listing.run("Add run archiving", "succeeded", [
    ["Plan", "succeeded", "exited"],
    ["Implement", "succeeded", "exited"],
    ["Review", "succeeded", "exited"],
  ]);
  listing.run("Inject agent instructions", "running", [
    ["Plan", "succeeded", "exited"],
    ["Implement", "running", "awaiting_permission"],
    ["Review", "pending"],
  ]);
  listing.run("Align run view icons", "running", [
    ["Plan", "succeeded", "exited"],
    ["Implement", "running", "running"],
    ["Review", "pending"],
  ]);
  listing.session("Old exploration", "exited");
  listing.session("Explain the event store", "idle");
  listing.session("Which bugs next?", "awaiting_input");
  const working = listing.session("Fix sidebar colors", "running");
  return {
    changes: listing.changes(),
    conversations: {
      [working.id]: [
        {
          type: "prompt",
          key: "1",
          text: "Make running and succeeded look different.",
          attachments: [],
        },
        {
          type: "tool_call",
          key: "2",
          title: "Read ui/src/styles/controls.css",
          status: "completed",
          raw_input: null,
          content: [],
        },
        { type: "text", key: "3", text: "Both states use `--success`. ", replace: false },
      ],
    },
    workflows: [{ workflow: "implement", name: "Implement", error: null }],
  };
}

function runStatus(): Scenario {
  const listing = new Listing();
  const run = listing.run("Follow workflow status updates", "running", [
    ["Plan", "succeeded", "exited"],
    ["Implement", "running", "running"],
    ...Array.from({ length: 30 }, (_, index): [string, StepState] => [
      `Verify fixture ${index + 1}`,
      "pending",
    ]),
  ]);
  return {
    changes: listing.changes(),
    conversations: {
      [run.steps[0]!.session_id!]: [
        { type: "status_update", key: "1", text: "Read the run view", posted_at: run.created_at },
      ],
      [run.steps[1]!.session_id!]: [
        {
          type: "status_update",
          key: "1",
          text: "Updated the run view",
          posted_at: run.created_at,
        },
        { type: "status_update", key: "2", text: "Running checks", posted_at: run.created_at },
      ],
    },
  };
}

export const scenarios: Record<string, Scenario> = {
  sidebar: sidebar(),
  "run-status": runStatus(),
  empty: { changes: [] },
};
