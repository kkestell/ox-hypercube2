import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useReducer } from "react";
import type { ConversationUpdate, Item } from "../../lib/protocol";

// The viewed session's items by key, in the order their keys first appeared.
interface State {
  sessionId: string | null;
  items: Map<string, Item>;
}

type Action =
  | { type: "view"; sessionId: string | null }
  | { type: "updates"; updates: ConversationUpdate[] };

// Viewing a session starts it empty. Each batch of updates inserts or
// replaces items by key, appends text chunks, or removes resolved requests.
function reduce(state: State, action: Action): State {
  if (action.type === "view") return { sessionId: action.sessionId, items: new Map() };
  const items = new Map(state.items);
  for (const update of action.updates) {
    if (update.type === "request_resolved") {
      items.delete(`request:${update.request_id}`);
      continue;
    }
    const existing = items.get(update.key);
    items.set(
      update.key,
      existing !== undefined &&
        (update.type === "text" || update.type === "thought") &&
        !update.replace &&
        existing.type === update.type
        ? { ...update, text: existing.text + update.text }
        : update,
    );
  }
  return { ...state, items };
}

const noItems: Item[] = [];

/**
 * The items of a session's conversation, replayed from its first event and
 * followed live, viewed again each time the app reconnects to `oxd`.
 */
export function useConversation(sessionId: string | null, connection: number): Item[] {
  const [state, dispatch] = useReducer(reduce, { sessionId, items: new Map() });

  useEffect(() => {
    // The first view waits for the first watch to connect.
    if (connection === 0) return;
    dispatch({ type: "view", sessionId });
    // Only the latest view's channel is read. Viewing no session ends the
    // previous view, such as one left by a webview reload.
    let current = true;
    const channel = new Channel<ConversationUpdate[]>();
    channel.onmessage = (updates) => {
      if (current) dispatch({ type: "updates", updates });
    };
    invoke("view_session", { sessionId, channel });
    return () => {
      current = false;
    };
  }, [sessionId, connection]);

  // The same items until they change, so that what renders them can skip
  // rendering them again.
  const items = useMemo(() => [...state.items.values()], [state.items]);
  // Until the effect views a newly chosen session, the state holds the
  // previous one's items.
  return state.sessionId === sessionId ? items : noItems;
}
