import {
  agentText,
  composer,
  deleteSessions,
  openSession,
  send,
  sessionEntry,
  slide,
  startSession,
} from "../helpers.ts";

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
    const paperclip = $('.composer button[aria-label="Attach Files"] svg');
    expect(await picker().$(".claude-logo").getLocation("x")).toBe(
      await paperclip.getLocation("x"),
    );
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
