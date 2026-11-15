import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  composer,
  defineWorkflow,
  gitWorkspace,
  openMenu,
  rowAction,
  send,
  startCheckoutSession,
  workspaceGroup,
} from "../helpers.ts";

/** A fake agent directive that takes a JSON string, such as `text "done"`. */
function directive(name: string, text: string) {
  return `${name} ${JSON.stringify(text)}`;
}

/** `text` as a YAML double-quoted scalar, which JSON's strings are. */
function yaml(text: string) {
  return JSON.stringify(text);
}

/** An agent step of the fake agent, which runs `prompt` as its script. */
function agentStep(name: string, prompt: string, checks = "") {
  return `  - name: ${name}\n    agent: {harness: pi}\n    prompt: ${yaml(prompt)}\n${checks}`;
}

/** A row of the workspace in `cwd` whose class list holds `kind`. */
function rowsOf(cwd: string, kind: "run" | "child") {
  return workspaceGroup(cwd).$$(`./ul/li[contains(concat(" ", @class, " "), " ${kind} ")]`);
}

function runRow(cwd: string) {
  return workspaceGroup(cwd).$(`./ul/li[contains(concat(" ", @class, " "), " run ")]`);
}

function childRow(cwd: string, step: string) {
  return workspaceGroup(cwd).$(
    `./ul/li[contains(concat(" ", @class, " "), " child ")][.//span[@class="title"][.="${step}"]]`,
  );
}

/** The run view's section of step `name`. */
function stepSection(name: string) {
  return $(`.conversation section.step[aria-label="${name}"]`);
}

/** The agent's first text item in step `name`'s section. */
function replyIn(name: string) {
  return stepSection(name).$('./div[contains(concat(" ", @class, " "), " text ")]');
}

/** The request panel below the run view's or the session view's transcript. */
function requestPanel() {
  return $(".composer .request-panel");
}

/** Chooses workflow `name` from the New Task menu and runs it with `input`. */
async function startRun(cwd: string, name: string, input: string) {
  await (await openMenu(cwd)).$(`button=${name}`).click();
  const dialog = $("dialog[open]");
  await expect(dialog.$("h2")).toHaveText(name);
  await dialog.$("textarea").setValue(input);
  await dialog.$("button=Run").click();
  await expect(dialog).not.toBeExisting();
  await expect(runRow(cwd).$("button.select")).toHaveAttribute("aria-current", "true");
}

async function viewRun(cwd: string) {
  const select = runRow(cwd).$("button.select");
  await select.click();
  await expect(select).toHaveAttribute("aria-current", "true");
}

async function viewChild(cwd: string, step: string) {
  const select = childRow(cwd, step).$("button.select");
  await select.click();
  await expect(select).toHaveAttribute("aria-current", "true");
}

/** What a step section shows, in order: `prompt`, or a command's heading and status. */
async function stepOrder(name: string): Promise<string[]> {
  return browser.execute(
    (section) =>
      [...section.children].flatMap((child) => {
        if (child.classList.contains("prompt")) return ["prompt"];
        if (!child.classList.contains("command")) return [];
        const heading = child.querySelector(".title")!.firstChild!.textContent;
        return [`${heading} ${child.querySelector(".status")!.getAttribute("aria-label")}`];
      }),
    await stepSection(name),
  );
}

// A test that fails with a dialog open leaves it to the next test.
afterEach(async () => {
  while (await $("dialog[open]").isExisting()) await browser.keys("Escape");
});

describe("A workflow run", () => {
  it("starts from the New Task menu and follows its steps", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await startCheckoutSession(workspace);
    await defineWorkflow(
      "follow",
      `name: Follow
steps:
${agentStep("First", directive("shell", "while [ ! -f go ]; do sleep 0.1; done"))}  - name: Build
    shell: "true"
${agentStep("Second", directive("text", "second"), '    postconditions:\n      - shell: "true"\n')}`,
    );
    await startRun(cwd, "Follow", '  text "Fix   the build"\n');
    await startCheckoutSession(workspace);

    await expect(runRow(cwd).$(".title")).toHaveText('text "Fix the build"');
    await expect(childRow(cwd, "First")).toBeExisting();
    await expect(rowsOf(cwd, "child")).toBeElementsArrayOfSize(1);
    await expect(childRow(cwd, "First").$(".tree-branch.first.last")).toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Running"]')).toBeExisting();
    const worktree = (await childRow(cwd, "First").$("button.select").getAttribute("title"))!;
    await writeFile(join(worktree.replace(/\/$/, ""), "go"), "");

    await expect(childRow(cwd, "Second")).toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Succeeded"]')).toBeExisting();
    const kinds = await browser.execute(
      (list) =>
        [...list.children].map((row) =>
          row.classList.contains("run")
            ? "run"
            : row.classList.contains("child")
              ? "child"
              : "session",
        ),
      await workspaceGroup(cwd).$("ul"),
    );
    expect(kinds).toEqual(["session", "run", "child", "child", "session"]);
    await expect(childRow(cwd, "First").$(".tree-branch.first")).toBeExisting();
    await expect(childRow(cwd, "Second").$(".tree-branch.last")).toBeExisting();
    await expect(runRow(cwd).$(".status")).toHaveAttribute("class", "status succeeded");
    for (const step of ["First", "Second"]) {
      await expect(childRow(cwd, step).$(".status")).toHaveAttribute(
        "class",
        "status hollow succeeded",
      );
    }

    await expect($$(".conversation .step-heading")).toHaveText(["First", "Build", "Second"]);
    await expect(stepSection("First").$(".prompt")).toHaveText(
      expect.stringContaining("while [ ! -f go ]"),
    );
    await expect(stepSection("Build").$(".tool-call .title")).toHaveText(
      expect.stringMatching(/^Command/),
    );
    await expect(stepSection("Build").$(".tool-call .title code")).toHaveText("true");
    expect(await stepOrder("Second")).toEqual(["prompt", "Postcondition Succeeded"]);
    await browser.saveScreenshot(join(process.env.OX_E2E_DIR!, "run-view.png"));
  });

  it("takes an answer to a child's request from the run view or the child's view", async () => {
    const { cwd } = await gitWorkspace();
    await defineWorkflow(
      "ask",
      `name: Ask
steps:
${agentStep("First", "permission")}  - name: Between
    shell: "true"
${agentStep("Second", "permission")}`,
    );
    await startRun(cwd, "Ask", "");
    await expect(runRow(cwd).$(".title")).toHaveText("Ask");

    await requestPanel().$('button[data-kind="allow_once"]').click();
    await expect(replyIn("First")).toHaveText("allowed");

    await expect(childRow(cwd, "Second")).toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Awaiting permission"]')).toBeExisting();
    await viewChild(cwd, "Second");
    await requestPanel().$('button[data-kind="allow_once"]').click();
    await expect($(".conversation > .text")).toHaveText("allowed");

    await viewRun(cwd);
    await expect(replyIn("Second")).toHaveText("allowed");
    await expect(requestPanel()).not.toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Succeeded"]')).toBeExisting();
  });

  it("shows a repair turn between the postconditions it fixed", async () => {
    const { cwd } = await gitWorkspace();
    const repair = `${directive("shell", "touch fixed")}\n${directive("text", "fixed it")}`;
    await defineWorkflow(
      "repair",
      `name: Repair
steps:
${agentStep(
  "Fix",
  `${directive("next", repair)}\n${directive("text", "done")}`,
  "    postconditions:\n      - shell: test -f fixed\n",
)}`,
    );
    await startRun(cwd, "Repair", "");
    await expect(runRow(cwd).$('.status[aria-label="Succeeded"]')).toBeExisting();
    expect(await stepOrder("Fix")).toEqual([
      "prompt",
      "Postcondition Failed",
      "prompt",
      "Postcondition Succeeded",
    ]);
  });

  it("disables a child's entry while it runs, and takes prompts once it has ended", async () => {
    const { cwd } = await gitWorkspace();
    await defineWorkflow("hold", `name: Hold\nsteps:\n${agentStep("Hold", "wait")}`);
    await startRun(cwd, "Hold", "");
    await viewChild(cwd, "Hold");
    await expect(composer()).toBeDisabled();
    await expect($('button[aria-label="Attach Files"]')).toBeDisabled();
    await $('.composer button[aria-label="Stop"]').click();
    await expect(runRow(cwd).$('.status[aria-label="Cancelled"]')).toBeExisting();

    await expect(composer()).toBeEnabled();
    await send(directive("text", "after"));
    await expect(
      $('//div[@class="conversation"]/div[contains(@class, "text")][last()]'),
    ).toHaveText("after");
    await viewRun(cwd);
    await expect(stepSection("Hold").$("./*[last()]")).toHaveText("after");

    await rowAction(runRow(cwd), "Delete Run");
    await $("dialog[open]").$("button=Discard and Delete").click();
    await expect(runRow(cwd)).not.toBeExisting();
    await expect(rowsOf(cwd, "child")).toBeElementsArrayOfSize(0);
  });

  it("shows a failed precondition with its output and the run's error", async () => {
    const { cwd } = await gitWorkspace();
    await defineWorkflow(
      "guarded",
      `name: Guarded
steps:
${agentStep("Guarded", directive("text", "never"), "    preconditions:\n      - shell: echo not ready; exit 3\n")}`,
    );
    await startRun(cwd, "Guarded", "");
    await expect(runRow(cwd).$('.status[aria-label="Failed"]')).toBeExisting();
    await expect(rowsOf(cwd, "child")).toBeElementsArrayOfSize(0);

    const check = stepSection("Guarded").$(".command");
    await expect(check.$('.status[aria-label="Failed"]')).toBeExisting();
    await check.$("summary").click();
    await expect(check.$(".details")).toHaveText(expect.stringContaining("Exited with status 3"));
    await expect(check.$(".details")).toHaveText(expect.stringContaining("not ready"));
    await expect($(".conversation > .turn-error")).toHaveText(
      expect.stringContaining("a precondition exited with status 3"),
    );
  });
});
