import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  defineWorkflow,
  gitWorkspace,
  openSession,
  oxctl,
  send,
  sessionEntry,
  sessionId,
  startCheckoutSession,
  startSession,
  workspaceGroup,
} from "../helpers.ts";

/** A turn of the fake agent that ends once a file `go` is in its directory. */
const held = `shell ${JSON.stringify("while [ ! -f go ]; do sleep 0.1; done")}`;

/** The dot of the session in `cwd`. */
function sessionDot(cwd: string) {
  return sessionEntry(cwd).$(".status");
}

function runRow(cwd: string) {
  return workspaceGroup(cwd).$(`./ul/li[contains(concat(" ", @class, " "), " run ")]`);
}

function childRow(cwd: string, step: string) {
  return workspaceGroup(cwd).$(
    `./ul/li[contains(concat(" ", @class, " "), " child ")][.//span[@class="title"][.="${step}"]]`,
  );
}

// The tests hold the window in front (`holdForeground`), so they dispatch
// the window's `blur` and `focus` events, which the app follows, to move it.
describe("Status dots", () => {
  it("show a turn that ended out of view until its session is viewed", async () => {
    const first = await startSession();
    const second = await startSession();
    await openSession(first);
    await send(held);
    await expect(sessionDot(first)).toHaveAttribute("class", "status running");

    await oxctl("prompt", await sessionId(second), 'text "done"');
    await expect(sessionDot(second)).toHaveAttribute("class", "status finished");
    await expect(sessionDot(second)).toHaveAttribute("aria-label", "Finished");
    // Viewing the second session clears its finish, which shows that oxd
    // has the first session out of view.
    await openSession(second);
    await expect(sessionDot(second)).toHaveAttribute("class", "status");
    await expect(sessionDot(second)).toHaveAttribute("aria-label", "Idle");

    await writeFile(join(first, "go"), "");
    await expect(sessionDot(first)).toHaveAttribute("class", "status finished");
    await openSession(first);
    await expect(sessionDot(first)).toHaveAttribute("class", "status");
  });

  it("show a turn that ended while the window was in the background", async () => {
    const cwd = await startSession();
    await openSession(cwd);
    await browser.execute(() => window.dispatchEvent(new Event("blur")));
    // The app tells oxd the window is in the background before it shows the
    // listing that follows, which shows the turn running.
    await send(held);
    await expect(sessionDot(cwd)).toHaveAttribute("class", "status running");

    await writeFile(join(cwd, "go"), "");
    await expect(sessionDot(cwd)).toHaveAttribute("class", "status finished");
    await browser.execute(() => window.dispatchEvent(new Event("focus")));
    await expect(sessionDot(cwd)).toHaveAttribute("class", "status");
  });

  it("show a failed run and its failed step until the run is viewed", async () => {
    const { cwd, workspace } = await gitWorkspace();
    await defineWorkflow(
      "broken",
      "name: Broken\nsteps:\n  - name: Break\n    agent: {harness: pi}\n    prompt: fail boom\n",
    );
    await startCheckoutSession(workspace);
    await openSession(cwd);
    await oxctl("run", "start", "--workspace", workspace, "broken", "");
    await runRow(cwd).$(".disclosure").click();

    await expect(runRow(cwd).$(".status")).toHaveAttribute("class", "status error");
    await expect(runRow(cwd).$(".status")).toHaveAttribute("aria-label", "Failed");
    await expect(childRow(cwd, "Break").$(".status")).toHaveAttribute("class", "status error");

    const select = runRow(cwd).$("button.select");
    await select.click();
    await expect(select).toHaveAttribute("aria-current", "true");
    await expect(runRow(cwd).$(".status")).toHaveAttribute("class", "status");
    await expect(childRow(cwd, "Break").$(".status")).toHaveAttribute("class", "status");
    await expect(childRow(cwd, "Break").$(".status")).toHaveAttribute("aria-label", "Failed");
  });
});
