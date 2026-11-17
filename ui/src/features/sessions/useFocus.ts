import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

/** Whether the window is in front, following its `focus` and `blur` events. */
function useForeground() {
  const [foreground, setForeground] = useState(() => document.hasFocus());
  useEffect(() => {
    const front = () => setForeground(true);
    const back = () => setForeground(false);
    window.addEventListener("focus", front);
    window.addEventListener("blur", back);
    return () => {
      window.removeEventListener("focus", front);
      window.removeEventListener("blur", back);
    };
  }, []);
  return foreground;
}

/**
 * Tells oxd which session the window shows in front of the user: `sessionId`
 * while the window is in front, and nothing while it is not.
 */
export function useFocus(sessionId: string | null) {
  const foreground = useForeground();
  const target = foreground ? sessionId : null;
  useEffect(() => {
    invoke("focus", { sessionId: target });
  }, [target]);
}
