import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { sessionAgent } from "../../components/HarnessLogo";
import type { ConfigOption, Controls, SessionInfo, WorkspaceInfo } from "../../lib/protocol";
import type { AgentId } from "../sessions/useAgents";

/** Option values by option ID. */
export type Values = Record<string, string | boolean>;

const controlsPrefix = "controls:";
const lastUsedPrefix = "lastUsed:";

/** The local storage entries under `prefix`, by the rest of their keys. */
function loadAll<T>(prefix: string): Record<string, T> {
  const entries: Record<string, T> = {};
  for (let index = 0; index < localStorage.length; index++) {
    const key = localStorage.key(index);
    if (key?.startsWith(prefix))
      entries[key.slice(prefix.length)] = JSON.parse(localStorage.getItem(key)!);
  }
  return entries;
}

/**
 * The controls each agent offers a new session in each workspace, which its
 * bridge reports when the desktop core probes it, kept in local storage.
 * Each installed agent and listed workspace with no entry is probed, one
 * probe at a time; `probe` asks again for one pair in the background. A pair
 * with no entry yet takes the agent's entry from another workspace.
 */
export function useControls(agents: AgentId[], workspaces: WorkspaceInfo[]) {
  const [controls, setControls] = useState(() => loadAll<Controls>(controlsPrefix));
  const queue = useRef(Promise.resolve());
  const probing = useRef(new Set<string>());

  const probe = useCallback((agent: AgentId, workspace: WorkspaceInfo) => {
    const pair = `${agent}:${workspace.id}`;
    if (probing.current.has(pair)) return;
    probing.current.add(pair);
    queue.current = queue.current.then(async () => {
      try {
        // TODO: A probe that never ends holds up every later probe.
        const probed = await invoke<Controls>("agent_controls", { agent, cwd: workspace.path });
        localStorage.setItem(controlsPrefix + pair, JSON.stringify(probed));
        setControls((current) => ({ ...current, [pair]: probed }));
      } catch (e) {
        console.error(`Reading the controls of ${agent} in ${workspace.path}:`, e);
      } finally {
        probing.current.delete(pair);
      }
    });
  }, []);

  useEffect(() => {
    for (const agent of agents) {
      for (const workspace of workspaces) {
        if (!(`${agent}:${workspace.id}` in controls)) probe(agent, workspace);
      }
    }
  }, [agents, workspaces, controls, probe]);

  return useMemo(
    () => ({
      get: (agent: AgentId, workspaceId: string): Controls | undefined =>
        controls[`${agent}:${workspaceId}`] ??
        Object.entries(controls).find(([pair]) => pair.startsWith(`${agent}:`))?.[1],
      probe,
    }),
    [controls, probe],
  );
}

export type ControlsCache = ReturnType<typeof useControls>;

/** The current value of each of `options`. */
function valuesOf(options: ConfigOption[]): Values {
  return Object.fromEntries(options.map((option) => [option.configId, option.currentValue]));
}

function sameValues(a: Values | undefined, b: Values) {
  return (
    a !== undefined &&
    Object.keys(a).length === Object.keys(b).length &&
    Object.entries(b).every(([id, value]) => a[id] === value)
  );
}

/**
 * The option values last used with each agent in each workspace, kept in
 * local storage: those of the newest session that reports options, or those
 * a draft was sent with, through `remember`.
 */
export function useLastUsed(sessions: SessionInfo[]) {
  const [lastUsed, setLastUsed] = useState(() => loadAll<Values>(lastUsedPrefix));

  const remember = useCallback((agent: AgentId, workspaceId: string, values: Values) => {
    const pair = `${agent}:${workspaceId}`;
    setLastUsed((current) => {
      if (sameValues(current[pair], values)) return current;
      localStorage.setItem(lastUsedPrefix + pair, JSON.stringify(values));
      return { ...current, [pair]: values };
    });
  }, []);

  // The newest session of each agent in each workspace.
  const newest = useMemo(() => {
    const newest = new Map<string, SessionInfo>();
    for (const session of sessions) {
      const agent = sessionAgent(session.command);
      if (agent === undefined) continue;
      const pair = `${agent}:${session.workspace_id}`;
      const current = newest.get(pair);
      if (current === undefined || current.created_at < session.created_at) {
        newest.set(pair, session);
      }
    }
    return newest;
  }, [sessions]);

  useEffect(() => {
    for (const session of newest.values()) {
      if (session.config_options.length === 0) continue;
      remember(
        sessionAgent(session.command)!,
        session.workspace_id,
        valuesOf(session.config_options),
      );
    }
  }, [newest, remember]);

  return useMemo(
    () => ({
      get: (agent: AgentId, workspaceId: string): Values | undefined =>
        lastUsed[`${agent}:${workspaceId}`],
      remember,
    }),
    [lastUsed, remember],
  );
}

export type LastUsed = ReturnType<typeof useLastUsed>;

/** Whether `option` offers `value`. */
function offers(option: ConfigOption, value: string | boolean | undefined) {
  if (option.type === "boolean") return typeof value === "boolean";
  return option.options.some((entry) =>
    "group" in entry
      ? entry.options.some((choice) => choice.value === value)
      : entry.value === value,
  );
}

/**
 * `options` with each current value taken from the first of `sources` whose
 * value for it the option offers, else left as it is.
 */
export function withValues(
  options: ConfigOption[],
  ...sources: (Values | undefined)[]
): ConfigOption[] {
  return options.map((option) => {
    const value = sources
      .map((source) => source?.[option.configId])
      .find((value) => value !== undefined && offers(option, value));
    return value === undefined || value === option.currentValue
      ? option
      : ({ ...option, currentValue: value } as ConfigOption);
  });
}
