import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useReducer, useRef } from "react";
import type { ConversationUpdate, Item } from "../../lib/protocol";

// A viewed session's items by key, in the order their keys first appeared,
// and the same items as a list.
interface Conversation {
  items: Map<string, Item>;
  list: Item[];
}

type Action =
  | { type: "view"; sessionId: string }
  | { type: "forget"; sessionId: string }
  | { type: "updates"; sessionId: string; updates: ConversationUpdate[] };

const empty: Conversation = { items: new Map(), list: [] };

// Viewing a session starts it empty. Each batch of updates inserts or
// replaces items by key, appends text chunks, or removes resolved requests,
// in that session's conversation only.
function reduce(state: Map<string, Conversation>, action: Action): Map<string, Conversation> {
  const next = new Map(state);
  if (action.type === "view") {
    next.set(action.sessionId, empty);
    return next;
  }
  if (action.type === "forget") {
    next.delete(action.sessionId);
    return next;
  }
  const current = state.get(action.sessionId);
  if (current === undefined) return state;
  const items = new Map(current.items);
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
  next.set(action.sessionId, { items, list: [...items.values()] });
  return next;
}

/**
 * The items of each session's conversation, by session ID, each replayed
 * from its first event and followed live on a view of its own, and viewed
 * again each time the app reconnects to `oxd`. A session's view ends when
 * its ID leaves `sessionIds`. A batch for one session leaves the other
 * sessions' lists as they were.
 */
export function useConversations(
  sessionIds: readonly string[],
  connection: number,
): Map<string, Item[]> {
  const [state, dispatch] = useReducer(reduce, new Map<string, Conversation>());
  // Each viewed session's channel, and the connection they were viewed on.
  const views = useRef(new Map<string, Channel<ConversationUpdate[]>>());
  const viewedOn = useRef(0);
  const key = sessionIds.join("\n");

  useEffect(() => {
    // The first views wait for the first watch to connect.
    if (connection === 0) return;
    const current = views.current;
    // A new watch ended every view the app had.
    if (viewedOn.current !== connection) {
      current.clear();
      viewedOn.current = connection;
    }
    const wanted = new Set(key === "" ? [] : key.split("\n"));
    for (const [sessionId, channel] of current) {
      if (wanted.has(sessionId)) continue;
      current.delete(sessionId);
      invoke("unview_session", { channelId: channel.id });
      dispatch({ type: "forget", sessionId });
    }
    for (const sessionId of wanted) {
      if (current.has(sessionId)) continue;
      const channel = new Channel<ConversationUpdate[]>();
      current.set(sessionId, channel);
      dispatch({ type: "view", sessionId });
      // Only the session's latest view is read.
      channel.onmessage = (updates) => {
        if (views.current.get(sessionId) === channel) {
          dispatch({ type: "updates", sessionId, updates });
        }
      };
      invoke("view_session", { sessionId, channel });
    }
  }, [key, connection]);

  useEffect(
    () => () => {
      for (const channel of views.current.values()) {
        invoke("unview_session", { channelId: channel.id });
      }
      views.current.clear();
    },
    [],
  );

  // The same map until a conversation changes, and the same list for each
  // conversation that did not, so that what renders them can skip rendering
  // them again.
  return useMemo(
    () => new Map([...state].map(([sessionId, conversation]) => [sessionId, conversation.list])),
    [state],
  );
}
