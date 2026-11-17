import { useCallback, useEffect, useMemo, useState } from "react";
import type { WorkspaceInfo } from "../../lib/protocol";
import type { AgentId } from "./useAgents";

/**
 * A new session not yet sent: the workspace, agent and checkout it starts
 * in, and the option values chosen in it, by option ID.
 */
export interface Draft {
  id: string;
  workspaceId: string;
  agent: AgentId;
  checkout: "workspace" | "worktree";
  config: Record<string, string | boolean>;
}

const key = "drafts";

function load(): Draft[] {
  const saved = localStorage.getItem(key);
  return saved === null ? [] : JSON.parse(saved);
}

/**
 * The drafts, newest first, kept in local storage. Once connected, the drafts
 * of workspaces that are not listed are dropped.
 */
export function useDrafts(workspaces: WorkspaceInfo[], connected: boolean) {
  const [drafts, setDrafts] = useState(load);
  useEffect(() => {
    localStorage.setItem(key, JSON.stringify(drafts));
  }, [drafts]);
  useEffect(() => {
    if (!connected) return;
    const listed = new Set(workspaces.map((workspace) => workspace.id));
    setDrafts((current) => keep(current, (draft) => listed.has(draft.workspaceId)));
  }, [workspaces, connected]);

  /** Adds a draft and returns its ID. */
  const create = useCallback((workspaceId: string, agent: AgentId, checkout: Draft["checkout"]) => {
    const id = `draft-${crypto.randomUUID()}`;
    setDrafts((current) => [{ id, workspaceId, agent, checkout, config: {} }, ...current]);
    return id;
  }, []);

  const remove = useCallback(
    (id: string) => setDrafts((current) => keep(current, (draft) => draft.id !== id)),
    [],
  );

  const setConfig = useCallback(
    (id: string, configId: string, value: string | boolean) =>
      setDrafts((current) =>
        current.map((draft) =>
          draft.id === id ? { ...draft, config: { ...draft.config, [configId]: value } } : draft,
        ),
      ),
    [],
  );

  /** Drops every draft other than `viewed` for which `empty` holds. */
  const dropEmpty = useCallback(
    (viewed: string | null, empty: (id: string) => boolean) =>
      setDrafts((current) => keep(current, (draft) => draft.id === viewed || !empty(draft.id))),
    [],
  );

  return useMemo(
    () => ({ list: drafts, create, remove, setConfig, dropEmpty }),
    [drafts, create, remove, setConfig, dropEmpty],
  );
}

export type Drafts = ReturnType<typeof useDrafts>;

/** `drafts` filtered by `kept`, the same array when it keeps them all. */
function keep(drafts: Draft[], kept: (draft: Draft) => boolean) {
  const next = drafts.filter(kept);
  return next.length === drafts.length ? drafts : next;
}

/** A draft's title: its text with runs of whitespace collapsed. */
export function draftTitle(text: string) {
  return text.replace(/\s+/g, " ").trim() || "New session";
}
