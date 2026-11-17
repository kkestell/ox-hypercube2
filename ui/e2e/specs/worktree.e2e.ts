import { access } from "node:fs/promises";
import { join } from "node:path";
import {
  agentText,
  commit,
  compareSession,
  git,
  gitWorkspace,
  newWorktreeSession,
  rowAction,
  send,
  startWorktreeSession,
  workspaceGroup,
  worktreeEntry,
} from "../helpers.ts";

/** The Merge button in the session's subheader. */
function mergeButton() {
  return $(".subheader").$("button.text-button");
}

// A test that fails with a dialog open leaves it to the next test.
afterEach(async () => {
  while (await $("dialog[open]").isExisting()) await browser.keys("Escape");
});

describe("A session in a worktree", () => {
  it("starts on a new branch at main and merges its commit into main", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const path = await newWorktreeSession(cwd);
    const branch = (await git(path, "branch", "--show-current")).trim();
    expect(branch).toMatch(/^ox\//);
    expect(await git(path, "rev-parse", "HEAD")).toBe(await git(cwd, "rev-parse", "main"));
    await expect(mergeButton()).toHaveAttribute("title", "No commits");

    const file = await commit(path);
    const tip = await git(path, "rev-parse", "HEAD");
    await compareSession(workspace);
    await expect(mergeButton()).toBeEnabled();
    await mergeButton().click();
    const dialog = $("dialog[open]");
    await expect(dialog.$("h2")).toHaveText("Merged into main");
    expect(await git(cwd, "rev-parse", "main")).toBe(tip);
    await access(join(cwd, file));

    await dialog.$("button=Delete Session").click();
    await expect(worktreeEntry()).not.toBeExisting();
    expect(await git(cwd, "branch", "--list", branch)).toBe("");
  });

  it("is archived, then unarchived and resumed", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const path = await startWorktreeSession(workspace);
    await worktreeEntry().$("button.select").click();
    await send("say 1 0");
    await expect(agentText()[0]).toHaveText("chunk-1");
    // The fake agent's session log in the worktree is unsaved work; committed,
    // it lets the unarchived worktree restore the session.
    await commit(path);

    await rowAction(worktreeEntry(), "Archive Session");
    await expect(worktreeEntry()).not.toBeExisting();
    await rowAction(workspaceGroup(cwd).$("h2"), "Show Archived");
    await rowAction(worktreeEntry(), "Unarchive Session");
    // Listed with archived sessions hidden once it is unarchived.
    await rowAction(workspaceGroup(cwd).$("h2"), "Show Archived");
    await expect(worktreeEntry().$('[aria-label="Exited"]')).toBeExisting();

    // Viewing the session loads its agent, which takes prompts once loaded.
    await worktreeEntry().$("button.select").click();
    await send("say 1 0");
    await expect(agentText()).toBeElementsArrayOfSize(2);
  });
});
