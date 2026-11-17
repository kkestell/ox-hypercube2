import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import { startSession, workspace } from "../helpers.ts";

it("shows a new file and updates its open contents as it changes on disk", async () => {
  const cwd = await workspace();
  await writeFile(join(cwd, "existing.txt"), "existing\n");
  await startSession(cwd, 'text "hello"');
  const tree = $("aside.panel.right");
  await expect(tree.$("button=existing.txt")).toBeDisplayed();

  const file = join(cwd, "notes.txt");
  await writeFile(file, "first\n");
  await tree.$("button=notes.txt").click();
  const contents = $(".file-body pre");
  await expect(contents).toHaveText("first");

  await writeFile(file, "second\n");
  await expect(contents).toHaveText("second");
});
