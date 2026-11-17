import { useCallback, useEffect, useMemo, useState } from "react";

/** A message being written: its text and the paths of its attachments. */
export type Message = { text: string; attachments: string[] };

/**
 * A composer's message, not yet sent. While a queued prompt is being edited,
 * it is that prompt, and `editing.held` is the message it replaced until the
 * edit is saved.
 */
export type Unsent = Message & { editing?: { promptId: string; held: Message } };
const emptyUnsent: Unsent = { text: "", attachments: [] };

const key = "unsent";

function load(): Record<string, Unsent> {
  const saved = localStorage.getItem(key);
  return saved === null ? {} : JSON.parse(saved);
}

/**
 * Keeps each session's and each draft's unsent text in local storage, so it
 * survives switching views and restarting the app. Once connected, entries of
 * IDs not in `ids` are dropped. An entry with no content is dropped unless it
 * is an edit. The result changes only when an entry does.
 */
export function useUnsent(ids: ReadonlySet<string>, connected: boolean) {
  const [unsent, setUnsent] = useState(load);
  useEffect(() => {
    localStorage.setItem(key, JSON.stringify(unsent));
  }, [unsent]);
  useEffect(() => {
    if (!connected) return;
    setUnsent((current) => {
      const entries = Object.entries(current);
      const kept = entries.filter(([id]) => ids.has(id));
      return kept.length === entries.length ? current : Object.fromEntries(kept);
    });
  }, [ids, connected]);

  const update = useCallback(
    (id: string, change: (unsent: Unsent) => Unsent | undefined) =>
      setUnsent((current) => {
        const previous = current[id] ?? emptyUnsent;
        const next = change(previous);
        if (next === previous) return current;
        if (next && (next.text !== "" || next.attachments.length > 0 || next.editing)) {
          return { ...current, [id]: next };
        }
        if (!(id in current)) return current;
        const rest = { ...current };
        delete rest[id];
        return rest;
      }),
    [],
  );

  return useMemo(
    () => ({ get: (id: string) => unsent[id] ?? emptyUnsent, update }),
    [unsent, update],
  );
}

export type Unsents = ReturnType<typeof useUnsent>;
