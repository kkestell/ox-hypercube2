export type FileContent = { type: "text"; text: string } | { type: "error"; message: string };

// The messages the backend sends, and the fields of oxd's types the UI reads.

export type SessionState =
  | "starting"
  | "idle"
  | "running"
  | "awaiting_permission"
  | "awaiting_input"
  | "exited";

export interface SelectOption {
  value: string;
  name: string;
  _meta?: { "ox/provider"?: string } | null;
}

export interface SelectGroup {
  group: string;
  name: string;
  options: SelectOption[];
}

export type ConfigOption = {
  configId: string;
  name: string;
  /** What the option selects, such as `model`, `thought_level`, or `mode`. */
  category?: string;
} & (
  | { type: "select"; currentValue: string; options: SelectOption[] | SelectGroup[] }
  | { type: "boolean"; currentValue: boolean }
);

/** A slash command advertised by the current agent. */
export interface Command {
  type: "command";
  name: string;
  description: string;
  input?: { type: string; hint?: string } | null;
}

/** A config option value that a new session's agent is set to. */
export interface ConfigSetting {
  config_id: string;
  value: string | boolean;
}

/** The config options and slash commands an agent offers a new session. */
export interface Controls {
  config_options: ConfigOption[];
  available_commands: Command[];
}

export interface WorkspaceInfo {
  id: string;
  path: string;
  /** Whether sessions can run in a worktree of the workspace. */
  git_working_tree: boolean;
}

/**
 * Where a new session runs: the workspace's directory, or a new worktree on
 * an `ox/<id8>` branch, named for the worktree's ID, from `HEAD`, compared
 * with the ref `compare`.
 */
export type Checkout = { type: "workspace" } | { type: "worktree"; compare: string };

export interface WorktreeInfo {
  id: string;
  path: string;
  branch: string;
  base: string;
  created_branch: boolean;
  /** The ref the branch is compared with. */
  compare: string | null;
  comparison: Comparison | null;
}

/**
 * How a worktree's branch compares with its comparison ref: the two tips and
 * the commits each has that the other lacks, or git's error.
 */
export type Comparison =
  | { type: "compared"; tip: string; compare_tip: string; ahead: number; behind: number }
  | { type: "failed"; error: string };

/** A workspace's branches, as the UI reads them. */
export interface Branches {
  /** The branch the workspace's checkout is on. */
  head: string | null;
}

/** The outcome of deleting a session or a run. */
export type Deletion =
  | { type: "deleted"; kept_branch: string | null }
  | { type: "unsaved_work"; message: string };

export type Archival = { type: "archived" } | { type: "unsaved_work"; message: string };

export interface SessionInfo {
  id: string;
  command: string[];
  workspace_id: string;
  cwd: string;
  state: SessionState;
  title: string | null;
  agent_session_id: string | null;
  config_options: ConfigOption[];
  available_commands: Command[];
  /** The ID of the worktree the session runs in. */
  worktree_id: string | null;
  archived: boolean;
  /** When the session was created, in milliseconds since the Unix epoch. */
  created_at: number;
  /** How the session's turns ended while it was out of focus, until it is focused. */
  unseen: Unseen | null;
}

/** How a session's turn or a run ended while no client had it in focus. */
export type Unseen = "finished" | "error";

/** The session or run the window shows in front of the user. */
export type Focus = { type: "session"; session_id: string } | { type: "run"; run_id: string };

/** A definition in oxd's workflows directory. */
export interface WorkflowInfo {
  /** The definition's file name, without `.yaml`. */
  workflow: string;
  /** The definition's name, or `workflow` when the file does not parse. */
  name: string;
  /** Why the file does not parse. */
  error: string | null;
}

export type RunState = "running" | "succeeded" | "failed" | "cancelled" | "interrupted";

export type StepState =
  | "pending"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "interrupted";

/** A run of a workflow, whose steps run in its worktree, agent steps in child sessions. */
export interface RunInfo {
  id: string;
  workspace_id: string;
  /** The definition's file name, without `.yaml`. */
  workflow: string;
  /** The definition's name. */
  name: string;
  input: string;
  worktree_id: string;
  state: RunState;
  /** Why the run failed. */
  error: string | null;
  steps: StepInfo[];
  /** When the run was started, in milliseconds since the Unix epoch. */
  created_at: number;
  /** How the run ended while it was out of focus, until it is focused. */
  unseen: Unseen | null;
}

export interface StepInfo {
  name: string;
  state: StepState;
  /** When the step left `pending`, in milliseconds since the Unix epoch. */
  started_at: number | null;
  /** The step's child session, once an agent step has started. */
  session_id: string | null;
}

/** An entry of a directory, as a directory watch lists it. */
export interface DirEntry {
  name: string;
  kind: "file" | "directory";
}

export interface PermissionOption {
  optionId: string;
  name: string;
  kind: "allow_once" | "allow_always" | "reject_once" | "reject_always";
}

/** A choice in a form field: the value it sends and its label. */
export interface EnumOption {
  const: string;
  title: string;
  description?: string;
}

/** One property of an agent's form, with its JSON Schema keywords. */
export type Field = {
  name: string;
  required: boolean;
  title?: string;
  description?: string;
} & (
  | { type: "string"; default?: string; oneOf?: EnumOption[] }
  | {
      type: "array";
      minItems?: number;
      maxItems?: number;
      items: { anyOf?: EnumOption[] };
      default?: string[];
    }
);

export type ToolCallStatus = "pending" | "in_progress" | "completed" | "failed";

export type ToolContent =
  | { type: "text"; text: string }
  | { type: "image"; mime_type: string; data: string };

export type Item =
  | { type: "prompt"; key: string; text: string; attachments: string[] }
  | { type: "text"; key: string; text: string; replace: boolean }
  | { type: "thought"; key: string; text: string; replace: boolean }
  | {
      type: "tool_call";
      key: string;
      title: string;
      status: ToolCallStatus;
      /** The tool's input as JSON, or `null`. */
      raw_input: unknown;
      content: ToolContent[];
    }
  | {
      type: "permission";
      key: string;
      request_id: string;
      title: string;
      options: PermissionOption[];
    }
  | {
      type: "elicitation";
      key: string;
      request_id: string;
      message: string;
      fields: Field[];
    }
  | { type: "turn_completed"; key: string }
  | { type: "turn_error"; key: string; message: string }
  | { type: "agent_exited"; key: string; error: string | null }
  | { type: "session_closed"; key: string };

/** An open permission request or form. */
export type Request = Extract<Item, { type: "permission" | "elicitation" }>;

export function isRequest(item: Item): item is Request {
  return item.type === "permission" || item.type === "elicitation";
}

export type ConversationUpdate = Item | { type: "request_resolved"; request_id: string };

/**
 * A change to the listing: a workspace, worktree, session or run added or
 * changed, or one removed. One that is new comes last; one that changed keeps
 * its place. A session's worktree comes before the session, and a run's
 * worktree and child sessions before the run.
 */
export type ListingChange =
  | { type: "workspace"; workspace: WorkspaceInfo }
  | { type: "workspace_removed"; workspace_id: string }
  | { type: "worktree"; worktree: WorktreeInfo }
  | { type: "worktree_removed"; worktree_id: string }
  | { type: "session"; session: SessionInfo }
  | { type: "session_removed"; session_id: string }
  | { type: "run"; run: RunInfo }
  | { type: "run_removed"; run_id: string };

export type WatchMessage =
  /** A watch's first changes list every workspace, worktree, session and run. */
  | { type: "changes"; changes: ListingChange[] }
  | { type: "error"; message: string }
  | { type: "disconnected" };
