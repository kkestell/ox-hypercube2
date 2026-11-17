import { useCallback, useEffect, useMemo, useState } from "react";

/** A composer's text and attached files, not yet sent. */
export type Unsent = { text: string; attachments: string[] };
const emptyUnsent: Unsent = { text: "", attachments: [] };

const key = "unsent";

function load(): Record<string, Unsent> {
  const saved = localStorage.getItem(key);
  return saved === null ? {} : JSON.parse(saved);
}

/**
 * Keeps each session's and each draft's unsent text in local storage, so it
 * survives switching views and restarting the app. Once connected, entries of
 * IDs not in `ids` are dropped. The result changes only when an
 * entry does.
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
        if (next && (next.text !== "" || next.attachments.length > 0)) {
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
