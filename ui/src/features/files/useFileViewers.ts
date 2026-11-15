import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import type { FileContent, SessionInfo } from "../../lib/protocol";
import {
  forgetDeletedViewers,
  openViewers,
  subscribeOpenViewers,
  updateViewer,
  viewerState,
} from "./viewerState";

/** Keep inactive viewers live too, so session selection can use their snapshots before paint. */
export function useFileViewers(sessions: SessionInfo[], connection: number) {
  const currentSessions = useRef(sessions);
  currentSessions.current = sessions;
  useEffect(() => {
    if (connection > 0) forgetDeletedViewers(sessions);
  }, [sessions, connection]);

  useEffect(() => {
    if (!connection) return;
    const subscriptions = new Map<string, { path: string; channel: Channel<FileContent> }>();
    const stop = (id: string) => {
      const subscription = subscriptions.get(id)!;
      subscriptions.delete(id);
      invoke("unwatch_file", { channelId: subscription.channel.id });
    };
    const reconcile = () => {
      const open = new Map(openViewers());
      for (const [id, subscription] of subscriptions) {
        if (open.get(id)?.path !== subscription.path) stop(id);
      }
      for (const [id, state] of open) {
        if (subscriptions.has(id)) continue;
        const session = currentSessions.current.find((session) => session.id === id);
        if (!session || !state.path) continue;
        const path = state.path;
        const channel = new Channel<FileContent>();
        subscriptions.set(id, { path, channel });
        channel.onmessage = (content) => {
          if (subscriptions.get(id)?.channel === channel && viewerState(id).path === path) {
            updateViewer(id, { content });
          }
        };
        invoke("watch_file", { path: `${session.cwd}/${path}`, channel });
      }
    };
    const unsubscribe = subscribeOpenViewers(reconcile);
    reconcile();
    return () => {
      unsubscribe();
      for (const id of subscriptions.keys()) stop(id);
    };
  }, [connection]);
}
