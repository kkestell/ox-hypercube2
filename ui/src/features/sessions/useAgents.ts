import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { HarnessConfig } from "../../lib/protocol";
import { showError } from "../../lib/showError";

export type AgentId = "claude" | "pi";

export const agentNames: Record<AgentId, string> = { claude: "Claude", pi: "Pi" };

/**
 * The harnesses Ox's config lists that are installed, with the models,
 * effort levels and modes it lists, in the order Claude, Pi. Empty until the
 * app first answers, and while the config cannot be read; asked again each
 * time the window gains focus, so an edit of the config applies then.
 */
export function useAgents(): HarnessConfig[] {
  const [harnesses, setHarnesses] = useState<HarnessConfig[]>([]);

  useEffect(() => {
    let mounted = true;
    const detect = async () => {
      try {
        const listed = await invoke<HarnessConfig[]>("agents");
        if (!mounted) return;
        setHarnesses((harnesses) =>
          JSON.stringify(harnesses) === JSON.stringify(listed) ? harnesses : listed,
        );
      } catch (e) {
        if (mounted) setHarnesses([]);
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

  return harnesses;
}
