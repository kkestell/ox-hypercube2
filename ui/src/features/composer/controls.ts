import { useCallback, useEffect, useMemo, useState } from "react";
import { sessionAgent } from "../../components/HarnessLogo";
import type {
  Choice,
  ConfigOption,
  HarnessConfig,
  ModelConfig,
  SelectOption,
  SessionInfo,
} from "../../lib/protocol";
import type { AgentId } from "../sessions/useAgents";

/** Option values by option ID. */
export type Values = Record<string, string | boolean>;

type Select = Extract<ConfigOption, { type: "select" }>;

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

/** The picker choices of `models`, each with its provider. */
function modelChoices(models: ModelConfig[]): SelectOption[] {
  return models.map((model) => ({
    value: model.id,
    name: model.name,
    _meta: model.provider === null ? undefined : { "ox/provider": model.provider },
  }));
}

function choicesOf(choices: Choice[]): SelectOption[] {
  return choices.map((choice) => ({ value: choice.id, name: choice.name }));
}

/** `choices`, ending with `value` named by itself when they list others but not it. */
function withReported(choices: SelectOption[], value: string): SelectOption[] {
  return choices.length === 0 || choices.some((choice) => choice.value === value)
    ? choices
    : [...choices, { value, name: value }];
}

function select(
  configId: string,
  name: string,
  category: string,
  currentValue: string,
  options: SelectOption[],
): Select {
  return { configId, name, category, type: "select", currentValue, options };
}

/**
 * A draft's options for `harness`: a Model select of the models its config
 * lists, an Effort select of the chosen model's effort levels, and a Mode
 * select of its modes, the last two only where there are any. Each is at the
 * first value of `sources` it offers, else at its first choice. The effort is
 * chosen after the model, from the chosen model's levels.
 */
export function draftOptions(
  harness: HarnessConfig,
  ...sources: (Values | undefined)[]
): ConfigOption[] {
  const pick = (configId: string, ids: string[]) =>
    sources
      .map((source) => source?.[configId])
      .find((value): value is string => typeof value === "string" && ids.includes(value)) ??
    ids[0]!;
  const options: ConfigOption[] = [];
  const modelId =
    harness.models.length > 0 &&
    pick(
      "model",
      harness.models.map(({ id }) => id),
    );
  if (modelId) {
    options.push(select("model", "Model", "model", modelId, modelChoices(harness.models)));
    const { efforts } = harness.models.find(({ id }) => id === modelId)!;
    if (efforts.length > 0) {
      const effort = pick(
        "effort",
        efforts.map(({ id }) => id),
      );
      options.push(select("effort", "Effort", "thought_level", effort, choicesOf(efforts)));
    }
  }
  if (harness.modes.length > 0) {
    const mode = pick(
      "mode",
      harness.modes.map(({ id }) => id),
    );
    options.push(select("mode", "Mode", "mode", mode, choicesOf(harness.modes)));
  }
  return options;
}

/**
 * A running session's options with the choices `harness` lists: its Model,
 * Effort and Mode selects, each where the session reports an option of its
 * category, at the reported value and with the reported option's ID. The
 * effort levels are those of the listed model the session reports. A
 * reported effort or mode they do not list is added as its ID; the model
 * picker shows an unlisted model's ID itself. A select with nothing listed,
 * such as every one of a harness the config does not list, is left out.
 */
export function sessionOptions(
  harness: HarnessConfig | undefined,
  reported: ConfigOption[],
): ConfigOption[] {
  const model = reported.find((option) => option.category === "model")?.currentValue;
  const efforts = harness?.models.find(({ id }) => id === model)?.efforts ?? [];
  return reported.flatMap((option): ConfigOption[] => {
    if (option.type !== "select") return [option];
    const listed = (name: string, choices: SelectOption[]) =>
      choices.length === 0
        ? []
        : [select(option.configId, name, option.category!, option.currentValue, choices)];
    switch (option.category) {
      case "model":
        return listed("Model", modelChoices(harness?.models ?? []));
      case "thought_level":
        return listed("Effort", withReported(choicesOf(efforts), option.currentValue));
      case "mode":
        return listed("Mode", withReported(choicesOf(harness?.modes ?? []), option.currentValue));
      default:
        return [option];
    }
  });
}
