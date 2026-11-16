import { invoke } from "@tauri-apps/api/core";
import { Menu } from "@tauri-apps/api/menu";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, FolderPlus, GitBranch, Plus } from "lucide-react";
import { type MouseEvent, memo, useCallback, useState } from "react";
import { Dialog } from "../../components/Dialog";
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

/** A run's or a step's dot class and its label. */
interface Status {
  className: string;
  label: string;
}

const settled: Record<Exclude<StepState, "running">, Status> = {
  pending: { className: "", label: "Pending" },
  succeeded: { className: "succeeded", label: "Succeeded" },
  failed: { className: "failed", label: "Failed" },
  cancelled: { className: "", label: "Cancelled" },
  interrupted: { className: "", label: "Interrupted" },
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
  items: { text: string; enabled: boolean; action: () => void }[],
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

/** A workspace's run or top-level session, newest first by creation time. */
type Entry = { run: RunInfo; session?: undefined } | { run?: undefined; session: SessionInfo };

export const SessionList = memo(function SessionList({
  workspaces,
  worktrees,
  sessions,
  runs,
  selected,
  onSelect,
  actions: { busy, deleteSession, archiveSession, unarchiveSession, cancelRun, deleteRun },
}: {
  workspaces: WorkspaceInfo[];
  worktrees: Map<string, WorktreeInfo>;
  sessions: SessionInfo[];
  runs: RunInfo[];
  selected: string | null;
  /** Views the session or run with this ID. */
  onSelect: (id: string) => void;
  actions: SessionActions;
}) {
  const agents = useAgents();
  // The agent the user chose, and the agent the New Task menus start: the
  // chosen one while it is installed, otherwise the first installed agent.
  const [chosen, setChosen] = useState<AgentId | null>(null);
  const agent = chosen !== null && agents.includes(chosen) ? chosen : agents[0];
  // The workspaces whose Archived groups are open.
  const [openArchives, setOpenArchives] = useState<ReadonlySet<string>>(new Set());
  // The definitions the New Task menus list, read each time one opens.
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
  const newTask = (workspace: WorkspaceInfo, agent: AgentId) =>
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

  const toggleArchive = (workspaceId: string) =>
    setOpenArchives((open) => {
      const next = new Set(open);
      if (!next.delete(workspaceId)) next.add(workspaceId);
      return next;
    });

  // A session is a child of the run that owns its worktree.
  const runOfWorktree = new Map(runs.map((run) => [run.worktree_id, run]));
  const children = new Map<string, SessionInfo[]>();
  for (const session of sessions) {
    const parent =
      session.worktree_id === null ? undefined : runOfWorktree.get(session.worktree_id);
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
    return [
      <RunRow
        key={run.id}
        run={run}
        status={status.className}
        label={status.label}
        selected={run.id === selected}
        busy={busy.has(run.id)}
        onSelect={onSelect}
        cancelRun={cancelRun}
        deleteRun={deleteRun}
      />,
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
        const topLevel = workspaceSessions.filter(
          (session) => session.worktree_id === null || !runOfWorktree.has(session.worktree_id),
        );
        const entries: Entry[] = [
          ...workspaceRuns.map((run) => ({ run })),
          ...topLevel.filter((session) => !session.archived).map((session) => ({ session })),
        ];
        entries.sort((a, b) => (b.run ?? b.session).created_at - (a.run ?? a.session).created_at);
        const archived = topLevel.filter((session) => session.archived);
        const archiveOpen = openArchives.has(workspace.id);
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
                      text: "Remove Workspace",
                      enabled: workspaceSessions.length === 0 && workspaceRuns.length === 0,
                      action: () => removeWorkspace(workspace.id),
                    },
                  ])
                }
              >
                {directoryName(workspace.path)}
              </h2>
              <button
                type="button"
                className="icon-button"
                title="New Task"
                aria-label="New Task"
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
                      onClick={() => newTask(workspace, agent)}
                    >
                      New Task
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
            <ul>
              {entries.map((entry) => (entry.run ? runRows(entry.run) : sessionRow(entry.session)))}
              {archived.length > 0 && (
                <li className="sidebar-row archived-toggle">
                  <button
                    type="button"
                    className="select"
                    aria-expanded={archiveOpen}
                    onClick={() => toggleArchive(workspace.id)}
                  >
                    <span className="status">
                      {archiveOpen ? <ChevronDown /> : <ChevronRight />}
                    </span>
                    <span className="title">Archived ({archived.length})</span>
                  </button>
                </li>
              )}
              {archiveOpen && archived.map((session) => sessionRow(session))}
            </ul>
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
  status,
  label,
  selected,
  busy,
  onSelect,
  cancelRun,
  deleteRun,
}: {
  run: RunInfo;
  /** The dot's class and its label, from `runStatus`. */
  status: string;
  label: string;
  selected: boolean;
  /** Whether an action on the run is in flight. */
  busy: boolean;
  onSelect: (runId: string) => void;
} & Pick<SessionActions, "cancelRun" | "deleteRun">) {
  const running = run.state === "running";
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
        onClick={() => onSelect(run.id)}
      >
        <span className={`status ${status}`} role="img" aria-label={label} title={label} />
        <span className="title">{runTitle(run)}</span>
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
    <li className="sidebar-row session child">
      <button type="button" className="select" onClick={onSelect}>
        <span
          className={["tree-branch", first && "first", last && "last"].filter(Boolean).join(" ")}
          aria-hidden="true"
        />
        <span
          className={`status ${status.className}`}
          role="img"
          aria-label={status.label}
          title={status.label}
        />
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
  const merge = localMerge(session, worktree);
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
        <span
          className={`status ${status.className}`}
          role="img"
          aria-label={status.label}
          title={status.label}
        />
        <span className="title">{step?.name ?? session.title ?? "New session"}</span>
        {branch && (
          <span
            className={`status branch-status${branch.attention ? " attention" : ""}`}
            role="img"
            aria-label={branch.tooltip}
            title={branch.tooltip}
          >
            <GitBranch />
          </span>
        )}
      </button>
    </li>
  );
});
