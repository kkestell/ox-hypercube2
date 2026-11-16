import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { DirEntry } from "../../lib/protocol";

/**
 * The entries of the directory at `path`, followed live, watched again each
 * time the app reconnects to `oxd`.
 */
export function useDirectory(path: string, connection: number): DirEntry[] {
  const [listing, setListing] = useState<{ path: string; entries: DirEntry[] }>({
    path,
    entries: [],
  });

  useEffect(() => {
    // The first watch waits for the first workspace watch to connect.
    if (connection === 0) return;
    let current = true;
    const channel = new Channel<DirEntry[]>();
    channel.onmessage = (entries) => {
      if (current) setListing({ path, entries });
    };
    invoke("watch_directory", { path, channel });
    return () => {
      current = false;
      invoke("unwatch", { channelId: channel.id });
    };
  }, [path, connection]);

  return listing.path === path ? listing.entries : [];
}
