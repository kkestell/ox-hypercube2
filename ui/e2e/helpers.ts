import { execFile } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, realpath, rm, writeFile } from "node:fs/promises";
import { createConnection, type Socket } from "node:net";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { promisify } from "node:util";
import type { ChainablePromiseElement } from "webdriverio";
import type { ListingChange, RunInfo, SessionInfo, WorkspaceInfo } from "../src/lib/protocol";

const target = resolve(import.meta.dirname, "../../target/debug");

/** The path of a binary as the workspace builds it. */
export function built(name: string): string {
  return join(target, name);
}

/** The directory the run's app, oxd and agents run from. */
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
 * Adds a new folder as a workspace, starts a session of the scripted fake
 * agent in it, and returns the folder's path, which the session's entry shows
 * on hover. Each line of a prompt to the fake agent is a directive, such as
 * `say 3 0`; its module documentation lists them.
 */
export async function startSession(...agentArgs: string[]): Promise<string> {
  const cwd = await tempDir("session-");
  await newSession(await addWorkspace(cwd), { type: "workspace" }, agentArgs);
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
  const id = await newSession(workspace, { type: "worktree" }, []);
  const session = (await sessions()).find((session) => session.id === id);
  return session!.cwd.replace(/\/$/, "");
}

/**
 * Starts a session of the fake agent with `agentArgs` in `workspace`, waits
 * for its agent to start, and returns the session's ID.
 */
async function newSession(
  workspace: string,
  checkout: { type: "workspace" | "worktree" },
  agentArgs: string[],
): Promise<string> {
  const { id } = await call<SessionInfo>("session.new", {
    workspace_id: workspace,
    command: [binary("oxd-fake-agent"), ...agentArgs],
    env: env(),
    checkout,
  });
  const connection = await Connection.open();
  try {
    await connection.call("session.subscribe", { session_id: id, after_seq: 0 });
    for (;;) {
      const { event } = await connection.read();
      if (event?.kind.type === "agent_started") return id;
      if (event?.kind.type === "agent_exited") {
        throw new Error(`The session did not start: ${event.kind.data?.error}`);
      }
    }
  } finally {
    connection.close();
  }
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
  return (await call<WorkspaceInfo>("workspace.add", { path: cwd })).id;
}

/** The ID of the one session in `cwd`. */
export async function sessionId(cwd: string): Promise<string> {
  const ids = (await sessions())
    .filter((session) => session.cwd.replace(/\/$/, "") === cwd)
    .map((session) => session.id);
  if (ids.length !== 1) throw new Error(`${cwd} has ${ids.length} sessions`);
  return ids[0]!;
}

/** Sends `text` to the session `sessionId` as a prompt. */
export async function prompt(sessionId: string, text: string) {
  await call("session.prompt", { session_id: sessionId, prompt: [{ type: "text", text }] });
}

/** Compares the branch of the one session of `workspace` with its ref again. */
export async function compareSession(workspace: string) {
  const worktrees = (await sessions())
    .filter((session) => session.workspace_id === workspace)
    .map((session) => session.worktree_id);
  if (worktrees.length !== 1) throw new Error(`${workspace} has ${worktrees.length} sessions`);
  await call("worktree.refresh", { worktree_id: worktrees[0] });
}

/**
 * Removes every run oxd has, cancelling a running one and waiting for it to
 * end first, then deletes every session, removes every workspace, and
 * removes the workflow definitions.
 */
export async function resetDaemon() {
  for (const run of (await listing()).flatMap((c) => (c.type === "run" ? [c.run] : []))) {
    if (run.state === "running") {
      await call("run.cancel", { run_id: run.id });
      await runEnded(run.id);
    }
    await call("run.delete", { run_id: run.id, discard: true, env: env() });
  }
  for (const session of await sessions()) {
    await call("session.delete", { session_id: session.id, discard: true, env: env() });
  }
  for (const change of await listing()) {
    if (change.type === "workspace") {
      await call("workspace.remove", { workspace_id: change.workspace.id });
    }
  }
  await rm(workflowsDir(), { recursive: true, force: true });
}

/** Waits until the run `runId` is not running. */
async function runEnded(runId: string) {
  const connection = await Connection.open();
  try {
    await connection.call("watch");
    for (;;) {
      const { changes } = await connection.read();
      const run = changes?.find(
        (change): change is { type: "run"; run: RunInfo } =>
          change.type === "run" && change.run.id === runId,
      );
      if (run && run.run.state !== "running") return;
    }
  } finally {
    connection.close();
  }
}

/**
 * Stops the run's oxd. It runs as `oxd --socket <OX_SOCKET>`, as the client
 * starts it.
 */
export async function stopDaemon() {
  const socket = process.env.OX_SOCKET!.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  await promisify(execFile)("pkill", ["-TERM", "-f", `oxd --socket ${socket}$`]);
}

/** A message from oxd: a request's reply, a session's event, or a watch's changes. */
type DaemonMessage = {
  id?: number;
  result?: unknown;
  error?: { message: string };
  event?: { kind: { type: string; data?: { error?: string | null } } };
  changes?: ListingChange[];
};

/**
 * A connection to the run's oxd. Each message is one line of JSON: a request
 * is answered by the message with its `id`, and a stream's messages follow.
 */
class Connection {
  private nextId = 1;
  private readonly lines: AsyncIterator<string>;

  private constructor(private readonly socket: Socket) {
    this.lines = createInterface({ input: socket })[Symbol.asyncIterator]();
  }

  static async open(): Promise<Connection> {
    const socket = createConnection(process.env.OX_SOCKET!);
    await once(socket, "connect");
    const connection = new Connection(socket);
    await connection.call("hello", { protocol_version: 1 });
    return connection;
  }

  async read(): Promise<DaemonMessage> {
    const { value, done } = await this.lines.next();
    if (done) throw new Error("oxd closed the connection");
    return JSON.parse(value);
  }

  /** Sends a request and returns its result. */
  async call<T>(method: string, params?: object): Promise<T> {
    const id = this.nextId++;
    this.socket.write(`${JSON.stringify({ id, method, params })}\n`);
    for (;;) {
      const message = await this.read();
      if (message.id !== id) continue;
      if (message.error) throw new Error(`${method}: ${message.error.message}`);
      return message.result as T;
    }
  }

  close() {
    this.socket.destroy();
  }
}

/** Sends one request to oxd on a connection of its own and returns its result. */
async function call<T = unknown>(method: string, params?: object): Promise<T> {
  const connection = await Connection.open();
  try {
    return await connection.call<T>(method, params);
  } finally {
    connection.close();
  }
}

/** Every workspace, worktree, session and run, as a watch's first changes list them. */
async function listing(): Promise<ListingChange[]> {
  const connection = await Connection.open();
  try {
    await connection.call("watch");
    for (;;) {
      const { changes } = await connection.read();
      if (changes) return changes;
    }
  } finally {
    connection.close();
  }
}

async function sessions(): Promise<SessionInfo[]> {
  return (await listing()).flatMap((change) => (change.type === "session" ? [change.session] : []));
}

/** The test process's environment, which git and agents run with over oxd's. */
function env(): Record<string, string> {
  return Object.fromEntries(
    Object.entries(process.env).filter(
      (entry): entry is [string, string] => entry[1] !== undefined,
    ),
  );
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

/**
 * Starts a Session in Worktree in the workspace in `cwd` by sending its draft's
 * first message, and returns its worktree's path.
 */
export async function newWorktreeSession(cwd: string): Promise<string> {
  await (await openMenu(cwd)).$("button=Session in Worktree").click();
  await send("hello");
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
