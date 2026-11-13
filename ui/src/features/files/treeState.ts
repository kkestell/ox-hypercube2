import type { SessionInfo } from "../../lib/protocol";

/**
 * A session's file tree as the user left it: its open folders, by path
 * relative to the tree's root, and how far its body was scrolled.
 */
export interface TreeState {
  open: string[];
  scrollTop: number;
}

const prefix = "fileTree:";

export function loadTreeState(sessionId: string): TreeState {
  const saved = localStorage.getItem(prefix + sessionId);
  return saved === null ? { open: [], scrollTop: 0 } : JSON.parse(saved);
}

export function saveTreeState(sessionId: string, state: TreeState) {
  localStorage.setItem(prefix + sessionId, JSON.stringify(state));
}

/** Removes the saved tree of every session not in `sessions`. */
export function forgetDeletedTrees(sessions: SessionInfo[]) {
  const ids = new Set(sessions.map((session) => session.id));
  const keys = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index));
  for (const key of keys) {
    if (key?.startsWith(prefix) && !ids.has(key.slice(prefix.length))) {
      localStorage.removeItem(key);
    }
  }
}
