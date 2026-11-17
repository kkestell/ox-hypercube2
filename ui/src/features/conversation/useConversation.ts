import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useReducer } from "react";
import type { ConversationUpdate, Item } from "../../lib/protocol";

// The viewed session's items by key, in the order their keys first appeared,
// and the same items as a list.
interface State {
  sessionId: string | null;
  items: Map<string, Item>;
  list: Item[];
}

type Action =
  | { type: "view"; sessionId: string | null }
  | { type: "updates"; updates: ConversationUpdate[] };

// Viewing a session starts it empty. Each batch of updates inserts or
// replaces items by key, appends text chunks, or removes items by key.
function reduce(state: State, action: Action): State {
  if (action.type === "view") return { sessionId: action.sessionId, items: new Map(), list: [] };
  const items = new Map(state.items);
  for (const update of action.updates) {
    if (update.type === "removed") {
      items.delete(update.key);
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
  return { ...state, items, list: [...items.values()] };
}

const noItems: Item[] = [];

/**
 * The items of a session's conversation, replayed from its first event and
 * followed live, and viewed again each time the app reconnects to `oxd`. The
 * list stays the same until the conversation changes, so that what renders
 * it can skip rendering it again.
 */
export function useConversation(sessionId: string | null, connection: number): Item[] {
  const [state, dispatch] = useReducer(reduce, { sessionId, items: new Map(), list: [] });

  useEffect(() => {
    dispatch({ type: "view", sessionId });
    // The first view waits for the first watch to connect.
    if (connection === 0 || sessionId === null) return;
    // Only the latest view's channel is read.
    let current = true;
    const channel = new Channel<ConversationUpdate[]>();
    channel.onmessage = (updates) => {
      if (current) dispatch({ type: "updates", updates });
    };
    invoke("view_session", { sessionId, channel });
    return () => {
      current = false;
      invoke("unwatch", { channelId: channel.id });
    };
  }, [sessionId, connection]);

  // Until the effect views a newly chosen session, the state holds the
  // previous one's items.
  return state.sessionId === sessionId ? state.list : noItems;
}
