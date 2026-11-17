import { invoke } from "@tauri-apps/api/core";
import { Menu } from "@tauri-apps/api/menu";
import { open } from "@tauri-apps/plugin-dialog";
import { ChevronRight, FolderPlus, GitBranch, Plus } from "lucide-react";
import { type MouseEvent, memo, useCallback, useState } from "react";
import { HarnessLogo } from "../../components/HarnessLogo";
import { SelectPicker } from "../../components/SelectPicker";
import { Sidebar } from "../../components/Sidebar";
import type { SessionInfo, SessionState, WorkspaceInfo, WorktreeInfo } from "../../lib/protocol";
import { showError } from "../../lib/showError";
import { branchStatus, localMerge } from "./localMerge";
import { type AgentId, agentNames } from "./useAgents";
import type { Draft } from "./useDrafts";
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

/** Whether the session's agent is starting or in a turn. */
function working(session: SessionInfo) {
  return session.state === "starting" || session.state === "running";
}

/**
 * A dot's colour: blue working, orange blocked, red error, green finished,
 * or gray, with no class.
 */
type Dot = "running" | "attention" | "error" | "finished" | null;

/** A session's dot and its label. */
interface Status {
  className: Dot;
  label: string;
}

/** A status's dot. */
function StatusIcon({ status }: { status: Status }) {
  return (
    <span
      className={status.className === null ? "status" : `status ${status.className}`}
      role="img"
      aria-label={status.label}
      title={status.label}
    />
  );
}

/**
 * What a session needs from the user: an answer, a look at an unseen error,
 * nothing while it works, or a look at an unseen finish.
 */
function sessionStatus(session: SessionInfo): Status {
  const label = stateLabels[session.state];
  if (awaiting(session)) return { className: "attention", label };
  if (session.unseen === "error") return { className: "error", label: "Error" };
  if (working(session)) return { className: "running", label };
  if (session.unseen === "finished") return { className: "finished", label: "Finished" };
  return { className: null, label };
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
export function directoryName(path: string) {
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
  agents,
  workspaces,
  worktrees,
  sessions,
  drafts,
  draftTitles,
  selected,
  onSelect,
  onNewDraft,
  onDeleteDraft,
  actions: { busy, deleteSession, archiveSession, unarchiveSession },
}: {
  /** The installed agents. */
  agents: AgentId[];
  workspaces: WorkspaceInfo[];
  worktrees: Map<string, WorktreeInfo>;
  sessions: SessionInfo[];
  /** The drafts, newest first. */
  drafts: Draft[];
  /** Each draft's title, by draft ID. */
  draftTitles: Map<string, string>;
  selected: string | null;
  /** Views the session or draft with this ID. */
  onSelect: (id: string) => void;
  /** Adds a draft and views it. */
  onNewDraft: (workspaceId: string, agent: AgentId, checkout: Draft["checkout"]) => void;
  onDeleteDraft: (id: string) => void;
  actions: SessionActions;
}) {
  // The agent the user chose, and the agent the New Session menus start: the
  // chosen one while it is installed, otherwise the first installed agent.
  const [chosen, setChosen] = useState<AgentId | null>(null);
  const agent = chosen !== null && agents.includes(chosen) ? chosen : agents[0];
  // The workspaces that list their archived sessions.
  const [showArchived, setShowArchived] = useState<ReadonlySet<string>>(new Set());
  // The workspaces whose rows are collapsed.
  const [collapsedWorkspaces, setCollapsedWorkspaces] = useState<ReadonlySet<string>>(new Set());

  const addWorkspace = async () => {
    const path = await open({ directory: true, title: "Add Workspace" });
    if (path === null) return;
    await run(() => invoke("add_workspace", { path }));
  };

  const removeWorkspace = (workspaceId: string) =>
    run(() => invoke("remove_workspace", { workspaceId }));

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
        const archivedShown = showArchived.has(workspace.id);
        const draftRows = drafts
          .filter((draft) => draft.workspaceId === workspace.id)
          .map((draft) => (
            <DraftRow
              key={draft.id}
              id={draft.id}
              title={draftTitles.get(draft.id) ?? ""}
              selected={draft.id === selected}
              onSelect={onSelect}
              onDelete={onDeleteDraft}
            />
          ));
        const sessionRows = workspaceSessions
          .filter((session) => archivedShown || !session.archived)
          .sort(newestFirst)
          .map((session) => (
            <SessionRow
              key={session.id}
              session={session}
              worktree={
                session.worktree_id === null ? undefined : worktrees.get(session.worktree_id)
              }
              selected={session.id === selected}
              busy={busy.has(session.id)}
              onSelect={select}
              deleteSession={deleteSession}
              archiveSession={archiveSession}
              unarchiveSession={unarchiveSession}
            />
          ));
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
                      enabled: workspaceSessions.length === 0,
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
                      onClick={() => onNewDraft(workspace.id, agent, "worktree")}
                    >
                      Session in Worktree
                    </button>
                  )}
                  <button
                    type="button"
                    role="menuitem"
                    popoverTarget={menu}
                    popoverTargetAction="hide"
                    onClick={() => onNewDraft(workspace.id, agent, "workspace")}
                  >
                    Session in Checkout
                  </button>
                </div>
              )}
            </div>
            {open && (
              <ul>
                {draftRows}
                {sessionRows}
              </ul>
            )}
          </section>
        );
      })}
    </Sidebar>
  );
});

const draftStatus: Status = { className: null, label: "Draft" };

/** A draft's row, which renders again only when its props change. */
const DraftRow = memo(function DraftRow({
  id,
  title,
  selected,
  onSelect,
  onDelete,
}: {
  id: string;
  title: string;
  selected: boolean;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  return (
    <li
      className="sidebar-row session"
      onContextMenu={(event) =>
        contextMenu(event, [{ text: "Delete Draft", enabled: true, action: () => onDelete(id) }])
      }
    >
      <button type="button" className="select" aria-current={selected} onClick={() => onSelect(id)}>
        <StatusIcon status={draftStatus} />
        <span className="title">{title}</span>
      </button>
    </li>
  );
});

/** A session's row, which renders again only when its props change. */
const SessionRow = memo(function SessionRow({
  session,
  worktree,
  selected,
  busy,
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
  onSelect: (session: SessionInfo) => void;
} & Pick<SessionActions, "deleteSession" | "archiveSession" | "unarchiveSession">) {
  const merge = localMerge(session, worktree);
  const branch = merge && branchStatus(merge.compare, merge.comparison);
  return (
    <li
      className={`sidebar-row session${session.state === "exited" ? " exited" : ""}`}
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
        <StatusIcon status={sessionStatus(session)} />
        <span className="title">{session.title ?? "New session"}</span>
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
