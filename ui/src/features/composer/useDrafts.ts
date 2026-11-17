import { useCallback, useEffect, useMemo, useState } from "react";
import type { SessionInfo } from "../../lib/protocol";

/** A message being written: its text and the paths of its attachments. */
export type Message = { text: string; attachments: string[] };

/**
 * A session's draft. While a queued prompt is being edited, the draft is that
 * prompt, and `editing.held` is the draft it replaced until the edit is saved.
 */
export type Draft = Message & { editing?: { promptId: string; held: Message } };

const emptyDraft: Draft = { text: "", attachments: [] };

/**
 * Keeps drafts across session switches for the lifetime of the webview. A
 * draft with no content is dropped unless it is an edit. The result changes
 * only when a draft does.
 */
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

  const update = useCallback(
    (id: string, change: (draft: Draft) => Draft | undefined) =>
      setDrafts((current) => {
        const previous = current[id] ?? emptyDraft;
        const draft = change(previous);
        if (draft === previous) return current;
        if (draft && (draft.text !== "" || draft.attachments.length > 0 || draft.editing)) {
          return { ...current, [id]: draft };
        }
        if (!(id in current)) return current;
        const next = { ...current };
        delete next[id];
        return next;
      }),
    [],
  );

  return useMemo(
    () => ({ get: (id: string) => drafts[id] ?? emptyDraft, update }),
    [drafts, update],
  );
}

export type Drafts = ReturnType<typeof useDrafts>;
