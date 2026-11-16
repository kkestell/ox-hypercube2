import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";
import type { FileContent } from "../../lib/protocol";
import {
  forgetDeletedViewers,
  openViewers,
  subscribeOpenViewers,
  updateViewer,
  viewerState,
} from "./viewerState";

/**
 * Keeps the viewer of each session and each run live, each `id` with its
 * files under `cwd`, so selection can use an inactive viewer's snapshot
 * before paint.
 */
export function useFileViewers(views: readonly { id: string; cwd: string }[], connection: number) {
  const currentViews = useRef(views);
  currentViews.current = views;
  useEffect(() => {
    if (connection > 0) forgetDeletedViewers(views);
  }, [views, connection]);

  useEffect(() => {
    if (!connection) return;
    const subscriptions = new Map<string, { path: string; channel: Channel<FileContent> }>();
    const stop = (id: string) => {
      const subscription = subscriptions.get(id)!;
      subscriptions.delete(id);
      invoke("unwatch", { channelId: subscription.channel.id });
    };
    const reconcile = () => {
      const open = new Map(openViewers());
      for (const [id, subscription] of subscriptions) {
        if (open.get(id)?.path !== subscription.path) stop(id);
      }
      for (const [id, state] of open) {
        if (subscriptions.has(id)) continue;
        const view = currentViews.current.find((view) => view.id === id);
        if (!view || !state.path) continue;
        const path = state.path;
        const channel = new Channel<FileContent>();
        subscriptions.set(id, { path, channel });
        channel.onmessage = (content) => {
          if (subscriptions.get(id)?.channel === channel && viewerState(id).path === path) {
            updateViewer(id, { content });
          }
        };
        invoke("watch_file", { path: `${view.cwd}/${path}`, channel });
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
