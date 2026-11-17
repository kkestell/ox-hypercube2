import { useCallback, useEffect, useMemo, useState } from "react";
import { sessionAgent } from "../../components/HarnessLogo";
import type {
  Choice,
  ConfigOption,
  ConfigSetting,
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

/** The value of each setting of `config`. */
function valuesOf(config: ConfigSetting[]): Values {
  return Object.fromEntries(config.map((setting) => [setting.config_id, setting.value]));
}

/** The settings that set an agent to each of `options`' current values. */
export function settingsOf(options: ConfigOption[]): ConfigSetting[] {
  return options.map((option) => ({ config_id: option.configId, value: option.currentValue }));
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
 * local storage: those of the newest session that has a config, or those a
 * draft was sent with, through `remember`.
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
      if (session.config.length === 0) continue;
      remember(sessionAgent(session.command)!, session.workspace_id, valuesOf(session.config));
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
 * A session's options: its Model, Effort and Mode selects, each where its
 * config has a `model`, `effort` or `mode` setting, at the recorded value
 * and with the choices `harness` lists. The effort levels are those of the
 * recorded model. A recorded effort or mode they do not list is added as its
 * ID; the model picker shows an unlisted model's ID itself. A select with
 * nothing listed, such as every one of a harness the config does not list,
 * is left out.
 */
export function sessionOptions(
  harness: HarnessConfig | undefined,
  config: ConfigSetting[],
): ConfigOption[] {
  const values = valuesOf(config);
  const recorded = (configId: string) => {
    const value = values[configId];
    return typeof value === "string" ? value : undefined;
  };
  const model = recorded("model");
  const efforts = harness?.models.find(({ id }) => id === model)?.efforts ?? [];
  const listed = (
    configId: string,
    name: string,
    category: string,
    choices: (value: string) => SelectOption[],
  ): ConfigOption[] => {
    const value = recorded(configId);
    if (value === undefined) return [];
    const offered = choices(value);
    return offered.length === 0 ? [] : [select(configId, name, category, value, offered)];
  };
  return [
    ...listed("model", "Model", "model", () => modelChoices(harness?.models ?? [])),
    ...listed("effort", "Effort", "thought_level", (value) =>
      withReported(choicesOf(efforts), value),
    ),
    ...listed("mode", "Mode", "mode", (value) =>
      withReported(choicesOf(harness?.modes ?? []), value),
    ),
  ];
}

/**
 * The settings one pick sends: `configId` at `value`, and with a new model,
 * the effort the draft rule picks from the model's levels in `shown`, the
 * config the pickers show, or none where the model lists none. The model and
 * effort go together because an agent may reset its effort when its model
 * changes.
 */
export function pickedSettings(
  harness: HarnessConfig | undefined,
  shown: ConfigSetting[],
  configId: string,
  value: string | boolean,
): ConfigSetting[] {
  const picked = { config_id: configId, value };
  if (!harness || configId !== "model") return [picked];
  const effort = draftOptions(harness, { model: value }, valuesOf(shown)).find(
    (option) => option.configId === "effort",
  );
  return effort ? [picked, { config_id: "effort", value: effort.currentValue }] : [picked];
}
