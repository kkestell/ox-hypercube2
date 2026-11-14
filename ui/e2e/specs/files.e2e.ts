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
    await mkdir(join(cwd, "sub", "nested"));
    await writeFile(join(cwd, "sub", "inner.txt"), "inner\n");
    const other = await startSession();
    await openSession(cwd);
    await expect(treeRow("sub")).toBeDisplayed();
    await expect(treeRow("a.txt")).toBeDisplayed();
    await expect(treeRow("sub").$("svg.lucide-folder")).toBeDisplayed();
    const rootIconX = await treeRow("sub").$("svg").getLocation("x");
    expect(await treeRow("a.txt").$("svg").getLocation("x")).toBe(rootIconX);

    await writeFile(join(cwd, "b.txt"), "b\n");
    await expect(treeRow("b.txt")).toBeDisplayed();

    const sub = treeRow("sub").$("button.select");
    await sub.click();
    await expect(sub).toHaveAttribute("aria-expanded", "true");
    await expect(treeRow("inner.txt")).toBeDisplayed();
    await expect(sub.$("svg.lucide-folder-open")).toBeDisplayed();
    await expect(treeRow("nested")).toBeDisplayed();
    const childIconX = await treeRow("nested").$("svg").getLocation("x");
    expect(await treeRow("inner.txt").$("svg").getLocation("x")).toBe(childIconX);
    expect(childIconX).toBeGreaterThan(rootIconX);

    await openSession(other);
    await expect(treeRow("sub")).not.toBeExisting();
    await openSession(cwd);
    await expect(treeRow("sub").$("button.select")).toHaveAttribute("aria-expanded", "true");
    await expect(treeRow("inner.txt")).toBeDisplayed();
  });
});
