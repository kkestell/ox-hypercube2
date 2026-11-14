import { useEffect, useState } from "react";
import type { SessionInfo } from "../../lib/protocol";

type Draft = { text: string; attachments: string[] };
const emptyDraft: Draft = { text: "", attachments: [] };

/** Keeps drafts across session switches for the lifetime of the webview. */
export function useDrafts(sessions: SessionInfo[], connected: boolean) {
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  useEffect(() => {
    if (!connected) return;
    setDrafts((current) => {
      const ids = new Set(sessions.map(({ id }) => id));
      const entries = Object.entries(current);
      const kept = entries.filter(([id]) => ids.has(id));
      return kept.length === entries.length ? current : Object.fromEntries(kept);
    });
  }, [sessions, connected]);

  return {
    get: (id: string) => drafts[id] ?? emptyDraft,
    update: (id: string, change: (draft: Draft) => Draft | undefined) =>
      setDrafts((current) => {
        const previous = current[id] ?? emptyDraft;
        const draft = change(previous);
        if (draft === previous) return current;
        if (draft && (draft.text !== "" || draft.attachments.length > 0)) {
          return { ...current, [id]: draft };
        }
        if (!(id in current)) return current;
        const next = { ...current };
        delete next[id];
        return next;
      }),
  };
}

export type Drafts = ReturnType<typeof useDrafts>;
