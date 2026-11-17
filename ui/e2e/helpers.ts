import { execFile } from "node:child_process";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import type { ChainablePromiseElement } from "webdriverio";

const target = resolve(import.meta.dirname, "../../target/debug");

/** The path of a binary as the workspace builds it. */
export function built(name: string): string {
  return join(target, name);
}

/** The directory the run's app, oxd, oxctl and agents run from. */
export function binDir(): string {
  return join(target, "e2e-bin");
}

/** The path of a binary the run uses, in `binDir()`. */
export function binary(name: string): string {
  return join(binDir(), name);
}

/**
 * Saves `yaml` as the definition of workflow `name` in oxd's workflows
 * directory. Agent steps on the `pi` harness run the fake agent, whose
 * prompt, with the step's input after it, is its script.
 */
export async function defineWorkflow(name: string, yaml: string) {
  await mkdir(workflowsDir(), { recursive: true });
  await writeFile(join(workflowsDir(), `${name}.yaml`), yaml);
}

/** oxd's workflows directory, in the run's directory. */
function workflowsDir(): string {
  return join(process.env.XDG_CONFIG_HOME!, "ox/workflows");
}

/** The directory of harness stubs on the app's `PATH`. */
export function harnesses(): string {
  return join(process.env.OX_E2E_DIR!, "harnesses");
}

/**
 * Adds a new folder as a workspace with `oxctl`, starts a session of the
 * scripted fake agent in it, and returns the folder's path, which the
 * session's entry shows on hover. Each line of a prompt to the fake agent is a
 * directive, such as `say 3 0`; its module documentation lists them.
 */
export async function startSession(...agentArgs: string[]): Promise<string> {
  const cwd = await realpath(await mkdtemp(join(process.env.OX_E2E_DIR!, "session-")));
  const workspace = (await oxctl("workspace", "add", cwd)).trim();
  await oxctl("new", "--workspace", workspace, "--", binary("oxd-fake-agent"), ...agentArgs);
  return cwd;
}

/**
 * Adds a new Git repository, with `main` at one commit, as a workspace, and
 * returns its path and workspace ID.
 */
export async function gitWorkspace(): Promise<{ cwd: string; workspace: string }> {
  const cwd = await tempDir("repo-");
  await git(cwd, "init", "-b", "main");
  await commit(cwd);
  return { cwd, workspace: await addWorkspace(cwd) };
}

/**
 * Starts a session of the fake agent in a new worktree of `workspace`, and
 * returns the worktree's path.
 */
export async function startWorktreeSession(workspace: string): Promise<string> {
  const id = (
    await oxctl("new", "--workspace", workspace, "--worktree", "--", binary("oxd-fake-agent"))
  ).trim();
  const listed = lines(await oxctl("ls")).find((line) => line.startsWith(`${id}\t`));
  return listed!.split("\t")[3]!.replace(/\/$/, "");
}

/** Runs git in `cwd` and returns its output. */
export async function git(cwd: string, ...args: string[]): Promise<string> {
  const { stdout } = await promisify(execFile)("git", args, { cwd });
  return stdout;
}

/**
 * Commits a change to a new file in `cwd`, with an identity of its own, and
 * returns the file's name.
 */
export async function commit(cwd: string): Promise<string> {
  const name = `${Date.now()}-${Math.random()}.txt`;
  await writeFile(join(cwd, name), "change\n");
  await git(cwd, "add", ".");
  await git(
    cwd,
    ...["-c", "user.name=Ox", "-c", "user.email=ox@example.com", "-c", "commit.gpgsign=false"],
    ...["commit", "-m", "change"],
  );
  return name;
}

async function tempDir(prefix: string): Promise<string> {
  return realpath(await mkdtemp(join(process.env.OX_E2E_DIR!, prefix)));
}

async function addWorkspace(cwd: string): Promise<string> {
  return (await oxctl("workspace", "add", cwd)).trim();
}

/** The ID of the one session in `cwd`, as `oxctl ls` lists it. */
export async function sessionId(cwd: string): Promise<string> {
  const ids = lines(await oxctl("ls"))
    .map((line) => line.split("\t"))
    .filter(([, , , path]) => path?.replace(/\/$/, "") === cwd)
    .map(([id]) => id!);
  if (ids.length !== 1) throw new Error(`${cwd} has ${ids.length} sessions`);
  return ids[0]!;
}

/**
 * Compares the one session of `workspace` again with `oxctl compare`, and
 * returns what it prints: the ref, the branch's commit, the ref's commit, and
 * the commits ahead and behind, separated by tabs.
 */
export async function compareSession(workspace: string, cwd?: string): Promise<string> {
  const sessions = lines(await oxctl("ls")).filter((line) => {
    const [, id, , path] = line.split("\t");
    return id === workspace && (cwd === undefined || path?.replace(/\/$/, "") === cwd);
  });
  if (sessions.length !== 1) throw new Error(`${workspace} has ${sessions.length} sessions`);
  return oxctl("compare", sessions[0]!.split("\t")[0]!);
}

/**
 * Removes every run oxd has, cancelling a running one and waiting for it to
 * end first, then deletes every session, removes every workspace, and
 * removes the workflow definitions.
 */
export async function resetDaemon() {
  for (const line of lines(await oxctl("run", "ls"))) {
    const [id, state] = line.split("\t");
    if (state === "running") {
      await oxctl("run", "cancel", id!);
      // `run wait` fails for a run that did not succeed.
      await oxctl("run", "wait", id!).catch(() => {});
    }
    await oxctl("run", "rm", "--discard", id!);
  }
  for (const line of lines(await oxctl("ls"))) {
    await oxctl("delete", "--discard", line.split("\t")[0]!);
  }
  for (const line of lines(await oxctl("workspace", "ls"))) {
    await oxctl("workspace", "rm", line.split("\t")[0]!);
  }
  await rm(workflowsDir(), { recursive: true, force: true });
}

/**
 * Stops the run's oxd. It runs as `oxd --socket <OX_SOCKET>`, as the client
 * starts it.
 */
export async function stopDaemon() {
  const socket = process.env.OX_SOCKET!.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await promisify(execFile)("pkill", ["-TERM", "-f", `oxd --socket ${socket}$`]);
}

function lines(output: string): string[] {
  return output.split("\n").filter(Boolean);
}

/** Runs `oxctl` with `args` and returns what it prints. */
export async function oxctl(...args: string[]): Promise<string> {
  const { stdout } = await promisify(execFile)(binary("oxctl"), args);
  return stdout;
}

/** The session list's group for the workspace in `cwd`. */
export function workspaceGroup(cwd: string) {
  return $(`//section[@class="workspace"][.//h2[@title="${cwd}"]]`);
}

/** The session list's entry for the session in `cwd`. */
export function sessionEntry(cwd: string) {
  return $(`//li[contains(@class, "session")][button[@class="select"][@title="${cwd}"]]`);
}

/** The session list's entry for the one session in a worktree. */
export function worktreeEntry() {
  const worktrees = join(process.env.OX_E2E_DIR!, "worktrees/");
  return $(
    `//li[contains(@class, "session")][button[@class="select"][starts-with(@title, "${worktrees}")]]`,
  );
}

/** Opens the New Session menu of the workspace in `cwd`. */
export async function openMenu(cwd: string) {
  await workspaceGroup(cwd).$('button[aria-label="New Session"]').click();
  const menu = workspaceGroup(cwd).$('[role="menu"]');
  await expect(menu).toBeDisplayed();
  return menu;
}

/** Starts a Session in Worktree in the workspace in `cwd`, and returns its worktree's path. */
export async function newWorktreeSession(cwd: string): Promise<string> {
  await (await openMenu(cwd)).$("button=Session in Worktree").click();
  const select = worktreeEntry().$("button.select");
  await expect(select).toHaveAttribute("aria-current", "true");
  return (await select.getAttribute("title"))!.replace(/\/$/, "");
}

/** Views the session in `cwd`. */
export async function openSession(cwd: string) {
  const select = sessionEntry(cwd).$("button.select");
  await select.click();
  await expect(select).toHaveAttribute("aria-current", "true");
}

export function composer() {
  return $(".composer textarea");
}

export function sendButton() {
  return $('.composer button[aria-label="Send"]');
}

/** Sends `text`, or what the composer holds, once the session can take a prompt. */
export async function send(text?: string) {
  if (text !== undefined) await composer().setValue(text);
  await expect(sendButton()).toBeEnabled();
  await sendButton().click();
}

/**
 * Reloads the webview and waits for the new document, which has none of the
 * old document's globals. `browser.refresh()` can return while the old
 * document is still loaded.
 */
export async function reload() {
  await browser.execute(() => {
    (window as unknown as { previous?: boolean }).previous = true;
  });
  await browser.refresh();
  await browser.waitUntil(() =>
    browser.execute(() => !(window as unknown as { previous?: boolean }).previous),
  );
  await holdForeground();
}

/**
 * Keeps the app's window in front for the tests, whichever app the desktop
 * puts in front: the window's own `focus` and `blur` events stop before the
 * app sees them, and a dispatched `focus` puts the window in front. Tests
 * dispatch `blur` and `focus` themselves to move it.
 */
export async function holdForeground() {
  await browser.execute(() => {
    for (const type of ["focus", "blur"]) {
      window.addEventListener(
        type,
        (event) => {
          if (event.isTrusted) event.stopImmediatePropagation();
        },
        true,
      );
    }
    window.dispatchEvent(new Event("focus"));
  });
}

/** The agent's text items in the viewed conversation. */
export function agentText() {
  return $$(".conversation > .text");
}

/**
 * Chooses `text` from the native context menu of the sidebar row `row`, and
 * returns whether each of the menu's items was enabled. WebDriver cannot
 * choose a native menu item, so this dispatches the row's contextmenu event
 * and delivers the choice through Tauri's menu channel instead of showing the
 * native popup. Menu creation and the row's action still run normally.
 */
export async function rowAction(
  row: ChainablePromiseElement,
  text: string,
): Promise<Record<string, boolean>> {
  return browser.execute(
    async (element, text) => {
      type MenuItem = { text: string; enabled: boolean; handler: string };
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            runCallback: (id: number, data: { index: number; message: string }) => void;
          };
        }
      ).__TAURI_INTERNALS__;
      const fetch = window.fetch;
      try {
        return await new Promise<Record<string, boolean>>((resolve, reject) => {
          let item: MenuItem;
          let enabled: Record<string, boolean> = {};
          // Tauri's invoke is read-only; intercept its IPC fetch instead.
          window.fetch = async (input, options) => {
            const command = decodeURIComponent(String(input));
            if (command.includes("plugin:menu|new")) {
              const { options: menu } = JSON.parse(String(options?.body)) as {
                options: { items: MenuItem[] };
              };
              enabled = Object.fromEntries(menu.items.map((item) => [item.text, item.enabled]));
              const choice = menu.items.find((item) => item.text === text);
              if (!choice?.enabled) {
                reject(new Error(`${text} is missing or disabled`));
                return new Response(JSON.stringify("menu choice unavailable"), {
                  headers: { "Content-Type": "application/json", "Tauri-Response": "error" },
                });
              }
              item = choice;
            }
            if (command.includes("plugin:menu|popup")) {
              window.fetch = fetch;
              const channel = Number(item.handler.split(":")[1]);
              internals.runCallback(channel, { index: 0, message: "" });
              resolve(enabled);
              return new Response("null", {
                headers: { "Content-Type": "application/json", "Tauri-Response": "ok" },
              });
            }
            return fetch.call(window, input, options);
          };
          element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
        });
      } finally {
        window.fetch = fetch;
      }
    },
    await row,
    text,
  );
}
