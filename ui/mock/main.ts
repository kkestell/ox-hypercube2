/**
 * Runs the app in a browser on a scenario's fixed state, chosen with
 * `?scenario=<name>`, in place of the desktop core and oxd. Commands that
 * change state do nothing.
 */
import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Branches, Choice, HarnessConfig, WatchMessage } from "../src/lib/protocol";
import { scenarios } from "./scenarios";

const levels = (...names: string[]): Choice[] =>
  names.map((name) => ({ id: name.toLowerCase(), name }));

/** What Ox's config lists, as `agents` answers. */
const harnesses: HarnessConfig[] = [
  {
    harness: "claude",
    models: [
      {
        id: "claude-opus-5-5",
        name: "Opus 5.5",
        provider: null,
        efforts: levels("Medium", "High"),
      },
      {
        id: "claude-sonnet-5-5",
        name: "Sonnet 5.5",
        provider: null,
        efforts: levels("High", "XHigh"),
      },
      {
        id: "claude-haiku-5-5",
        name: "Haiku 5.5",
        provider: null,
        efforts: levels("Low", "Medium"),
      },
    ],
    modes: [
      { id: "manual", name: "Manual" },
      { id: "auto", name: "Auto" },
      { id: "acceptEdits", name: "Accept edits" },
      { id: "plan", name: "Plan" },
    ],
  },
  {
    harness: "pi",
    models: [
      {
        id: "openai/gpt-6-luna",
        name: "GPT-6 Luna",
        provider: "OpenAI",
        efforts: levels("Low", "Medium", "High", "XHigh"),
      },
      {
        id: "openrouter/deepseek/deepseek-v4.1-flash-1017",
        name: "DeepSeek V4.1 Flash",
        provider: "OpenRouter",
        efforts: levels("Low", "High"),
      },
    ],
    modes: [],
  },
];

const name = new URLSearchParams(location.search).get("scenario") ?? "sidebar";
const scenario = scenarios[name];
if (scenario === undefined) {
  throw new Error(`No scenario "${name}". Scenarios: ${Object.keys(scenarios).join(", ")}`);
}

mockIPC((cmd, args) => {
  const { channel, sessionId } = (args ?? {}) as {
    channel?: Channel<unknown>;
    sessionId?: string;
  };
  switch (cmd) {
    case "watch":
      channel?.onmessage({ type: "changes", changes: scenario.changes } satisfies WatchMessage);
      return;
    case "view_session":
      channel?.onmessage(scenario.conversations?.[sessionId ?? ""] ?? []);
      return;
    case "watch_directory":
      channel?.onmessage([]);
      return;
    case "agents":
      return harnesses;
    case "workspace_branches":
      return { head: "main" } satisfies Branches;
  }
});

import("../src/main");
