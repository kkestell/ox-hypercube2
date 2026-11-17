import { sessionButton, startSession, stopDaemon, workspace } from "../helpers.ts";

it("reconnects and restores the open conversation after oxd exits", async () => {
  const cwd = await workspace();
  await startSession(cwd, 'text "hello"');
  const reply = $(".conversation > .text");
  await expect(reply).toHaveText("hello");
  await expect($('.composer button[aria-label="Send"]')).toBeDisplayed();

  expect(stopDaemon()).toBe(true);

  await expect(sessionButton(cwd, "li.session.exited")).toHaveAttribute("aria-current", "true");
  await expect(reply).toHaveText("hello");
});

it("loads an agent's tool image in the webview", async () => {
  await startSession(await workspace(), "tool listed");
  const tool = $(".conversation .tool-call");
  await tool.$("summary").click();
  await expect(tool.$("img")).toHaveProperty("naturalWidth", 1);
});
