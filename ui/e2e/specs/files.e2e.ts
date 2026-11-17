import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { openSession, startSession } from "../helpers.ts";

/** The right sidebar's row for the entry `name`. */
function treeRow(name: string) {
  return $(
    `//aside[@class="panel right"]//div[contains(@class, "tree-row")][.//span[@class="title"][.="${name}"]]`,
  );
}

describe("The file tree and viewer", () => {
  it("follow the session's folder and open file on disk", async () => {
    const cwd = await startSession();
    await mkdir(join(cwd, "sub"));
    await writeFile(join(cwd, "sub", "main.rs"), "fn main() {}\n");
    await openSession(cwd);

    await treeRow("sub").$("button.select").click();
    await treeRow("main.rs").$("button.select").click();
    await expect($(".file-body pre")).toHaveText("fn main() {}");
    await writeFile(join(cwd, "sub", "main.rs"), "fn main() { let value = 1; }\n");
    await expect($(".file-body pre")).toHaveText("fn main() { let value = 1; }");

    await writeFile(join(cwd, "b.txt"), "b\n");
    await expect(treeRow("b.txt")).toBeDisplayed();
  });
});
