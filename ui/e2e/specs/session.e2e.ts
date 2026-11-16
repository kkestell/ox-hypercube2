import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  agentText,
  answerNativeDialog,
  choose,
  composer,
  deleteSessions,
  openSession,
  reload,
  send,
  sendButton,
  sessionEntry,
  startSession,
} from "../helpers.ts";

describe("A session", () => {
  let cwd: string;

  beforeEach(async () => {
    cwd = await startSession();
    await openSession(cwd);
  });

  it("is listed as a new session until the agent names it", async () => {
    await expect(sessionEntry(cwd).$(".title")).toHaveText("New session");
    await send("title Release notes");
    await expect(sessionEntry(cwd).$(".title")).toHaveText("Release notes");
  });

  it("shows the agent's reply while the turn runs", async () => {
    // The agent says one chunk, then keeps the turn open until it is stopped.
    await composer().setValue("say 1 0");
    await browser.keys(["Shift", "Enter"]);
    await composer().addValue("wait");
    await expect(composer()).toHaveValue("say 1 0\nwait");
    await send();
    await expect(agentText()[0]).toHaveText("chunk-1");
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).toBeDisplayed();
    await $('.composer button[aria-label="Stop"]').click();
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).not.toBeExisting();
    await expect(sendButton()).toBeDisplayed();
  });

  it("shows the prompt and the agent's reply", async () => {
    await send("say 3 0");
    await expect($(".conversation .prompt .text")).toHaveText("say 3 0");
    await expect(agentText()[0]).toHaveText("chunk-1chunk-2chunk-3");
    await expect(agentText()).toBeElementsArrayOfSize(1);
    await expect(composer()).toHaveValue("");
  });

  it("sends with Enter", async () => {
    await composer().setValue("say 1 0");
    await expect(sendButton()).toBeEnabled();
    await browser.keys("Enter");
    await expect(agentText()[0]).toHaveText("chunk-1");
  });

  it("keeps text and attachments when switching sessions and clears them after sending", async () => {
    const attachment = join(cwd, "notes.txt");
    await writeFile(attachment, "notes");
    await composer().setValue("say 1 0");
    await answerNativeDialog("open", [attachment]);
    await $('button[aria-label="Attach Files"]').click();
    const attached = () => $('.composer .attachments li[title$="/notes.txt"]');
    await expect(attached()).toBeDisplayed();

    const other = await startSession();
    await openSession(other);
    await expect(composer()).toHaveValue("");
    await expect(attached()).not.toBeExisting();
    await composer().setValue("other draft");

    await openSession(cwd);
    await expect(composer()).toHaveValue("say 1 0");
    await expect(attached()).toBeDisplayed();
    await send();
    await expect(agentText()[0]).toHaveText("chunk-1");
    await openSession(other);
    await expect(composer()).toHaveValue("other draft");
    await openSession(cwd);
    await expect(composer()).toHaveValue("");
    await expect(attached()).not.toBeExisting();
  });

  it("asks for permission above the composer until it is answered", async () => {
    await send("permission");
    const permission = $(".composer .request-panel");
    await expect(sessionEntry(cwd).$('[aria-label="Awaiting permission"]')).toBeDisplayed();
    await permission.$("button=Allow").click();
    await expect(agentText()[0]).toHaveText("allowed");
    await expect(permission).not.toBeExisting();
    await expect($(".conversation .request-panel")).not.toBeExisting();
    await expect(sessionEntry(cwd).$('[aria-label="Awaiting permission"]')).not.toBeExisting();

    await reload();
    await expect(agentText()[0]).toHaveText("allowed");
    await expect(permission).not.toBeExisting();
  });

  it("asks the agent's questions above the composer", async () => {
    await send("elicit");
    const form = $(".composer .request-panel");
    const awaiting = sessionEntry(cwd).$('[aria-label="Awaiting input"]');
    await expect(form.$(".title")).toHaveText("Order");
    await expect(form.$("legend=Color")).toBeDisplayed();
    await expect(form.$("p=Pick a color")).toBeDisplayed();
    await expect(form.$("label*=Blue").$(".description")).toHaveText("Cool");
    await expect(awaiting).toBeDisplayed();
    // Each question has its own page; a field without a description stays
    // with the question before it.
    await expect(form.$(".step")).toHaveText("1 of 2");
    await expect(form.$("legend=Extras")).not.toBeExisting();
    await expect(form.$("button=Back")).not.toBeExisting();
    // The colour is required, so the form stays on its page.
    await form.$("button=Next").click();
    await expect(form.$(".step")).toHaveText("1 of 2");
    await form.$("label*=Blue").click();
    await form.$("button=Next").click();
    await expect(form.$(".step")).toHaveText("2 of 2");
    await expect(form.$("legend=Color")).not.toBeExisting();
    await expect(form.$("p=Pick up to two")).toBeDisplayed();
    await form.$("label*=Nuts").click();
    const note = form.$('textarea[aria-label="Note"]');
    await expect(note).toHaveValue("first\nsecond");
    await note.setValue("edited\nsecond");
    await browser.saveScreenshot(join(process.env.OX_E2E_DIR!, "elicitation-text-fields.png"));
    // Going back keeps the answers.
    await form.$("button=Back").click();
    await expect(form.$('input[value="blue"]')).toBeSelected();
    await form.$("button=Next").click();
    await expect(form.$('input[value="nuts"]')).toBeSelected();
    await expect(awaiting).toBeDisplayed();
    await form.$("button=Submit").click();
    // Had an earlier page gone out, the agent would have answered it alone.
    await expect(agentText()[0]).toHaveText(
      'accepted {"color":"blue","extras":["nuts"],"note":"edited\\nsecond"}',
    );
    await expect(form).not.toBeExisting();
    await expect(awaiting).not.toBeExisting();
  });

  it("checks the agent's constraints before sending the answers", async () => {
    await send("elicit");
    const form = $(".composer .request-panel");
    await form.$("label*=Blue").click();
    await form.$("button=Next").click();
    await form.$('textarea[aria-label="Note"]').clearValue();
    await form.$("label*=Sprinkles").click();
    await form.$("label*=Nuts").click();
    await form.$("label*=Cherries").click();
    // At most two extras.
    await expect(form.$('input[name="extras"]:invalid')).toBeExisting();
    await form.$("button=Submit").click();
    await expect(form).toBeDisplayed();

    await form.$("label*=Sprinkles").click();
    await expect(form.$(":invalid")).not.toBeExisting();
    await form.$("button=Submit").click();
    await expect(agentText()[0]).toHaveText(
      'accepted {"color":"blue","extras":["nuts","cherries"]}',
    );
    await expect(form).not.toBeExisting();
  });

  it("declines the agent's questions", async () => {
    await send("elicit");
    const form = $(".composer .request-panel");
    await form.$("button=Decline").click();
    await expect(agentText()[0]).toHaveText("declined");
    await expect(form).not.toBeExisting();
  });

  it("withdraws the questions when the turn stops", async () => {
    await send("elicit");
    const form = $(".composer .request-panel");
    await expect(form.$(".title")).toHaveText("Order");
    await $('.composer button[aria-label="Stop"]').click();
    await expect(agentText()[0]).toHaveText("withdrawn");
    await expect(form).not.toBeExisting();
  });

  it("shows a tool call's output", async () => {
    await send("tool listed `a.rs`");
    const tool = $(".conversation .tool-call");
    await expect(tool.$(".title")).toHaveText("Fake tool listed `a.rs`");
    await expect(tool.$(".title code")).toHaveText("listed `a.rs`");
    await expect(tool.$('.status[aria-label="Completed"] svg')).toBeDisplayed();
    await tool.$("summary").click();
    await expect(tool.$("section:first-child pre")).toHaveText('{\n  "text": "listed `a.rs`"\n}');
    await expect(tool.$("section:last-child pre")).toHaveText("listed `a.rs`");
    const image = tool.$("section:last-child img");
    await expect(image).toBeDisplayed();
    await expect(image).toHaveProperty("naturalWidth", 1);
  });

  it("folds a completed turn's work under Worked", async () => {
    await send('thought "Planning"\nmessages 1 "Checking the tests"\ntool listed\ntext "Done"');
    const group = $(".conversation > .worked");
    const toggle = group.$(".worked-toggle");
    const closed = async () => {
      await expect(toggle).toHaveText("Worked");
      await expect(toggle).toHaveAttribute("aria-expanded", "false");
      const children = await browser.execute(() =>
        [...document.querySelectorAll(".conversation > *")].map((child) => child.className),
      );
      expect(children).toEqual(["prompt", "worked", "text markdown"]);
      await expect(agentText()).toBeElementsArrayOfSize(1);
      await expect(agentText()[0]).toHaveText("Done");
      await expect($(".conversation .tool-call")).not.toBeExisting();
      await expect($(".conversation .thought")).not.toBeExisting();
    };
    await toggle.waitForExist();
    await closed();

    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    await expect(group.$(".thought")).toBeDisplayed();
    await expect(group.$(".worked-items > .text")).toHaveText("Checking the tests");
    await expect(group.$(".tool-call .title")).toHaveText("Fake tool listed");

    await reload();
    await toggle.waitForExist();
    await closed();
  });

  it("keeps a turn that ends in an error expanded", async () => {
    await send('thought "Planning"\ntool listed\nfail broken');
    await expect($(".conversation .turn-error")).toHaveText("broken");
    await expect($(".conversation .worked")).not.toBeExisting();
    await expect($(".conversation > .tool-call")).toBeDisplayed();
  });

  it("stops a turn the agent started", async () => {
    await send("own-turn");
    await expect(agentText()[0]).toHaveText("own-turn");
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).toBeDisplayed();
    await $('.composer button[aria-label="Stop"]').click();
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).not.toBeExisting();
    await expect(sendButton()).toBeDisplayed();
  });

  it("sets a config option on the agent", async () => {
    const mode = '.composer select[aria-label="Mode"]';
    await expect($(mode)).toHaveValue("ask");
    await choose(mode, "code");
    await expect($(mode)).toHaveValue("code");
    await send("config mode");
    await expect(agentText()[0]).toHaveText("code");
  });

  it("replays the conversation after the webview reloads", async () => {
    await send("say 2 0");
    await expect(agentText()[0]).toHaveText("chunk-1chunk-2");
    await reload();
    await expect(sessionEntry(cwd).$("button.select")).toHaveAttribute("aria-current", "true");
    await expect($(".conversation .prompt .text")).toHaveText("say 2 0");
    await expect(agentText()[0]).toHaveText("chunk-1chunk-2");
  });

  it("reports that the agent exited and disables the composer", async () => {
    await send("exit 3");
    await expect($(".conversation .agent-exited")).toBeDisplayed();
    await expect(sessionEntry(cwd).$('[aria-label="Exited"]')).toBeDisplayed();
    await expect(composer()).toBeDisabled();
  });

  it("hides the transcript when deleted during a turn", async () => {
    await send("say 1 0\nwait");
    await expect(agentText()[0]).toHaveText("chunk-1");
    await expect(sessionEntry(cwd).$('[aria-label="Running"]')).toBeDisplayed();
    await deleteSessions(cwd);
    await expect(sessionEntry(cwd)).not.toBeExisting();
    await expect($(".conversation")).not.toBeExisting();
  });
});
