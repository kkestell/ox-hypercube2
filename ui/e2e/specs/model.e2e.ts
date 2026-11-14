import { execFile } from "node:child_process";
import { mkdtemp, realpath, symlink } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";
import {
  agentText,
  binary,
  composer,
  deleteSessions,
  openSession,
  send,
  sessionEntry,
  slide,
  startSession,
} from "../helpers.ts";

/** Runs the scripted agent under a real bridge's name to exercise harness identity. */
async function startHarnessSession(bridge: string): Promise<string> {
  const cwd = await realpath(await mkdtemp(join(process.env.OX_E2E_DIR!, "harness-")));
  const command = join(cwd, bridge);
  await symlink(binary("oxd-fake-agent"), command);
  const run = promisify(execFile);
  const { stdout } = await run(binary("oxctl"), ["workspace", "add", cwd]);
  await run(binary("oxctl"), ["new", "--workspace", stdout.trim(), "--", command]);
  return cwd;
}

const picker = () => $("button.model-picker");
const menu = () => $("#model-menu");
const search = () => menu().$('input[aria-label="Search models"]');
const options = () => menu().$$('[role="option"]');
const sliderSelector = '#model-menu input[type="range"]';
const slider = () => $(sliderSelector);

describe("The model picker", () => {
  let cwd: string;

  beforeEach(async () => {
    cwd = await startSession();
    await openSession(cwd);
  });

  it("shows the session's model and effort", async () => {
    await expect(picker()).toHaveText("SmartMedium");
    await expect($(".composer .harness-logo")).not.toBeExisting();
  });

  it("shows providers in a muted column and updates the selected provider", async () => {
    await openSession(await startSession("--model-providers"));
    await expect(picker().$(".provider-name")).toHaveText("OpenAI subscription");
    await picker().click();
    const providers = menu().$$(".provider-name");
    await expect(providers[0]!).toHaveText("OpenRouter");
    await expect(providers[1]!).toHaveText("OpenAI subscription");
    expect(await providers[0]!.getLocation("x")).toBe(await providers[1]!.getLocation("x"));
    const colors = await browser.execute(() => {
      const row = document.querySelector('#model-menu [role="option"]')!;
      return {
        model: getComputedStyle(row.firstElementChild!).color,
        provider: getComputedStyle(row.lastElementChild!).color,
        muted: getComputedStyle(document.querySelector(".effort .stops")!).color,
      };
    });
    expect(colors.provider).toBe(colors.muted);
    expect(colors.provider).not.toBe(colors.model);
    await menu().$("#model-fast").click();
    await expect(picker().$(".provider-name")).toHaveText("OpenRouter");
    await expect(picker().$("span")).toHaveText("Fast");
    await browser.saveScreenshot(join(process.env.OX_E2E_DIR!, "model-providers.png"));
    await send("config model");
    await expect(agentText()[0]).toHaveText("fast");
  });

  it("shows the harness beside the selected model and leaves model options icon-free", async () => {
    for (const [bridge, label] of [
      ["claude-code-acp", "Claude Code"],
      ["pi-acp", "Pi"],
    ] as const) {
      await openSession(await startHarnessSession(bridge));
      const logo = $(`.composer .harness-logo[aria-label="${label}"]`);
      await expect(logo).toBeDisplayed();
      await expect(picker()).toHaveText("SmartMedium");
      const paperclip = $('.composer button[aria-label="Attach Files"] svg');
      expect(await logo.getLocation("x")).toBe(await paperclip.getLocation("x"));
      expect(await logo.getSize()).toEqual(await paperclip.getSize());
      await picker().click();
      await expect(menu()).toBeDisplayed();
      await expect(menu().$$("[role='option'] svg")).toBeElementsArrayOfSize(0);
      await menu().$("button=Fast").click();
      await expect(picker()).toHaveText("FastMedium");
      await expect(logo).toBeDisplayed();
      await browser.saveScreenshot(join(process.env.OX_E2E_DIR!, `${bridge}-model-menu.png`));
    }
    await openSession(cwd);
    await expect($(".composer .harness-logo")).not.toBeExisting();
  });

  it("reserves the settings row before the agent reports its configuration", async () => {
    // oxctl waits for startup; view the published Starting row while it waits.
    const opening = startSession("--hang-initialize").catch((error: unknown) => error);
    const row = $(".session:has(.status.starting) button.select");
    await expect(row).toBeDisplayed();
    const starting = (await row.getAttribute("title"))!;
    await openSession(starting);
    await expect(sessionEntry(starting).$('[aria-label="Starting"]')).toBeDisplayed();
    await expect($(".composer .settings")).toHaveText("");
    const before = await composer().getLocation("y");
    await openSession(cwd);
    await expect(picker()).toBeDisplayed();
    expect(await composer().getLocation("y")).toBe(before);
    await deleteSessions(starting);
    expect(await opening).toBeInstanceOf(Error);
  });

  it("omits search for a short list and chooses with the keyboard, staying open", async () => {
    await picker().click();
    await expect(menu()).toBeDisplayed();
    await expect(search()).not.toBeExisting();
    await expect(options()).toBeElementsArrayOfSize(2);
    await expect(menu().$('[role="option"][aria-selected="true"]')).toHaveText("Smart");
    await expect(menu().$('[role="option"][aria-selected="true"]')).toBeFocused();
    await browser.keys("ArrowUp");
    await browser.keys("Enter");
    await expect(picker()).toHaveText("FastMedium");
    await expect(menu()).toBeDisplayed();
    await expect(menu().$('[role="option"][aria-selected="true"]')).toHaveText("Fast");
    await send("config model");
    await expect(agentText()[0]).toHaveText("fast");
  });

  it("shows a chosen model at once, before the agent reports it", async () => {
    await picker().click();
    await expect(menu()).toBeDisplayed();
    // React commits a click's update in a microtask the click queues. The
    // label is read in the microtask after it, before any task, and so before
    // any answer from the agent.
    const shown = await browser.execute(
      () =>
        new Promise<string | null>((done) => {
          [...document.querySelectorAll<HTMLButtonElement>('#model-menu [role="option"]')]
            .find((option) => option.textContent === "Fast")!
            .click();
          queueMicrotask(() => done(document.querySelector("button.model-picker")!.textContent));
        }),
    );
    expect(shown).toBe("FastMedium");
    await send("config model");
    await expect(agentText()[0]).toHaveText("fast");
    await expect(picker()).toHaveText("FastMedium");
  });

  it("chooses the hovered model with Enter", async () => {
    await picker().click();
    await expect(menu().$('[aria-selected="true"]')).toBeFocused();
    const fast = menu().$("button=Fast");
    // The embedded driver moves the pointer without sending mouseover.
    await browser.execute(
      (element) => {
        element.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
      },
      await fast,
    );
    await expect(fast).toBeFocused();
    await browser.keys("Enter");
    await expect(picker()).toHaveText("FastMedium");
    await send("config model");
    await expect(agentText()[0]).toHaveText("fast");
  });

  it("sets the effort with the slider", async () => {
    await picker().click();
    await expect(slider()).toHaveValue("1");
    await slide(sliderSelector, 2);
    await expect(slider()).toHaveValue("2");
    await expect(picker()).toHaveText("SmartHigh");
    await send("config effort");
    await expect(agentText()[0]).toHaveText("high");
  });
});
