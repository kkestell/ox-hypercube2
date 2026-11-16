import { access, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  answerNativeDialog,
  commit,
  compareSession,
  composer,
  git,
  gitWorkspace,
  newTask,
  openMenu,
  openSession,
  send,
  sessionEntry,
  startCheckoutSession,
  startWorktreeSession,
  worktreeEntry,
} from "../helpers.ts";

/** The Merge button in the content header. */
function mergeButton() {
  return $(".content-header").$("button.text-button");
}

/** The git status icon of the one worktree session's row. */
function branchStatus() {
  return worktreeEntry().$(".branch-status");
}

/** Clicks Merge into main and waits for the dialog that follows the move. */
async function merge() {
  await expect(mergeButton()).toHaveText("Merge into main");
  await expect(mergeButton()).toBeEnabled();
  await mergeButton().click();
  const dialog = $("dialog[open]");
  await expect(dialog.$("h2")).toHaveText("Merged into main");
  return dialog;
}

// A test that fails with a dialog open leaves it to the next test.
afterEach(async () => {
  while (await $("dialog[open]").isExisting()) await browser.keys("Escape");
});

describe("Local merge", () => {
  it("preserves an ignored local file that the task would overwrite", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await writeFile(join(cwd, ".gitignore"), "local.env\n");
    await commit(cwd);
    const base = await git(cwd, "rev-parse", "HEAD");
    const path = await newTask(cwd);
    await writeFile(join(path, "local.env"), "from task\n");
    await git(path, "add", "-f", "local.env");
    await commit(path);
    await writeFile(join(cwd, "local.env"), "keep me\n");
    await compareSession(workspace);
    await expect(mergeButton()).toBeEnabled();
    await answerNativeDialog("message", null);
    await mergeButton().click();
    await browser.waitUntil(
      async () => (await mergeButton().isEnabled()) || (await $("dialog[open]").isExisting()),
    );

    expect(await readFile(join(cwd, "local.env"), "utf8")).toBe("keep me\n");
    expect(await git(cwd, "rev-parse", "HEAD")).toBe(base);
    await expect($("body")).toHaveAttribute(
      "data-native-dialog",
      expect.stringContaining("local.env"),
    );
  });

  it("refuses to merge a task whose branch changed since comparison", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const base = await git(cwd, "rev-parse", "HEAD");
    const path = await newTask(cwd);
    await commit(path);
    await compareSession(workspace);
    await expect(mergeButton()).toBeEnabled();
    await git(path, "reset", "--hard", "main");
    await commit(path);
    await commit(path);
    await answerNativeDialog("message", null);
    await mergeButton().click();
    await browser.waitUntil(
      async () =>
        (await $("body").getAttribute("data-native-dialog")) !== null ||
        (await $("dialog[open]").isExisting()),
    );

    expect(await git(cwd, "rev-parse", "HEAD")).toBe(base);
    await expect($("body")).toHaveAttribute(
      "data-native-dialog",
      expect.stringContaining("expected"),
    );
    await expect(mergeButton()).toHaveAttribute("title", "2 commits: ask the agent to squash");
  });

  it("blocks prompts in the destination checkout until the merge finishes", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await startCheckoutSession(workspace);
    const path = await newTask(cwd);
    await commit(path);
    const tip = await git(path, "rev-parse", "HEAD");
    await compareSession(workspace, path);
    await expect(mergeButton()).toBeEnabled();

    // Pause Git after Ox has checked the destination, while it updates main.
    // The pause outlasts the test's timeout, so only a hung test fails the move.
    const ready = join(cwd, ".git", "move-ready");
    const release = join(cwd, ".git", "move-release");
    await writeFile(
      join(cwd, ".git", "hooks", "reference-transaction"),
      `#!/bin/sh
[ "$1" = prepared ] || exit 0
grep -q ' refs/heads/main$' || exit 0
dir="$(dirname "$0")/.."
touch "$dir/move-ready"
attempt=0
while [ "$attempt" -lt 90 ]; do
  [ ! -f "$dir/move-release" ] || exit 0
  attempt=$((attempt + 1))
  sleep 1
done
exit 1
`,
      { mode: 0o755 },
    );
    await git(cwd, "config", "core.hooksPath", join(cwd, ".git", "hooks"));
    await mergeButton().click();
    try {
      await browser.waitUntil(() =>
        access(ready).then(
          () => true,
          () => false,
        ),
      );
      await openSession(cwd);
      await answerNativeDialog("message", null);
      await send("wait");
      await browser.waitUntil(
        async () =>
          (await $("body").getAttribute("data-native-dialog")) !== null ||
          (await sessionEntry(cwd).$('[aria-label="Running"]').isExisting()),
      );
      await expect(sessionEntry(cwd).$('[aria-label="Idle"]')).toBeDisplayed();
      await expect($("body")).toHaveAttribute(
        "data-native-dialog",
        expect.stringContaining("branch move"),
      );
      await expect(composer()).toHaveValue("wait");
    } finally {
      await writeFile(release, "");
    }
    await expect(branchStatus()).not.toBeExisting();
    expect(await git(cwd, "rev-parse", "HEAD")).toBe(tip);
    await send();
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).toBeDisplayed();
  });

  it("can't merge a task with no commits", async () => {
    const { cwd } = await gitWorkspace();
    await newTask(cwd);
    await expect(mergeButton()).toHaveText("Merge into main");
    await expect(mergeButton()).toBeDisabled();
    await expect(mergeButton()).toHaveAttribute("title", "No commits");
    await expect(branchStatus()).not.toBeExisting();
  });

  it("merges a task one commit ahead into main and deletes it", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await writeFile(join(cwd, ".gitignore"), "local.env\n");
    await commit(cwd);
    await writeFile(join(cwd, "local.env"), "keep me\n");
    const path = await newTask(cwd);
    const file = await commit(path);
    await compareSession(workspace);
    await expect(branchStatus()).toHaveAttribute("title", "1 commit ahead of main");
    await expect(branchStatus()).not.toHaveElementClass("attention");
    const branch = (await git(path, "branch", "--show-current")).trim();
    const tip = await git(path, "rev-parse", "HEAD");

    const dialog = await merge();
    expect(await git(cwd, "rev-parse", "main")).toBe(tip);
    expect(await readFile(join(cwd, "local.env"), "utf8")).toBe("keep me\n");
    await access(join(cwd, file));
    await expect(dialog.$("code")).toHaveText(branch);
    await dialog.$("button=Delete Session").click();
    await expect(dialog).not.toBeExisting();
    await expect(worktreeEntry()).not.toBeExisting();
    expect(await git(cwd, "branch", "--list", branch)).toBe("");
  });

  it("keeps a merged task, which then has no commits", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const path = await newTask(cwd);
    await commit(path);
    await compareSession(workspace);

    const dialog = await merge();
    await dialog.$("button=Keep Session").click();
    await expect(dialog).not.toBeExisting();
    await expect(worktreeEntry()).toBeExisting();
    await expect(mergeButton()).toHaveAttribute("title", "No commits");
    await expect(mergeButton()).toBeDisabled();
  });

  it("asks for a squash when the task has two commits", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const path = await newTask(cwd);
    await commit(path);
    await commit(path);
    await compareSession(workspace);
    await expect(branchStatus()).toHaveAttribute("title", "2 commits ahead of main");
    await expect(branchStatus()).toHaveElementClass("attention");
    await expect(mergeButton()).toHaveAttribute("title", "2 commits: ask the agent to squash");
    await expect(mergeButton()).toBeDisabled();
  });

  it("asks for a rebase when main has moved on", async () => {
    const { cwd, workspace } = await gitWorkspace();
    const path = await newTask(cwd);
    await commit(path);
    await commit(cwd);
    await compareSession(workspace);
    await expect(branchStatus()).toHaveAttribute("title", "1 commit ahead, 1 behind main");
    await expect(branchStatus()).toHaveElementClass("attention");
    await expect(mergeButton()).toHaveAttribute("title", "Behind main: ask the agent to rebase");
    await expect(mergeButton()).toBeDisabled();
  });

  it("leaves out sessions with no comparison ref", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await (await openMenu(cwd)).$("button=Session in Checkout").click();
    const checkout = sessionEntry(cwd).$("button.select");
    await expect(checkout).toHaveAttribute("aria-current", "true");
    await expect(mergeButton()).not.toBeExisting();
    await expect(sessionEntry(cwd).$(".branch-status")).not.toBeExisting();

    await startWorktreeSession(workspace);
    const worktree = worktreeEntry().$("button.select");
    await worktree.click();
    await expect(worktree).toHaveAttribute("aria-current", "true");
    await expect(mergeButton()).not.toBeExisting();
    await expect(branchStatus()).not.toBeExisting();
  });
});
