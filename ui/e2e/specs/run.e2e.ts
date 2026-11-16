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

/** The run children's rows of the workspace in `cwd`. */
function childRows(cwd: string) {
  return workspaceGroup(cwd).$$(`./ul/li[contains(concat(" ", @class, " "), " child ")]`);
}

function runRow(cwd: string) {
  return workspaceGroup(cwd).$(`./ul/li[contains(concat(" ", @class, " "), " run ")]`);
}

function childRow(cwd: string, step: string) {
  return workspaceGroup(cwd).$(
    `./ul/li[contains(concat(" ", @class, " "), " child ")][.//span[@class="title"][.="${step}"]]`,
  );
}

/** Step `name` in the run view's list of steps. */
function statusStep(name: string) {
  return $(`.run-status-step[aria-label="${name}"]`);
}

/** A time as the run view shows it, `HH:MM`. */
const clockText = /^\d{2}:\d{2}$/;

/** The status icon of step `name` in the run view, labelled `label`. */
function stepStatus(name: string, label: string) {
  return statusStep(name).$(`.status[aria-label="${label}"]`);
}

/** The last text item of the session view's conversation. */
function lastReply() {
  return $('//div[@class="conversation"]/div[contains(@class, "text")][last()]');
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
    await expect(childRows(cwd)).toBeElementsArrayOfSize(3);
    await expect(childRow(cwd, "First").$(".tree-branch.first")).toBeExisting();
    await expect(childRow(cwd, "Second").$(".tree-branch.last")).toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Running"]')).toBeExisting();
    const worktree = (await childRow(cwd, "First").$("button.select").getAttribute("title"))!;
    await writeFile(join(worktree.replace(/\/$/, ""), "go"), "");

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
    expect(kinds).toEqual(["session", "run", "child", "child", "child", "session"]);
    await expect(childRow(cwd, "First").$(".tree-branch.first")).toBeExisting();
    await expect(childRow(cwd, "Second").$(".tree-branch.last")).toBeExisting();
    await expect(runRow(cwd).$(".status")).toHaveAttribute("class", "status succeeded");
    for (const step of ["First", "Build", "Second"]) {
      await expect(childRow(cwd, step).$(".status")).toHaveAttribute("class", "status succeeded");
    }

    await expect($$(".run-status-step .title")).toHaveText(["First", "Build", "Second"]);
    for (const step of ["First", "Build", "Second"]) {
      await expect(stepStatus(step, "Succeeded")).toBeExisting();
      await expect(statusStep(step).$(".run-status-time")).toHaveText(clockText);
    }
    await browser.saveScreenshot(join(process.env.OX_E2E_DIR!, "run-view.png"));
  });

  it("shows each started step's start time and its agent's status updates", async () => {
    const { cwd } = await gitWorkspace();
    const report = [
      "mcp-status Reading the code",
      "mcp-status Writing the fix",
      directive("shell", "while [ ! -f go ]; do sleep 0.1; done"),
    ].join("\n");
    await defineWorkflow(
      "report",
      `name: Report
steps:
${agentStep("Report", report)}  - name: Later
    shell: "true"
`,
    );
    await startRun(cwd, "Report", "");

    const updates = ["Reading the code", "Writing the fix"];
    await expect(statusStep("Report").$$(".run-status-updates li")).toBeElementsArrayOfSize(2);
    await expect(statusStep("Report").$$(".run-status-updates li p")).toHaveText(updates);
    await expect(statusStep("Report").$$(".run-status-updates li time")).toHaveText([
      clockText,
      clockText,
    ]);
    await expect(statusStep("Report").$(".run-status-time")).toHaveText(clockText);
    await expect(statusStep("Later").$(".run-status-time")).not.toBeExisting();

    const worktree = (await childRow(cwd, "Report").$("button.select").getAttribute("title"))!;
    await writeFile(join(worktree.replace(/\/$/, ""), "go"), "");
    await expect(runRow(cwd).$('.status[aria-label="Succeeded"]')).toBeExisting();
    await expect(statusStep("Report").$(".run-status-updates")).not.toBeExisting();
    await statusStep("Report").$(".run-status-toggle").click();
    await expect(statusStep("Report").$$(".run-status-updates li p")).toHaveText(updates);
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
    await expect(stepStatus("First", "Succeeded")).toBeExisting();

    await expect(runRow(cwd).$('.status[aria-label="Awaiting permission"]')).toBeExisting();
    await viewChild(cwd, "Second");
    await requestPanel().$('button[data-kind="allow_once"]').click();
    await expect($(".conversation > .text")).toHaveText("allowed");

    await viewRun(cwd);
    await expect(stepStatus("Second", "Succeeded")).toBeExisting();
    await expect(requestPanel()).not.toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Succeeded"]')).toBeExisting();
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
    await expect(lastReply()).toHaveText("after");
    await viewRun(cwd);
    await expect(stepStatus("Hold", "Cancelled")).toBeExisting();

    await rowAction(runRow(cwd), "Delete Run");
    await $("dialog[open]").$("button=Discard and Delete").click();
    await expect(runRow(cwd)).not.toBeExisting();
    await expect(childRows(cwd)).toBeElementsArrayOfSize(0);
  });

  it("shows a failed precondition's error under its step", async () => {
    const { cwd } = await gitWorkspace();
    await defineWorkflow(
      "guarded",
      `name: Guarded
steps:
${agentStep("Guarded", directive("text", "never"), "    preconditions:\n      - shell: echo not ready; exit 3\n")}`,
    );
    await startRun(cwd, "Guarded", "");
    await expect(runRow(cwd).$('.status[aria-label="Failed"]')).toBeExisting();
    await expect(childRow(cwd, "Guarded").$('.status[aria-label="Failed"]')).toBeExisting();

    await expect(stepStatus("Guarded", "Failed")).toBeExisting();
    await expect(statusStep("Guarded").$(".run-status-error")).toHaveText(
      expect.stringContaining("a precondition exited with status 3"),
    );
  });
});
