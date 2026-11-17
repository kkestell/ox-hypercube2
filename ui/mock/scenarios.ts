import type {
  Comparison,
  ConversationUpdate,
  ListingChange,
  SessionInfo,
  SessionState,
  WorkspaceInfo,
  WorktreeInfo,
} from "../src/lib/protocol";

/** A fixed state of the daemon, as the app's mocked backend reports it. */
export interface Scenario {
  /** The listing's changes: workspaces, then worktrees and sessions. */
  changes: ListingChange[];
  /** Each session's conversation, by session ID. */
  conversations?: Record<string, ConversationUpdate[]>;
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
  private clock = Date.now() - 3_600_000;

  /** A minute after the previous time this listing gave out. */
  tick() {
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
      unseen: null,
      queue: [],
    };
    this.sessions.push(session);
    return session;
  }

  /**
   * An idle session in a new worktree, compared with `main` as `comparison`
   * says, or not yet compared when it is null.
   */
  worktreeSession(title: string, comparison: Comparison | null) {
    const id = `worktree-${this.worktrees.length}`;
    this.worktrees.push({
      id,
      path: `${this.workspace.path}/.ox/${id}`,
      branch: `ox/${id}`,
      base: "main",
      created_branch: true,
      compare: "main",
      comparison,
    });
    return this.session(title, "idle", id);
  }

  changes(): ListingChange[] {
    return [
      { type: "workspace", workspace: this.workspace },
      ...this.worktrees.map((worktree) => ({ type: "worktree" as const, worktree })),
      ...this.sessions.map((session) => ({ type: "session" as const, session })),
    ];
  }
}

/** Every session status in the sidebar. */
function sidebar(): Scenario {
  const listing = new Listing();
  for (const title of ["Try a flat sidebar", "Profile replay", "Draft the README"]) {
    listing.session(title, "exited").archived = true;
  }
  listing.session("Old exploration", "exited");
  listing.session("Debug the parser crash", "exited").unseen = "error";
  const explainer = listing.session("Explain the event store", "idle");
  explainer.unseen = "finished";
  const reviewer = listing.session("Hand off the store review", "awaiting_permission");
  listing.session("Which bugs next?", "awaiting_input");
  const working = listing.session("Fix sidebar colors", "running");
  working.queue = [
    {
      id: "queued-1",
      prompt: [{ type: "text", text: "Then check the dots in the dark theme." }],
    },
    {
      id: "queued-2",
      prompt: [
        { type: "text", text: "Add a screenshot of both themes to the pull request." },
        {
          type: "resource_link",
          name: "tokens.css",
          uri: "file:///src/ox/ui/src/styles/tokens.css",
        },
      ],
    },
  ];
  return {
    changes: listing.changes(),
    conversations: {
      [working.id]: [
        {
          type: "prompt",
          key: "1",
          text: "Why do the sidebar's status dots look the same?",
          attachments: [],
        },
        {
          type: "thought",
          key: "thought:1",
          text: "The dots take their colours from the status classes.",
          replace: false,
        },
        {
          type: "text",
          key: "text:1",
          text: "Checking the status styles.",
          replace: false,
        },
        {
          type: "tool_call",
          key: "tool:1",
          title: "Read `ui/src/features/sessions/SessionList.tsx`",
          status: "completed",
          raw_input: { file_path: "ui/src/features/sessions/SessionList.tsx" },
          content: [],
        },
        {
          type: "tool_call",
          key: "tool:2",
          title: "Search for `--success`",
          status: "completed",
          raw_input: { pattern: "--success" },
          content: [{ type: "text", text: "ui/src/styles/controls.css" }],
        },
        {
          type: "text",
          key: "text:2",
          text: "Running and succeeded sessions both use `--success`.",
          replace: false,
        },
        { type: "turn_completed", key: "7" },
        {
          type: "prompt",
          key: "8",
          text: "Make running and succeeded look different.",
          attachments: [],
        },
        {
          type: "tool_call",
          key: "tool:3",
          title: "Read ui/src/styles/controls.css",
          status: "completed",
          raw_input: null,
          content: [],
        },
        {
          type: "text",
          key: "text:3",
          text: "Both states use `--success`. ",
          replace: false,
        },
        {
          type: "steered",
          key: "12",
          text: "Keep the failed colour as it is.",
          attachments: [],
        },
      ],
      [explainer.id]: [
        {
          type: "prompt",
          key: "1",
          text: "Hand the event store change to another session for review.",
          attachments: [],
        },
        {
          type: "tool_call",
          key: "tool:1",
          title: "ListSessions",
          status: "completed",
          raw_input: {},
          content: [
            {
              type: "text",
              text:
                "Workspace /Users/dev/src/project\n" +
                `- id: ${reviewer.id}, title: "Hand off the store review", state: idle, ` +
                "harness: claude, directory: /Users/dev/src/project, branch: main\n" +
                `- id: ${working.id}, title: "Fix sidebar colors", state: running, ` +
                "harness: claude, directory: /Users/dev/src/project, branch: main",
            },
          ],
        },
        {
          type: "tool_call",
          key: "tool:2",
          title: "SendMessage `The event store change is ready. Can someone review it?`",
          status: "completed",
          raw_input: {
            to: reviewer.id,
            text: "The event store change is ready. Can someone review it?",
          },
          content: [{ type: "text", text: `Sent. Session ${reviewer.id} started a turn with it.` }],
        },
        {
          type: "text",
          key: "text:1",
          text: "I sent the change to **Hand off the store review**.",
          replace: false,
        },
        { type: "turn_completed", key: "5" },
        {
          type: "prompt",
          key: "6",
          text: "On it. I'm asking the user to start a review session in a new worktree.",
          attachments: [],
          from: { session_id: reviewer.id, title: "Hand off the store review" },
        },
        {
          type: "text",
          key: "text:2",
          text: "The reviewer is asking for a review session.",
          replace: false,
        },
        { type: "turn_completed", key: "8" },
      ],
      [reviewer.id]: [
        {
          type: "prompt",
          key: "1",
          text: "The event store change is ready. Can someone review it?",
          attachments: [],
          from: { session_id: explainer.id, title: "Explain the event store" },
        },
        {
          type: "tool_call",
          key: "tool:1",
          title:
            "SendMessage `On it. I'm asking the user to start a review session in a new worktree.`",
          status: "completed",
          raw_input: {
            to: explainer.id,
            text: "On it. I'm asking the user to start a review session in a new worktree.",
          },
          content: [
            { type: "text", text: `Sent. Session ${explainer.id} started a turn with it.` },
          ],
        },
        {
          type: "tool_call",
          key: "tool:2",
          title: "StartSession `Review the event store change on main.`",
          status: "in_progress",
          raw_input: { prompt: "Review the event store change on main.", worktree: true },
          content: [],
        },
        {
          type: "permission",
          key: "request:1",
          request_id: "1",
          title: "Start a session in project?",
          session_request: {
            harness: "claude",
            model: "Opus 5.5",
            provider: null,
            effort: "High",
            mode: "Manual",
            checkout: { type: "worktree", branch: null, base: "main" },
            prompt: "Review the event store change on main.",
          },
          options: [
            { optionId: "start", name: "Start Session", kind: "allow_once" },
            { optionId: "decline", name: "Decline", kind: "reject_once" },
          ],
        },
      ],
    },
  };
}

/** A branch `ahead` and `behind` its comparison ref. */
function compared(ahead: number, behind: number): Comparison {
  return { type: "compared", tip: "b".repeat(40), compare_tip: "a".repeat(40), ahead, behind };
}

/** Sessions whose Merge button is enabled, and blocked for each reason. */
function merge(): Scenario {
  const listing = new Listing();
  listing.worktreeSession("Failed comparison", {
    type: "failed",
    error: "fatal: bad revision 'main'",
  });
  listing.worktreeSession("No commits", compared(0, 0));
  listing.worktreeSession("Behind main", compared(1, 2));
  listing.worktreeSession("Unsquashed commits", compared(3, 0));
  listing.worktreeSession("Ready to merge", compared(1, 0));
  return { changes: listing.changes() };
}

/** A session in the workspace's checkout, and worktree sessions in each comparison state. */
function checkouts(): Scenario {
  const listing = new Listing();
  listing.worktreeSession("Worktree: comparison failed", {
    type: "failed",
    error: "fatal: bad revision 'main'",
  });
  listing.worktreeSession("Worktree: 1 ahead, 2 behind (rebase)", compared(1, 2));
  listing.worktreeSession("Worktree: 3 ahead (squash)", compared(3, 0));
  listing.worktreeSession("Worktree: 1 ahead (mergeable)", compared(1, 0));
  listing.worktreeSession("Worktree: no commits, 2 behind", compared(0, 2));
  listing.worktreeSession("Worktree: no commits", compared(0, 0));
  listing.worktreeSession("Worktree: not compared yet", null);
  listing.session("Checkout", "idle");
  return { changes: listing.changes() };
}

export const scenarios: Record<string, Scenario> = {
  sidebar: sidebar(),
  merge: merge(),
  checkouts: checkouts(),
  empty: { changes: [] },
};
