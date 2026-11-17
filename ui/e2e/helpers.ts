import { spawnSync } from "node:child_process";
import { mkdtemp, realpath } from "node:fs/promises";
import { join } from "node:path";

function workspaceGroup(cwd: string) {
  return $(`//section[@class="workspace"][.//h2[@title="${cwd}"]]`);
}

export function sessionButton(cwd: string, row = "li.session") {
  return workspaceGroup(cwd).$(`${row} button.select[title]`);
}

/** Adds a fresh workspace through Tauri, bypassing the native folder picker. */
export async function workspace(): Promise<string> {
  const cwd = await realpath(await mkdtemp(join(process.env.OX_E2E_DIR!, "workspace-")));
  await browser.execute(async (path) => {
    const { __TAURI_INTERNALS__: tauri } = window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (command: string, args: { path: string }) => Promise<unknown>;
      };
    };
    await tauri.invoke("add_workspace", { path });
  }, cwd);
  return cwd;
}

/** Opens a draft and sends the prompt that starts its agent. */
export async function startSession(cwd: string, prompt: string) {
  const group = workspaceGroup(cwd);
  const newSession = group.$('button[aria-label="New Session"]');
  await expect(newSession).toBeEnabled();
  await newSession.click();
  await group.$("button=Session in Checkout").click();
  await $(".composer textarea").setValue(prompt);
  await $('.composer button[aria-label="Send"]').click();
  await expect(sessionButton(cwd)).toHaveAttribute("aria-current", "true");
}

/** Signals only the daemon on this run's socket. */
export function stopDaemon() {
  const socket = process.env.OX_SOCKET!.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const { error, status } = spawnSync("pkill", ["-TERM", "-f", `oxd --socket ${socket}$`], {
    stdio: "inherit",
  });
  if (error) throw error;
  if (status !== 0 && status !== 1) throw new Error(`pkill exited with status ${status}`);
  return status === 0;
}
