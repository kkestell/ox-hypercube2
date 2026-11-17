import { defineWorkflow, gitWorkspace, openMenu, rowAction, workspaceGroup } from "../helpers.ts";

function runRow(cwd: string) {
  return workspaceGroup(cwd).$(`./ul/li[contains(concat(" ", @class, " "), " run ")]`);
}

/** Step `name` in the run view, with the status `label`. */
function stepStatus(name: string, label: string) {
  return $(`.run-status-step[aria-label="${name}"] .status[aria-label="${label}"]`);
}

// A test that fails with a dialog open leaves it to the next test.
afterEach(async () => {
  while (await $("dialog[open]").isExisting()) await browser.keys("Escape");
});

describe("A workflow run", () => {
  it("starts from the New Session menu, takes a step's answer, and is deleted", async () => {
    const { cwd } = await gitWorkspace();
    // Agent steps on the `pi` harness run the fake agent; their prompt and the
    // run's input are its script.
    await defineWorkflow(
      "ask",
      `name: Ask
steps:
  - name: Ask
    agent: {harness: pi}
    prompt: permission
  - name: Build
    shell: "true"
`,
    );
    await (await openMenu(cwd)).$("button=Ask").click();
    const dialog = $("dialog[open]");
    await dialog.$("textarea").setValue('text "Fixed"');
    await dialog.$("button=Run").click();
    await expect(runRow(cwd).$(".title")).toHaveText('text "Fixed"');
    await expect(runRow(cwd).$("button.select")).toHaveAttribute("aria-current", "true");

    await $(".composer .request-panel").$('button[data-kind="allow_once"]').click();
    await expect(stepStatus("Ask", "Succeeded")).toBeExisting();
    await expect(stepStatus("Build", "Succeeded")).toBeExisting();
    await expect(runRow(cwd).$('.status[aria-label="Succeeded"]')).toBeExisting();

    await rowAction(runRow(cwd), "Delete Run");
    await $("dialog[open]").$("button=Discard and Delete").click();
    await expect(runRow(cwd)).not.toBeExisting();
  });
});
