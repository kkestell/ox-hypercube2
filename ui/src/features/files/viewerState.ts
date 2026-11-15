import type { FileContent } from "../../lib/protocol";

export interface ViewerState {
  path: string | null;
  ratio: number;
  scrollTop: number;
  scrollLeft: number;
  content: FileContent | null;
}

const empty: ViewerState = { path: null, ratio: 0.5, scrollTop: 0, scrollLeft: 0, content: null };
const states = new Map<string, ViewerState>();
const listeners = new Map<string, Set<() => void>>();
const watchers = new Set<() => void>();

export function viewerState(id: string): ViewerState {
  return states.get(id) ?? empty;
}

export function subscribeViewer(id: string, listener: () => void) {
  const group = listeners.get(id) ?? new Set();
  listeners.set(id, group);
  group.add(listener);
  return () => {
    group.delete(listener);
    if (!group.size) listeners.delete(id);
  };
}

export function updateViewer(id: string, patch: Partial<ViewerState>) {
  states.set(id, { ...viewerState(id), ...patch });
  for (const listener of listeners.get(id) ?? []) listener();
}

export function openFile(id: string, path: string | null) {
  if (viewerState(id).path === path) return;
  updateViewer(id, { path, content: null, scrollLeft: 0, scrollTop: 0 });
  for (const watcher of watchers) watcher();
}

export function forgetDeletedViewers(sessions: readonly { id: string }[]) {
  const ids = new Set(sessions.map((session) => session.id));
  for (const id of states.keys()) if (!ids.has(id)) states.delete(id);
  for (const watcher of watchers) watcher();
}

export function openViewers() {
  return [...states].filter(([, state]) => state.path !== null);
}

export function subscribeOpenViewers(listener: () => void) {
  watchers.add(listener);
  return () => {
    watchers.delete(listener);
  };
}
