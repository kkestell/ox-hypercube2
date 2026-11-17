import { Channel, invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import type {
  ListingChange,
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
}

const emptyListing: Listing = { workspaces: [], worktrees: new Map(), sessions: [] };

/**
 * `listing` with `changes` applied. Each list is copied once at most, and
 * the entries that did not change keep their identity.
 */
function apply(listing: Listing, changes: ListingChange[]): Listing {
  let { workspaces, worktrees, sessions } = listing;
  // Where each session is in `sessions`, once a change needs it.
  let positions: Map<string, number> | null = null;
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
    }
  }
  return workspaces === listing.workspaces &&
    worktrees === listing.worktrees &&
    sessions === listing.sessions
    ? listing
    : { workspaces, worktrees, sessions };
}

/**
 * The daemon's workspaces, worktrees and sessions, from a watch that restarts,
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

  return {
    workspaces: listing.workspaces,
    worktrees: listing.worktrees,
    sessions: listing.sessions,
    connection,
  };
}
