import { invoke } from "@tauri-apps/api/core";
import { Menu } from "@tauri-apps/api/menu";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronRight, FolderPlus, GitBranch, Plus } from "lucide-react";
import { type MouseEvent, memo, useCallback, useState } from "react";
import { Dialog } from "../../components/Dialog";
import { HarnessLogo } from "../../components/HarnessLogo";
import { SelectPicker } from "../../components/SelectPicker";
import { Sidebar } from "../../components/Sidebar";
import type {
  Branches,
  Checkout,
  RunInfo,
  SessionInfo,
  SessionState,
  StepInfo,
  StepState,
  WorkflowInfo,
  WorkspaceInfo,
  WorktreeInfo,
} from "../../lib/protocol";
import { showError } from "../../lib/showError";
import { branchStatus, localMerge } from "./localMerge";
import { type AgentId, agentNames, useAgents } from "./useAgents";
import type { SessionActions } from "./useSessionActions";

const stateLabels: Record<SessionState, string> = {
  starting: "Starting",
  idle: "Idle",
  running: "Running",
  awaiting_permission: "Awaiting permission",
  awaiting_input: "Awaiting input",
  exited: "Exited",
};

/** Whether the session awaits a permission answer or a form. */
function awaiting(session: SessionInfo) {
  return session.state === "awaiting_permission" || session.state === "awaiting_input";
}

/** A session's, a run's or a step's status class and its label. */
interface Status {
  className: string;
  label: string;
}

/** A status's dot. */
export function StatusIcon({ status }: { status: Status }) {
  return (
    <span
      className={`status ${status.className}`}
      role="img"
      aria-label={status.label}
      title={status.label}
    />
  );
}

const settled: Record<Exclude<StepState, "running">, Status> = {
  pending: { className: "pending", label: "Pending" },
  succeeded: { className: "succeeded", label: "Succeeded" },
  failed: { className: "failed", label: "Failed" },
  cancelled: { className: "cancelled", label: "Cancelled" },
  interrupted: { className: "interrupted", label: "Interrupted" },
};

/** A run's status, from its state and its child sessions'. */
function runStatus(run: RunInfo, children: readonly SessionInfo[]): Status {
  const waiting = children.find(awaiting);
  if (waiting) return { className: "attention", label: stateLabels[waiting.state] };
  if (run.state === "running") return { className: "running", label: "Running" };
  return settled[run.state];
}

/** A step's status, from its state and its child session's, if it has one. */
export function childStatus(step: StepInfo, session: SessionInfo | undefined): Status {
  if (session && awaiting(session)) {
    return { className: "attention", label: stateLabels[session.state] };
  }
  if (step.state === "running" || session?.state === "running") {
    return { className: "running", label: "Running" };
  }
  return settled[step.state];
}

/**
 * A run's title: its input with runs of whitespace collapsed, or its
 * workflow's name when the input is blank.
 */
export function runTitle(run: RunInfo) {
  return run.input.replace(/\s+/g, " ").trim() || run.name;
}

/** Opens a native context menu of `items` at the pointer. */
async function contextMenu(
  event: MouseEvent,
  items: { text: string; enabled: boolean; checked?: boolean; action: () => void }[],
) {
  event.preventDefault();
  const menu = await Menu.new({ items });
  await menu.popup();
}

/** The last component of a workspace's path. */
function directoryName(path: string) {
  return path.split("/").filter(Boolean).pop() ?? path;
}

async function run(action: () => Promise<unknown>) {
  try {
    await action();
  } catch (e) {
    await showError(e);
  }
}

/** `set` with `id` added, or removed when it holds it. */
function toggled(set: ReadonlySet<string>, id: string): ReadonlySet<string> {
  const next = new Set(set);
  if (!next.delete(id)) next.add(id);
  return next;
}

/** Newest first by creation time. */
function newestFirst(a: { created_at: number }, b: { created_at: number }) {
  return b.created_at - a.created_at;
}

export const SessionList = memo(function SessionList({
  workspaces,
  worktrees,
  sessions,
  runs,
  parents,
  selected,
  onSelect,
  actions: { busy, deleteSession, archiveSession, unarchiveSession, cancelRun, deleteRun },
}: {
  workspaces: WorkspaceInfo[];
  worktrees: Map<string, WorktreeInfo>;
  sessions: SessionInfo[];
  runs: RunInfo[];
  /** The run each child session belongs to, by session ID. */
  parents: Map<string, RunInfo>;
  selected: string | null;
  /** Views the session or run with this ID. */
  onSelect: (id: string) => void;
  actions: SessionActions;
}) {
  const agents = useAgents();
  // The agent the user chose, and the agent the New Session menus start: the
  // chosen one while it is installed, otherwise the first installed agent.
  const [chosen, setChosen] = useState<AgentId | null>(null);
  const agent = chosen !== null && agents.includes(chosen) ? chosen : agents[0];
  // The workspaces that list their archived sessions.
  const [showArchived, setShowArchived] = useState<ReadonlySet<string>>(new Set());
  // The runs whose steps show.
  const [expandedRuns, setExpandedRuns] = useState<ReadonlySet<string>>(new Set());
  // The workspaces whose rows are collapsed.
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<ReadonlySet<string>>(new Set());
  // The definitions the New Session menus list, read each time one opens.
  const [workflows, setWorkflows] = useState<WorkflowInfo[]>([]);
  // The workflow whose input the Start Run dialog asks for.
  const [starting, setStarting] = useState<{
    workspaceId: string;
    workflow: WorkflowInfo;
  } | null>(null);

  const addWorkspace = async () => {
    const path = await open({ directory: true, title: "Add Workspace" });
    if (path === null) return;
    await run(() => invoke("add_workspace", { path }));
  };

  const removeWorkspace = (workspaceId: string) =>
    run(() => invoke("remove_workspace", { workspaceId }));

  const newSession = (workspaceId: string, agent: AgentId, checkout: Checkout) =>
    run(async () => {
      const session = await invoke<SessionInfo>("new_session", { agent, workspaceId, checkout });
      onSelect(session.id);
    });

  // A session in a new worktree, on a new `ox/<id8>` branch named for the
  // worktree's ID, from `HEAD`, compared with the branch the workspace's
  // checkout is on.
  const newWorktreeSession = (workspace: WorkspaceInfo, agent: AgentId) =>
    run(async () => {
      const { head } = await invoke<Branches>("workspace_branches", {
        workspaceId: workspace.id,
      });
      if (head === null) {
        await showError(`${directoryName(workspace.path)} is not on a branch`);
        return;
      }
      const checkout: Checkout = { type: "worktree", compare: head };
      const session = await invoke<SessionInfo>("new_session", {
        agent,
        workspaceId: workspace.id,
        checkout,
      });
      onSelect(session.id);
    });

  // Viewing a saved session whose agent is not running starts its agent, which
  // replays the conversation.
  const select = useCallback(
    async (session: SessionInfo) => {
      if (session.state === "exited" && !session.archived && session.agent_session_id !== null) {
        await run(() => invoke("load_session", { sessionId: session.id }));
      }
      onSelect(session.id);
    },
    [onSelect],
  );

  const toggleRun = useCallback(
    (runId: string) => setExpandedRuns((expanded) => toggled(expanded, runId)),
    [],
  );

  const children = new Map<string, SessionInfo[]>();
  for (const session of sessions) {
    const parent = parents.get(session.id);
    if (parent === undefined) continue;
    const list = children.get(parent.id) ?? [];
    list.push(session);
    children.set(parent.id, list);
  }
  for (const list of children.values()) list.sort((a, b) => a.created_at - b.created_at);

  const sessionRow = (
    session: SessionInfo,
    child?: { step: StepInfo; first: boolean; last: boolean },
  ) => (
    <SessionRow
      key={session.id}
      session={session}
      worktree={session.worktree_id === null ? undefined : worktrees.get(session.worktree_id)}
      selected={session.id === selected}
      busy={busy.has(session.id)}
      step={child?.step}
      first={child?.first ?? false}
      last={child?.last ?? false}
      onSelect={select}
      deleteSession={deleteSession}
      archiveSession={archiveSession}
      unarchiveSession={unarchiveSession}
    />
  );

  const runRows = (run: RunInfo) => {
    const runChildren = children.get(run.id) ?? [];
    const status = runStatus(run, runChildren);
    const expanded = expandedRuns.has(run.id);
    const row = (
      <RunRow
        key={run.id}
        run={run}
        worktree={worktrees.get(run.worktree_id)}
        status={status.className}
        label={status.label}
        selected={run.id === selected}
        busy={busy.has(run.id)}
        expanded={expanded}
        onSelect={onSelect}
        onToggle={toggleRun}
        cancelRun={cancelRun}
        deleteRun={deleteRun}
      />
    );
    if (!expanded) return [row];
    return [
      row,
      ...run.steps.map((step, index) => {
        const child = {
          step,
          first: index === 0,
          last: index === run.steps.length - 1,
        };
        const session = runChildren.find((session) => session.id === step.session_id);
        if (session !== undefined) return sessionRow(session, child);
        return (
          <StepRow
            // biome-ignore lint/suspicious/noArrayIndexKey: a run's steps never move
            key={`${run.id}:${index}`}
            step={step}
            first={child.first}
            last={child.last}
            onSelect={() => onSelect(run.id)}
          />
        );
      }),
    ];
  };

  return (
    <Sidebar
      actions={
        <button
          type="button"
          className="icon-button"
          title="Add Workspace"
          aria-label="Add Workspace"
          onClick={addWorkspace}
        >
          <FolderPlus />
        </button>
      }
    >
      {workspaces.map((workspace) => {
        const workspaceSessions = sessions.filter(
          (session) => session.workspace_id === workspace.id,
        );
        const workspaceRuns = runs.filter((run) => run.workspace_id === workspace.id);
        const archivedShown = showArchived.has(workspace.id);
        const rows = [
          ...workspaceSessions
            .filter((session) => !parents.has(session.id) && (archivedShown || !session.archived))
            .map((session) => ({
              created_at: session.created_at,
              rows: [sessionRow(session)],
            })),
          ...workspaceRuns.map((run) => ({ created_at: run.created_at, rows: runRows(run) })),
        ].sort(newestFirst);
        const open = !collapsedWorkspaces.has(workspace.id);
        const menu = `new-session-${workspace.id}`;
        return (
          <section
            key={workspace.id}
            className="workspace"
            aria-labelledby={`workspace-${workspace.id}`}
          >
            <div className="workspace-header">
              <h2
                id={`workspace-${workspace.id}`}
                title={workspace.path}
                onContextMenu={(event) =>
                  contextMenu(event, [
                    {
                      text: "Show Archived",
                      enabled: true,
                      checked: archivedShown,
                      action: () => setShowArchived((shown) => toggled(shown, workspace.id)),
                    },
                    {
                      text: "Remove Workspace",
                      enabled: workspaceSessions.length === 0 && workspaceRuns.length === 0,
                      action: () => removeWorkspace(workspace.id),
                    },
                  ])
                }
              >
                <button
                  type="button"
                  className="workspace-toggle"
                  aria-expanded={open}
                  onClick={() =>
                    setCollapsedWorkspaces((collapsed) => toggled(collapsed, workspace.id))
                  }
                >
                  <span className="workspace-name">{directoryName(workspace.path)}</span>
                  <span className={`disclosure${open ? " open" : ""}`} aria-hidden="true">
                    <ChevronRight />
                  </span>
                </button>
              </h2>
              <button
                type="button"
                className="icon-button"
                title="New Session"
                aria-label="New Session"
                disabled={agent === undefined}
                popoverTarget={menu}
                style={{ anchorName: `--${menu}` }}
              >
                <Plus />
              </button>
              {agent !== undefined && (
                <div
                  id={menu}
                  className="menu new-session-menu"
                  role="menu"
                  popover="auto"
                  style={{ positionAnchor: `--${menu}` }}
                  onToggle={(event) => {
                    if (event.newState === "open" && workspace.git_working_tree) {
                      run(async () => setWorkflows(await invoke<WorkflowInfo[]>("workflows")));
                    }
                  }}
                >
                  {agents.length > 1 && (
                    <div className="menu-field">
                      <HarnessLogo agent={agent} />
                      <SelectPicker
                        label="Harness"
                        value={agent}
                        options={agents.map((id) => ({ value: id, name: agentNames[id] }))}
                        onChange={(id) => setChosen(id as AgentId)}
                      />
                    </div>
                  )}
                  {workspace.git_working_tree && (
                    <button
                      type="button"
                      role="menuitem"
                      popoverTarget={menu}
                      popoverTargetAction="hide"
                      onClick={() => newWorktreeSession(workspace, agent)}
                    >
                      Session in Worktree
                    </button>
                  )}
                  <button
                    type="button"
                    role="menuitem"
                    popoverTarget={menu}
                    popoverTargetAction="hide"
                    onClick={() => newSession(workspace.id, agent, { type: "workspace" })}
                  >
                    Session in Checkout
                  </button>
                  {workspace.git_working_tree && workflows.length > 0 && (
                    <>
                      <hr className="menu-separator" />
                      {workflows.map((workflow) => (
                        <button
                          key={workflow.workflow}
                          type="button"
                          role="menuitem"
                          title={workflow.error ?? undefined}
                          disabled={workflow.error !== null}
                          popoverTarget={menu}
                          popoverTargetAction="hide"
                          onClick={() => setStarting({ workspaceId: workspace.id, workflow })}
                        >
                          {workflow.name}
                        </button>
                      ))}
                    </>
                  )}
                </div>
              )}
            </div>
            {open && <ul>{rows.flatMap((row) => row.rows)}</ul>}
          </section>
        );
      })}
      {starting !== null && (
        <StartRunDialog
          workspaceId={starting.workspaceId}
          workflow={starting.workflow}
          onStarted={onSelect}
          onClose={() => setStarting(null)}
        />
      )}
    </Sidebar>
  );
});

/** Asks for a run's input, then starts the run and views it. */
function StartRunDialog({
  workspaceId,
  workflow,
  onStarted,
  onClose,
}: {
  workspaceId: string;
  workflow: WorkflowInfo;
  onStarted: (runId: string) => void;
  onClose: () => void;
}) {
  const [input, setInput] = useState("");
  const [pending, setPending] = useState(false);

  const start = async () => {
    setPending(true);
    try {
      const started = await invoke<RunInfo>("start_run", {
        workspaceId,
        workflow: workflow.workflow,
        input,
      });
      onClose();
      onStarted(started.id);
    } catch (e) {
      setPending(false);
      await showError(e);
    }
  };

  return (
    <Dialog
      title={workflow.name}
      onClose={onClose}
      actions={
        <>
          <button type="button" className="text-button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="text-button" disabled={pending} onClick={start}>
            Run
          </button>
        </>
      }
    >
      <textarea
        className="text-field"
        aria-label="Input"
        rows={4}
        value={input}
        disabled={pending}
        onChange={(event) => setInput(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && event.metaKey && !pending) {
            event.preventDefault();
            start();
          }
        }}
      />
    </Dialog>
  );
}

/** A run's row, which renders again only when its props change. */
const RunRow = memo(function RunRow({
  run,
  worktree,
  status,
  label,
  selected,
  busy,
  expanded,
  onSelect,
  onToggle,
  cancelRun,
  deleteRun,
}: {
  run: RunInfo;
  worktree: WorktreeInfo | undefined;
  /** The status class and its label, from `runStatus`. */
  status: string;
  label: string;
  selected: boolean;
  /** Whether an action on the run is in flight. */
  busy: boolean;
  /** Whether the run's steps show. */
  expanded: boolean;
  onSelect: (runId: string) => void;
  onToggle: (runId: string) => void;
} & Pick<SessionActions, "cancelRun" | "deleteRun">) {
  const running = run.state === "running";
  const branch =
    worktree?.compare != null && worktree.comparison != null
      ? branchStatus(worktree.compare, worktree.comparison)
      : null;
  return (
    <li
      className="sidebar-row run"
      onContextMenu={(event) =>
        contextMenu(event, [
          { text: "Cancel Run", enabled: !busy && running, action: () => cancelRun(run.id) },
          {
            text: "Delete Run",
            enabled: !busy && !running,
            action: () => deleteRun(run.id, false),
          },
        ])
      }
    >
      <button
        type="button"
        className="select"
        aria-current={selected}
        aria-expanded={expanded}
        onClick={() => onSelect(run.id)}
        onDoubleClick={() => onToggle(run.id)}
      >
        <StatusIcon status={{ className: status, label }} />
        <span className="title-group">
          <span className="title">{runTitle(run)}</span>
          <span
            className={`disclosure${expanded ? " open" : ""}`}
            aria-hidden="true"
            onClick={(event) => {
              event.stopPropagation();
              onToggle(run.id);
            }}
          >
            <ChevronRight />
          </span>
        </span>
        {branch && <BranchIcon status={branch} />}
      </button>
    </li>
  );
});

/**
 * A run's step that has no session yet, or runs no agent: a shell step, or a
 * pending agent step. It follows the run's tree line, and selects the run.
 */
function StepRow({
  step,
  first,
  last,
  onSelect,
}: {
  step: StepInfo;
  first: boolean;
  last: boolean;
  onSelect: () => void;
}) {
  const status = childStatus(step, undefined);
  return (
    <li className={`sidebar-row session child${step.state === "pending" ? " pending" : ""}`}>
      <button type="button" className="select" onClick={onSelect}>
        <span
          className={["tree-branch", first && "first", last && "last"].filter(Boolean).join(" ")}
          aria-hidden="true"
        />
        <StatusIcon status={status} />
        <span className="title">{step.name}</span>
      </button>
    </li>
  );
}

/**
 * A session's row, which renders again only when its props change. A run's
 * child is titled with its step's name, and follows the run's tree line.
 */
const SessionRow = memo(function SessionRow({
  session,
  worktree,
  selected,
  busy,
  step,
  first,
  last,
  onSelect,
  deleteSession,
  archiveSession,
  unarchiveSession,
}: {
  session: SessionInfo;
  worktree: WorktreeInfo | undefined;
  selected: boolean;
  /** Whether an action on the session is in flight. */
  busy: boolean;
  /** The step, when the session is a run's child. */
  step: StepInfo | undefined;
  /** Whether a child is its run's first, or its last. */
  first: boolean;
  last: boolean;
  onSelect: (session: SessionInfo) => void;
} & Pick<SessionActions, "deleteSession" | "archiveSession" | "unarchiveSession">) {
  const child = step !== undefined;
  const status = step
    ? childStatus(step, session)
    : { className: session.state, label: stateLabels[session.state] };
  // A run's children share its worktree, whose branch the run's row shows.
  const merge = child ? null : localMerge(session, worktree);
  const branch = merge && branchStatus(merge.compare, merge.comparison);
  const classes = [
    "sidebar-row",
    "session",
    child && "child",
    session.state === "exited" && "exited",
  ];
  return (
    <li
      className={classes.filter(Boolean).join(" ")}
      onContextMenu={(event) =>
        contextMenu(event, [
          {
            text: session.archived ? "Unarchive Session" : "Archive Session",
            enabled: !busy && session.state !== "starting",
            action: () =>
              session.archived ? unarchiveSession(session.id) : archiveSession(session.id, false),
          },
          {
            text: "Delete Session",
            enabled: !busy,
            action: () => deleteSession(session.id, false),
          },
        ])
      }
    >
      <button
        type="button"
        className="select"
        title={session.cwd}
        aria-current={selected}
        onClick={() => onSelect(session)}
      >
        {child && (
          <span
            className={["tree-branch", first && "first", last && "last"].filter(Boolean).join(" ")}
            aria-hidden="true"
          />
        )}
        <StatusIcon status={status} />
        <span className="title">{step?.name ?? session.title ?? "New session"}</span>
        {branch && <BranchIcon status={branch} />}
      </button>
    </li>
  );
});

/** A row's branch status icon, from `branchStatus`. */
function BranchIcon({ status }: { status: { attention: boolean; tooltip: string } }) {
  return (
    <span
      className={`status branch-status${status.attention ? " attention" : ""}`}
      role="img"
      aria-label={status.tooltip}
      title={status.tooltip}
    >
      <GitBranch />
    </span>
  );
}
