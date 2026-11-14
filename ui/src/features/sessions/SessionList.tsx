import { invoke } from "@tauri-apps/api/core";
import { Menu } from "@tauri-apps/api/menu";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, FolderPlus, GitBranch, Plus } from "lucide-react";
import { type MouseEvent, memo, useCallback, useState } from "react";
import { SelectPicker } from "../../components/SelectPicker";
import { Sidebar } from "../../components/Sidebar";
import type {
  Branches,
  Checkout,
  SessionInfo,
  SessionState,
  WorkspaceInfo,
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

export const SessionList = memo(function SessionList({
  workspaces,
  sessions,
  selected,
  onSelect,
  actions: { busy, deleteSession, archiveSession, unarchiveSession },
}: {
  workspaces: WorkspaceInfo[];
  sessions: SessionInfo[];
  selected: string | null;
  onSelect: (sessionId: string) => void;
  actions: SessionActions;
}) {
  const agents = useAgents();
  // The agent the user chose, and the agent the New Task menus start: the
  // chosen one while it is installed, otherwise the first installed agent.
  const [chosen, setChosen] = useState<AgentId | null>(null);
  const agent = chosen !== null && agents.includes(chosen) ? chosen : agents[0];
  // The workspaces whose Archived groups are open.
  const [openArchives, setOpenArchives] = useState<ReadonlySet<string>>(new Set());

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

  // A worktree session on a new `ox/<id8>` branch from `HEAD`, compared with
  // the branch the workspace's checkout is on.
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

  const sessionRow = (session: SessionInfo) => (
    <SessionRow
      key={session.id}
      session={session}
      selected={session.id === selected}
      busy={busy.has(session.id)}
      onSelect={select}
      deleteSession={deleteSession}
      archiveSession={archiveSession}
      unarchiveSession={unarchiveSession}
    />
  );

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
        const active = workspaceSessions.filter((session) => !session.archived);
        const archived = workspaceSessions.filter((session) => session.archived);
        const archiveOpen = openArchives.has(workspace.id);
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
                      enabled: workspaceSessions.length === 0,
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
                popoverTarget={`new-session-${workspace.id}`}
                style={{ anchorName: `--new-session-${workspace.id}` }}
              >
                <Plus />
              </button>
              {agent !== undefined && (
                <div
                  id={`new-session-${workspace.id}`}
                  className="menu new-session-menu"
                  role="menu"
                  popover="auto"
                  style={{ positionAnchor: `--new-session-${workspace.id}` }}
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
                      popoverTarget={`new-session-${workspace.id}`}
                      popoverTargetAction="hide"
                      onClick={() => newTask(workspace, agent)}
                    >
                      New Task
                    </button>
                  )}
                  <button
                    type="button"
                    role="menuitem"
                    popoverTarget={`new-session-${workspace.id}`}
                    popoverTargetAction="hide"
                    onClick={() => newSession(workspace.id, agent, { type: "workspace" })}
                  >
                    Session in Checkout
                  </button>
                </div>
              )}
            </div>
            <ul>
              {active.map(sessionRow)}
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
              {archiveOpen && archived.map(sessionRow)}
            </ul>
          </section>
        );
      })}
    </Sidebar>
  );
});

/** A session's row, which renders again only when its props change. */
const SessionRow = memo(function SessionRow({
  session,
  selected,
  busy,
  onSelect,
  deleteSession,
  archiveSession,
  unarchiveSession,
}: {
  session: SessionInfo;
  selected: boolean;
  /** Whether an action on the session is in flight. */
  busy: boolean;
  onSelect: (session: SessionInfo) => void;
} & Omit<SessionActions, "busy">) {
  const state = stateLabels[session.state];
  const merge = localMerge(session);
  const status = merge && branchStatus(merge.compare, merge.comparison);
  return (
    <li
      className="sidebar-row session"
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
        <span className={`status ${session.state}`} role="img" aria-label={state} title={state} />
        <span className="title">{session.title ?? "New session"}</span>
        {status && (
          <span
            className={`status branch-status${status.attention ? " attention" : ""}`}
            role="img"
            aria-label={status.tooltip}
            title={status.tooltip}
          >
            <GitBranch />
          </span>
        )}
      </button>
    </li>
  );
});
