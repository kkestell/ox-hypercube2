import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { showError } from "../../lib/showError";

export type AgentId = "claude" | "pi";

export const agentNames: Record<AgentId, string> = { claude: "Claude", pi: "Pi" };

/**
 * The agents whose harnesses are installed, in the order Claude, Pi. Empty
 * until the app first answers; asked again each time the window gains focus.
 */
export function useAgents(): AgentId[] {
  const [agents, setAgents] = useState<AgentId[]>([]);

  useEffect(() => {
    let mounted = true;
    const detect = async () => {
      try {
        const installed = await invoke<AgentId[]>("agents");
        if (!mounted) return;
        setAgents((agents) =>
          agents.length === installed.length && agents.every((id, i) => id === installed[i])
            ? agents
            : installed,
        );
      } catch (e) {
        await showError(e);
      }
    };
    detect();
    window.addEventListener("focus", detect);
    return () => {
      mounted = false;
      window.removeEventListener("focus", detect);
    };
  }, []);

  return agents;
}
