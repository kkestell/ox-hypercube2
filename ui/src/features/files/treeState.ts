/**
 * A view's file tree as the user left it: its open folders, by path
 * relative to the tree's root, and how far its body was scrolled.
 */
export interface TreeState {
  open: string[];
  scrollTop: number;
}

const prefix = "fileTree:";

export function loadTreeState(viewId: string): TreeState {
  const saved = localStorage.getItem(prefix + viewId);
  return saved === null ? { open: [], scrollTop: 0 } : JSON.parse(saved);
}

export function saveTreeState(viewId: string, state: TreeState) {
  localStorage.setItem(prefix + viewId, JSON.stringify(state));
}

/** Removes the saved tree of every session or draft not in `views`. */
export function forgetDeletedTrees(views: readonly { id: string }[]) {
  const ids = new Set(views.map((view) => view.id));
  const keys = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index));
  for (const key of keys) {
    if (key?.startsWith(prefix) && !ids.has(key.slice(prefix.length))) {
      localStorage.removeItem(key);
    }
  }
}
