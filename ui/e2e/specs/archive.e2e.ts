import { access, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  agentText,
  commit,
  composer,
  deleteSessions,
  git,
  gitWorkspace,
  send,
  startStuckSession,
  startWorktreeSession,
  workspaceGroup,
} from "../helpers.ts";

function exists(path: string) {
  return access(path).then(
    () => true,
    () => false,
  );
}

/** The entry of the workspace's one session. */
function entry(cwd: string) {
  return workspaceGroup(cwd).$('./ul/li[@class="sidebar-row session"]');
}

/**
 * WebDriver cannot choose a native menu item. Dispatch the row's contextmenu
 * event and deliver the choice through Tauri's menu channel instead of showing
 * the native popup. Menu creation and the session action still run normally.
 */
async function sessionAction(
  cwd: string,
  text: "Archive Session" | "Unarchive Session" | "Delete Session",
  archiveDisabled = false,
) {
  const archiveEnabled = await browser.execute(
    async (element, text) => {
      type MenuItem = { text: string; enabled: boolean; handler: string };
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            runCallback: (id: number, data: { index: number; message: string }) => void;
          };
        }
      ).__TAURI_INTERNALS__;
      const fetch = window.fetch;
      try {
        return await new Promise<boolean>((resolve, reject) => {
          let item: MenuItem;
          let archiveEnabled = false;
          // Tauri's invoke is read-only; intercept its IPC fetch instead.
          window.fetch = async (input, options) => {
            const command = decodeURIComponent(String(input));
            if (command.includes("plugin:menu|new")) {
              const { options: menu } = JSON.parse(String(options?.body)) as {
                options: { items: MenuItem[] };
              };
              archiveEnabled = menu.items.some(
                (item) => item.text === "Archive Session" && item.enabled,
              );
              const choice = menu.items.find((item) => item.text === text);
              if (!choice?.enabled) {
                reject(new Error(`${text} is missing or disabled`));
                return new Response(JSON.stringify("menu choice unavailable"), {
                  headers: { "Content-Type": "application/json", "Tauri-Response": "error" },
                });
              }
              item = choice;
            }
            if (command.includes("plugin:menu|popup")) {
              window.fetch = fetch;
              const channel = Number(item.handler.split(":")[1]);
              internals.runCallback(channel, { index: 0, message: "" });
              resolve(archiveEnabled);
              return new Response("null", {
                headers: { "Content-Type": "application/json", "Tauri-Response": "ok" },
              });
            }
            return fetch.call(window, input, options);
          };
          element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
        });
      } finally {
        window.fetch = fetch;
      }
    },
    await entry(cwd),
    text,
  );
  if (archiveDisabled) expect(archiveEnabled).toBe(false);
}

/** The Archived group's toggle in the workspace in `cwd`. */
function archivedToggle(cwd: string) {
  return workspaceGroup(cwd).$("li.archived-toggle button.select");
}

/** Opens the Archived group in the workspace in `cwd`. */
async function openArchived(cwd: string) {
  const toggle = archivedToggle(cwd);
  await expect(toggle).toBeDisplayed();
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
}

/** A worktree session that has answered a prompt, selected. */
async function conversedWorktree(workspaceCwd: string, workspace: string) {
  await startWorktreeSession(workspace);
  await expect(entry(workspaceCwd)).toBeExisting();
  const select = entry(workspaceCwd).$("button.select");
  const path = (await select.getAttribute("title"))!.replace(/\/$/, "");
  await select.click();
  await send("say 2 0");
  await expect(agentText()[0]).toHaveText("chunk-1chunk-2");
  return path;
}

/**
 * Archives the workspace's one session, whose worktree holds the fake agent's
 * session log, which counts as unsaved work.
 */
async function archiveDiscarding(cwd: string) {
  await sessionAction(cwd, "Archive Session");
  await $("dialog[open]").$("button=Discard and Archive").click();
  await expect(entry(cwd)).not.toBeExisting();
}

// A test that fails with a dialog open leaves it to the next test.
afterEach(async () => {
  while (await $("dialog[open]").isExisting()) await browser.keys("Escape");
});

describe("A session stuck starting", () => {
  it("offers Delete and disables Archive", async () => {
    const cwd = await startStuckSession();
    await expect(entry(cwd).$('[aria-label="Starting"]')).toBeDisplayed();
    await sessionAction(cwd, "Delete Session", true);
    await expect(entry(cwd)).not.toBeExisting();
  });
});

describe("Archiving a worktree session", () => {
  it("has no Archived group until a session is archived", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await startWorktreeSession(workspace);
    await expect(entry(cwd)).toBeExisting();
    await expect(archivedToggle(cwd)).not.toBeExisting();
  });

  it("moves a clean session into the collapsed Archived group and keeps its branch", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await startWorktreeSession(workspace);
    await expect(entry(cwd)).toBeExisting();
    const path = (await entry(cwd).$("button.select").getAttribute("title"))!.replace(/\/$/, "");
    const branch = (await git(path, "branch", "--show-current")).trim();

    await expect(entry(cwd).$(".row-actions")).not.toBeExisting();
    await sessionAction(cwd, "Archive Session");
    await expect(entry(cwd)).not.toBeExisting();
    await expect(archivedToggle(cwd)).toHaveAttribute("aria-expanded", "false");
    expect(await exists(path)).toBe(false);
    expect((await git(cwd, "branch", "--list", branch)).trim()).toBe(branch);
  });

  it("shows the conversation of an archived session with the composer disabled", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await conversedWorktree(cwd, workspace);
    await archiveDiscarding(cwd);
    await openArchived(cwd);
    await expect(entry(cwd)).toBeExisting();
    await entry(cwd).$("button.select").click();
    await expect(agentText()[0]).toHaveText("chunk-1chunk-2");
    await expect(composer()).toBeDisabled();
  });

  it("asks before discarding unsaved work", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await startWorktreeSession(workspace);
    await expect(entry(cwd)).toBeExisting();
    const path = (await entry(cwd).$("button.select").getAttribute("title"))!.replace(/\/$/, "");
    await writeFile(join(path, "unsaved.txt"), "unsaved\n");
    await sessionAction(cwd, "Archive Session");
    let dialog = $("dialog[open]");
    await expect(dialog.$(".message")).toHaveText(expect.stringContaining(path));
    await dialog.$("button=Cancel").click();
    await expect(dialog).not.toBeExisting();
    await expect(entry(cwd)).toBeExisting();
    expect(await exists(path)).toBe(true);

    await sessionAction(cwd, "Archive Session");
    dialog = $("dialog[open]");
    await dialog.$("button=Discard and Archive").click();
    await expect(entry(cwd)).not.toBeExisting();
    await expect(archivedToggle(cwd)).toBeDisplayed();
    expect(await exists(path)).toBe(false);
  });
});

describe("An archived session", () => {
  it("returns to the active list on unarchive, and loads when opened", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const path = await conversedWorktree(cwd, workspace);
    const branch = (await git(path, "branch", "--show-current")).trim();
    // The fake agent keeps its session log in the worktree; committing it
    // lets the unarchived worktree restore the session.
    await commit(path);
    await sessionAction(cwd, "Archive Session");
    await openArchived(cwd);

    await sessionAction(cwd, "Unarchive Session");
    await expect(archivedToggle(cwd)).not.toBeExisting();
    await expect(entry(cwd)).toBeExisting();
    expect((await git(path, "branch", "--show-current")).trim()).toBe(branch);
    await expect(entry(cwd).$('[aria-label="Exited"]')).toBeExisting();

    await entry(cwd).$("button.select").click();
    await expect(entry(cwd).$('[aria-label="Idle"]')).toBeDisplayed();
    await send("say 1 0");
    await expect(agentText()).toBeElementsArrayOfSize(2);
    await expect(agentText()[1]).toHaveText("chunk-1");
  });

  it("disappears when deleted", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await startWorktreeSession(workspace);
    await expect(entry(cwd)).toBeExisting();
    await sessionAction(cwd, "Archive Session");
    await openArchived(cwd);

    await expect(entry(cwd)).toBeExisting();
    await deleteSessions(cwd);
    await expect(entry(cwd)).not.toBeExisting();
    await expect(archivedToggle(cwd)).not.toBeExisting();
  });
});
