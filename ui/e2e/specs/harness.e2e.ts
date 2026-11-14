import { openMenu, setHarnesses, startSession, workspaceGroup } from "../helpers.ts";

const harness = 'select[aria-label="Harness"]';

describe("The New Task menu's harnesses", () => {
  afterEach(async () => {
    await setHarnesses("claude");
  });

  it("starts the one installed agent without a Harness select", async () => {
    const cwd = await startSession();
    const menu = await openMenu(cwd);
    await expect(menu.$(harness)).not.toBeExisting();
    await menu.$("button=Session in Checkout").click();
    await expect(workspaceGroup(cwd).$$("li.session")).toBeElementsArrayOfSize(2);
  });

  it("is disabled with no harness installed", async () => {
    const cwd = await startSession();
    await setHarnesses();
    await expect(workspaceGroup(cwd).$('button[aria-label="New Task"]')).toBeDisabled();
  });

  it("offers Claude and Pi while both are installed", async () => {
    const cwd = await startSession();
    await setHarnesses("claude", "pi");
    const menu = await openMenu(cwd);
    const options = menu.$(harness).$$("option");
    await expect(options).toBeElementsArrayOfSize(2);
    await expect(options[0]).toHaveText("Claude");
    await expect(options[1]).toHaveText("Pi");
    await setHarnesses("claude");
    await expect(menu).toBeDisplayed();
    await expect(menu.$(harness)).not.toBeExisting();
  });
});
