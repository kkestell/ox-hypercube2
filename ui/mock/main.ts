/**
 * Runs the app in a browser on a scenario's fixed state, chosen with
 * `?scenario=<name>`, in place of the desktop core and oxd. Commands that
 * change state do nothing.
 */
import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import type { Branches, WatchMessage } from "../src/lib/protocol";
import { scenarios } from "./scenarios";

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
      return ["claude", "pi"];
    case "workspace_branches":
      return { head: "main" } satisfies Branches;
  }
});

import("../src/main");
