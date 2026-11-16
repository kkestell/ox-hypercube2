import { execFile } from "node:child_process";
import { mkdtemp, realpath } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";
import { agentText, binary } from "../helpers.ts";

// Far beyond everyday use: a sidebar of hundreds of sessions and a transcript
// of thousands of long Markdown replies.
const SESSIONS = 300;
const MESSAGES = 3000;
// Interactions finish, and the webview's main thread never stalls, within
// these budgets. Work proportional to the transcript or the session list on
// each change exceeds them by far at this scale.
const INTERACTION_MS = 50;
const STALL_MS = 100;
// Opening renders every reply once.
const OPEN_MS = 4000;

const reply = [
  "## Changes to `session.rs`",
  "",
  "The daemon now **records** each change once, and the watch sends only the",
  "session that changed. See [the docs](https://example.com/docs) for details.",
  "",
  "1. Read the registry",
  "2. Compare each session's version with the one last sent",
  "   - unchanged sessions are skipped",
  "   - changed ones are sent",
  "",
  "```rust",
  "fn changed(&self, sent: &mut HashMap<String, u64>) -> Vec<SessionInfo> {",
  "    self.sessions.values().filter(|s| s.version() != sent[&s.id]).collect()",
  "}",
  "```",
  "",
  "| Step | Cost |",
  "| --- | --- |",
  "| compare | O(n) integers |",
  "| send | O(changed) |",
  "",
  "> Note: a ~~full~~ listing is sent only when the watch starts.",
].join("\n");

async function oxctl(...args: string[]): Promise<string> {
  const { stdout } = await promisify(execFile)(binary("oxctl"), args, {
    maxBuffer: 64 * 1024 * 1024,
  });
  return stdout.trim();
}

/** Runs `task` on each of `items`, `limit` at a time. */
async function inParallel<T>(items: T[], limit: number, task: (item: T) => Promise<unknown>) {
  const queue = [...items];
  await Promise.all(
    Array.from({ length: limit }, async () => {
      for (let item = queue.shift(); item !== undefined; item = queue.shift()) await task(item);
    }),
  );
}

/** Waits until `oxctl ls` lists the session with `state` and, if given, `title`. */
async function waitForSession(session: string, state: string, title?: string) {
  await browser.waitUntil(
    async () =>
      (await oxctl("ls")).split("\n").some((line) => {
        const fields = line.split("\t");
        return (
          fields[0] === session &&
          fields[2] === state &&
          (title === undefined || fields[5] === title)
        );
      }),
    { timeout: 120_000, interval: 100 },
  );
}

/**
 * A workspace with `SESSIONS` exited sessions and one live session whose
 * transcript has `MESSAGES` replies, and the live session's ID and folder.
 * The long turn ends on a tool call, without a reply, so its replies stay in
 * the transcript rather than under a Worked group.
 */
async function bigWorkspace(): Promise<{ workspace: string; session: string; cwd: string }> {
  const cwd = await realpath(await mkdtemp(join(process.env.OX_E2E_DIR!, "stress-")));
  const workspace = await oxctl("workspace", "add", cwd);
  const agent = binary("oxd-fake-agent");
  await inParallel(
    Array.from({ length: SESSIONS }, (_, index) => index),
    16,
    async (index) => {
      const id = await oxctl("new", "--workspace", workspace, "--", agent);
      await oxctl("prompt", id, `title Session ${index}\nexit 0`);
      await waitForSession(id, "exited");
    },
  );
  const session = await oxctl("new", "--workspace", workspace, "--", agent);
  await oxctl(
    "prompt",
    session,
    `title Long\nmessages ${MESSAGES} ${JSON.stringify(reply)}\ntool end`,
  );
  // The title arrives once the turn is running, so idle after it ends the turn.
  await waitForSession(session, "idle", "Long");
  return { workspace, session, cwd };
}

/**
 * Views the session with the long transcript, which shares its folder with
 * the rest, and returns the milliseconds until the webview shows all of it.
 */
async function openLong(): Promise<number> {
  const select = $('//button[@class="select"][span[@class="title"][.="Long"]]');
  await select.waitForExist();
  const opened = await browser.execute(
    (select, count) =>
      new Promise<number>((done) => {
        const start = performance.now();
        const check = () => {
          if (document.querySelectorAll(".conversation > .text").length >= count) {
            done(performance.now() - start);
          } else {
            requestAnimationFrame(check);
          }
        };
        select.click();
        check();
      }),
    await select,
    MESSAGES,
  );
  await expect(select).toHaveAttribute("aria-current", "true");
  await settle();
  return opened;
}

/** Waits until the webview has laid out and painted what it rendered. */
async function settle() {
  await browser.execute(
    () =>
      new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(() => done(null)))),
  );
}

/**
 * Chooses `model` in the open model menu and returns the milliseconds until
 * the picker shows it.
 */
async function chooseModel(model: string): Promise<number> {
  return browser.execute(
    (model) =>
      new Promise<number>((done) => {
        const picker = document.querySelector("button.model-picker")!;
        const start = performance.now();
        new MutationObserver((_, observer) => {
          if (picker.textContent!.startsWith(model)) {
            observer.disconnect();
            done(performance.now() - start);
          }
        }).observe(picker, { subtree: true, childList: true, characterData: true });
        [...document.querySelectorAll<HTMLButtonElement>('#model-menu [role="option"]')]
          .find((option) => option.textContent === model)!
          .click();
      }),
    model,
  );
}

/**
 * Runs `action` and returns the longest gap between the webview's timer
 * callbacks meanwhile, in milliseconds. `action` waits through `oxctl`, since
 * finding thousands of elements through the driver occupies the webview.
 */
async function longestStall(action: () => Promise<unknown>): Promise<number> {
  await browser.execute(() => {
    const probe = { longest: 0, running: true };
    (window as unknown as { stallProbe: typeof probe }).stallProbe = probe;
    let last = performance.now();
    const tick = () => {
      const now = performance.now();
      probe.longest = Math.max(probe.longest, now - last);
      last = now;
      // biome-ignore lint/style/noRestrictedGlobals: the timer measures the webview's stalls.
      if (probe.running) setTimeout(tick, 4);
    };
    // biome-ignore lint/style/noRestrictedGlobals: the timer measures the webview's stalls.
    setTimeout(tick, 4);
  });
  await action();
  await settle();
  return browser.execute(() => {
    const probe = (window as unknown as { stallProbe: { longest: number; running: boolean } })
      .stallProbe;
    probe.running = false;
    return probe.longest;
  });
}

describe("A huge workload", function () {
  this.timeout(600_000);
  let big: { workspace: string; session: string; cwd: string };

  beforeEach(async () => {
    big = await bigWorkspace();
  });

  it("opens a long transcript and changes its model promptly", async () => {
    const opened = await openLong();
    console.log(`stress: opened ${MESSAGES} replies in ${opened.toFixed(0)} ms`);
    expect(opened).toBeLessThan(OPEN_MS);

    await $("button.model-picker").click();
    await expect($("#model-menu")).toBeDisplayed();
    for (const model of ["Fast", "Smart"]) {
      const changed = await chooseModel(model);
      console.log(`stress: changed the model to ${model} in ${changed.toFixed(1)} ms`);
      expect(changed).toBeLessThan(INTERACTION_MS);
    }
  });

  it("streams a long reply into a long transcript without stalling", async () => {
    await openLong();
    const chunk = JSON.stringify(`${reply}\n\n`);
    const stalled = await longestStall(async () => {
      await oxctl("prompt", big.session, `title Streamed\nstream 200 10 ${chunk}`);
      await waitForSession(big.session, "idle", "Streamed");
    });
    console.log(`stress: longest stall while streaming ${stalled.toFixed(1)} ms`);
    expect(stalled).toBeLessThan(STALL_MS);
    await expect(agentText()).toBeElementsArrayOfSize(MESSAGES + 1);
    await expect(agentText()[MESSAGES]!.$$("table")).toBeElementsArrayOfSize(200);
  });

  it("keeps a long transcript responsive while other sessions change", async () => {
    await openLong();
    const other = await oxctl("new", "--workspace", big.workspace, "--", binary("oxd-fake-agent"));
    const titles = Array.from({ length: 100 }, (_, index) => `title Busy ${index}\nsay 2 10`);
    const stalled = await longestStall(async () => {
      await oxctl("prompt", other, titles.join("\n"));
      await waitForSession(other, "idle", "Busy 99");
    });
    console.log(`stress: longest stall while another session changed ${stalled.toFixed(1)} ms`);
    expect(stalled).toBeLessThan(STALL_MS);
    await expect($('//span[@class="title"][.="Busy 99"]')).toBeDisplayed();
  });
});
