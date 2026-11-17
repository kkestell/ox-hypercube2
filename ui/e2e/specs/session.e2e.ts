import {
  agentText,
  composer,
  openSession,
  oxctl,
  reload,
  send,
  sendButton,
  sessionEntry,
  sessionId,
  startSession,
  stopDaemon,
} from "../helpers.ts";

describe("A session", () => {
  let cwd: string;

  beforeEach(async () => {
    cwd = await startSession();
    await openSession(cwd);
  });

  it("runs a turn the user stops, named by the agent", async () => {
    await expect(sessionEntry(cwd).$(".title")).toHaveText("New session");
    await send("title Release notes\nsay 1 0\nwait");
    await expect($(".conversation .prompt .text")).toHaveText("title Release notes\nsay 1 0\nwait");
    await expect(agentText()[0]).toHaveText("chunk-1");
    await expect(sessionEntry(cwd).$(".title")).toHaveText("Release notes");
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).toBeDisplayed();
    await $('.composer button[aria-label="Stop"]').click();
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).not.toBeExisting();
    await expect(sendButton()).toBeDisplayed();
  });

  it("answers a permission request and replays it after a reload", async () => {
    await send("permission");
    await expect(sessionEntry(cwd).$('[aria-label="Awaiting permission"]')).toBeDisplayed();
    await $(".composer .request-panel").$("button=Allow").click();
    await expect(agentText()[0]).toHaveText("allowed");
    await expect($(".composer .request-panel")).not.toBeExisting();

    await reload();
    await expect($(".conversation .prompt .text")).toHaveText("permission");
    await expect(agentText()[0]).toHaveText("allowed");
    await expect($(".composer .request-panel")).not.toBeExisting();
  });

  it("answers the agent's questions", async () => {
    await send("elicit");
    const form = $(".composer .request-panel");
    await form.$("label*=Blue").click();
    await form.$("button=Next").click();
    await form.$("button=Submit").click();
    await expect(agentText()[0]).toHaveText('accepted {"color":"blue","note":"first\\nsecond"}');
    await expect(form).not.toBeExisting();
  });

  it("sets the agent's model", async () => {
    await $("button.model-picker").click();
    await $("#model-menu").$("button=Fast").click();
    await expect($("button.model-picker")).toHaveText("FastMedium");
    await send("config model");
    await expect(agentText()[0]).toHaveText("fast");
  });

  // The webview's content security policy decides whether data URLs load.
  it("shows a tool call's image", async () => {
    await send("tool listed");
    const tool = $(".conversation .tool-call");
    await tool.$("summary").click();
    await expect(tool.$("img")).toHaveProperty("naturalWidth", 1);
  });

  it("shows a turn that ended out of view until it is viewed", async () => {
    const other = await startSession();
    await oxctl("prompt", await sessionId(other), 'text "done"');
    const dot = sessionEntry(other).$(".status");
    await expect(dot).toHaveAttribute("aria-label", "Finished");
    await openSession(other);
    await expect(dot).toHaveAttribute("aria-label", "Idle");
  });

  it("queues prompts during a turn, then edits, deletes and sends them", async () => {
    await send("take");
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).toBeDisplayed();
    for (const name of ["first", "second", "third"]) await send(`text "${name}"`);
    const queue = $(".composer .queue");
    await expect(queue.$$("li")).toBeElementsArrayOfSize(3);
    const action = async (name: string, label: string) => {
      const row = queue.$(`li*=${name}`);
      await row.moveTo();
      await row.$(`button[aria-label="${label}"]`).click();
    };

    await action("first", "Edit");
    await expect(composer()).toHaveValue('text "first"');
    await send('text "edited"');
    await expect(queue.$("li*=edited")).toBeDisplayed();
    await action("second", "Delete");
    await expect(queue.$$("li")).toBeElementsArrayOfSize(2);
    // Sent now, the third is taken into the turn, which ends; the edited
    // prompt starts the next.
    await action("third", "Send Now");
    await expect(agentText()[0]).toHaveText("edited");
    await expect(queue).not.toBeExisting();
  });

  it("is viewed again after oxd restarts", async () => {
    await send("say 1 0");
    await expect(agentText()[0]).toHaveText("chunk-1");
    await stopDaemon();
    // The restarted oxd lists the session as exited and replays its history.
    await expect(sessionEntry(cwd).$('[aria-label="Exited"]')).toBeDisplayed();
    await expect(agentText()).toBeElementsArrayOfSize(1);
    await expect(agentText()[0]).toHaveText("chunk-1");
    await expect(composer()).toBeDisabled();
  });
});
