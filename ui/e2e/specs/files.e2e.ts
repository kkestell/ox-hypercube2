import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { openSession, startSession } from "../helpers.ts";

/** The right sidebar's row for the entry `name`. */
function treeRow(name: string) {
  return $(
    `//aside[@class="panel right"]//div[@class="sidebar-row tree-row"][.//span[@class="title"][.="${name}"]]`,
  );
}

describe("The file tree", () => {
  it("shows the viewed session's folder live and keeps its open folders", async () => {
    const cwd = await startSession();
    await writeFile(join(cwd, "a.txt"), "a\n");
    await mkdir(join(cwd, "sub"));
    await writeFile(join(cwd, "sub", "inner.txt"), "inner\n");
    const other = await startSession();
    await openSession(cwd);
    await expect(treeRow("sub")).toBeDisplayed();
    await expect(treeRow("a.txt")).toBeDisplayed();

    await writeFile(join(cwd, "b.txt"), "b\n");
    await expect(treeRow("b.txt")).toBeDisplayed();

    const sub = treeRow("sub").$("button.select");
    await sub.click();
    await expect(sub).toHaveAttribute("aria-expanded", "true");
    await expect(treeRow("inner.txt")).toBeDisplayed();

    await openSession(other);
    await expect(treeRow("sub")).not.toBeExisting();
    await openSession(cwd);
    await expect(treeRow("sub").$("button.select")).toHaveAttribute("aria-expanded", "true");
    await expect(treeRow("inner.txt")).toBeDisplayed();
  });
});
