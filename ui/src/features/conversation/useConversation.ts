import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useReducer } from "react";
import type { ConversationUpdate, Item } from "../../lib/protocol";

// The viewed session's items by key, in the order their keys first appeared.
interface State {
  sessionId: string | null;
  items: Map<string, Item>;
}

type Action =
  | { type: "view"; sessionId: string | null }
  | { type: "update"; update: ConversationUpdate };

// Viewing a session starts it empty. Updates insert or replace items by key,
// append text chunks, or remove resolved requests.
function reduce(state: State, action: Action): State {
  if (action.type === "view") return { sessionId: action.sessionId, items: new Map() };
  const { update } = action;
  const items = new Map(state.items);
  if (update.type === "request_resolved") {
    return items.delete(`request:${update.request_id}`) ? { ...state, items } : state;
  }
  const existing = items.get(update.key);
  const next =
    existing !== undefined &&
    (update.type === "text" || update.type === "thought") &&
    !update.replace &&
    existing.type === update.type
      ? { ...update, text: existing.text + update.text }
      : update;
  return { ...state, items: items.set(update.key, next) };
}

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
    const channel = new Channel<ConversationUpdate>();
    channel.onmessage = (update) => {
      if (current) dispatch({ type: "update", update });
    };
    invoke("view_session", { sessionId, channel });
    return () => {
      current = false;
    };
  }, [sessionId, connection]);

  // Until the effect views a newly chosen session, the state holds the
  // previous one's items.
  return state.sessionId === sessionId ? [...state.items.values()] : [];
}
