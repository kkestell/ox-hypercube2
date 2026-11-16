import { Channel, invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import type {
  ListingChange,
  RunInfo,
  SessionInfo,
  WatchMessage,
  WorkspaceInfo,
  WorktreeInfo,
} from "../../lib/protocol";

interface Listing {
  workspaces: WorkspaceInfo[];
  /** The worktrees by ID. */
  worktrees: Map<string, WorktreeInfo>;
  sessions: SessionInfo[];
  runs: RunInfo[];
}

const emptyListing: Listing = { workspaces: [], worktrees: new Map(), sessions: [], runs: [] };

/**
 * `listing` with `changes` applied. Each list is copied once at most, and
 * the entries that did not change keep their identity.
 */
function apply(listing: Listing, changes: ListingChange[]): Listing {
  let { workspaces, worktrees, sessions, runs } = listing;
  // Where each session is in `sessions`, and each run in `runs`, once a
  // change needs it.
  let positions: Map<string, number> | null = null;
  let runPositions: Map<string, number> | null = null;
  for (const change of changes) {
    switch (change.type) {
      case "workspace": {
        if (workspaces === listing.workspaces) workspaces = [...workspaces];
        const position = workspaces.findIndex(({ id }) => id === change.workspace.id);
        if (position === -1) workspaces.push(change.workspace);
        else workspaces[position] = change.workspace;
        break;
      }
      case "workspace_removed":
        workspaces = workspaces.filter((workspace) => workspace.id !== change.workspace_id);
        break;
      case "worktree":
        if (worktrees === listing.worktrees) worktrees = new Map(worktrees);
        worktrees.set(change.worktree.id, change.worktree);
        break;
      case "worktree_removed":
        if (worktrees === listing.worktrees) worktrees = new Map(worktrees);
        worktrees.delete(change.worktree_id);
        break;
      case "session": {
        if (sessions === listing.sessions) sessions = [...sessions];
        positions ??= new Map(sessions.map((session, index) => [session.id, index]));
        const position = positions.get(change.session.id);
        if (position === undefined) {
          positions.set(change.session.id, sessions.length);
          sessions.push(change.session);
        } else {
          sessions[position] = change.session;
        }
        break;
      }
      case "session_removed":
        sessions = sessions.filter((session) => session.id !== change.session_id);
        positions = null;
        break;
      case "run": {
        if (runs === listing.runs) runs = [...runs];
        runPositions ??= new Map(runs.map((run, index) => [run.id, index]));
        const position = runPositions.get(change.run.id);
        if (position === undefined) {
          runPositions.set(change.run.id, runs.length);
          runs.push(change.run);
        } else {
          runs[position] = change.run;
        }
        break;
      }
      case "run_removed":
        runs = runs.filter((run) => run.id !== change.run_id);
        runPositions = null;
        break;
    }
  }
  return workspaces === listing.workspaces &&
    worktrees === listing.worktrees &&
    sessions === listing.sessions &&
    runs === listing.runs
    ? listing
    : { workspaces, worktrees, sessions, runs };
}

/**
 * The daemon's workspaces, worktrees, sessions and runs, from a watch that restarts,
 * reconnecting, when the connection drops. `connection` changes each time a
 * watch connects.
 */
export function useWorkspaces() {
  const [listing, setListing] = useState(emptyListing);
  const [attempt, setAttempt] = useState(0);
  const [connection, setConnection] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: each attempt starts a new watch
  useEffect(() => {
    // Only the latest watch's channel is read.
    let current = true;
    let listed = false;
    const channel = new Channel<WatchMessage>();
    channel.onmessage = (message) => {
      if (!current) return;
      if (message.type === "changes") {
        // A watch's first changes list everything, replacing what an earlier
        // watch listed.
        const base = listed ? undefined : emptyListing;
        if (!listed) {
          listed = true;
          setConnection((connection) => connection + 1);
        }
        setListing((listing) => apply(base ?? listing, message.changes));
      } else if (message.type === "error") {
        ask(`Cannot reach oxd: ${message.message}`, {
          kind: "error",
          okLabel: "Retry",
          cancelLabel: "Cancel",
        }).then((retry) => {
          if (retry) setAttempt((attempt) => attempt + 1);
        });
      } else {
        setAttempt((attempt) => attempt + 1);
      }
    };
    invoke("watch", { channel });
    return () => {
      current = false;
    };
  }, [attempt]);

  // A session is a child of the run that owns its worktree.
  const parents = useMemo(() => {
    const runOfWorktree = new Map(listing.runs.map((run) => [run.worktree_id, run]));
    const parents = new Map<string, RunInfo>();
    for (const session of listing.sessions) {
      const run = session.worktree_id === null ? undefined : runOfWorktree.get(session.worktree_id);
      if (run !== undefined) parents.set(session.id, run);
    }
    return parents;
  }, [listing.sessions, listing.runs]);

  return {
    workspaces: listing.workspaces,
    worktrees: listing.worktrees,
    sessions: listing.sessions,
    runs: listing.runs,
    /** The run each child session belongs to, by session ID. */
    parents,
    connection,
  };
}
