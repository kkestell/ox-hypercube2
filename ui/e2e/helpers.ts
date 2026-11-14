import { execFile } from "node:child_process";
import { mkdtemp, realpath, writeFile } from "node:fs/promises";
import { createConnection } from "node:net";
import { join, resolve } from "node:path";
import { promisify } from "node:util";

const target = resolve(import.meta.dirname, "../../target/debug");

/** The path of a binary the workspace builds. */
export function binary(name: string): string {
  return join(target, name);
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

/** Starts a fake agent that never finishes initializing. oxctl waits for startup. */
export async function startStuckSession(): Promise<string> {
  const cwd = await tempDir("session-");
  const workspace = await addWorkspace(cwd);
  const socket = createConnection(process.env.OX_SOCKET!);
  try {
    socket.write(
      [
        { id: 1, method: "hello", params: { protocol_version: 1 } },
        {
          id: 2,
          method: "session.new",
          params: {
            workspace_id: workspace,
            command: [binary("oxd-fake-agent"), "--hang-initialize"],
          },
        },
      ]
        .map((request) => `${JSON.stringify(request)}\n`)
        .join(""),
    );
    socket.setTimeout(5000, () => socket.destroy(new Error("timed out starting the fake agent")));
    let pending = "";
    for await (const chunk of socket.setEncoding("utf8")) {
      pending += chunk;
      const lines = pending.split("\n");
      pending = lines.pop()!;
      for (const line of lines) {
        const response = JSON.parse(line);
        if (response.error) throw new Error(response.error.message);
        if (response.id === 2) return cwd;
      }
    }
    throw new Error("the daemon closed the socket during startup");
  } finally {
    socket.destroy();
  }
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

/** Starts a session of the fake agent in a new worktree of `workspace`. */
export async function startWorktreeSession(workspace: string) {
  await oxctl("new", "--workspace", workspace, "--worktree", "--", binary("oxd-fake-agent"));
}

/** Starts the fake agent in an existing workspace's checkout. */
export async function startCheckoutSession(workspace: string) {
  await oxctl("new", "--workspace", workspace, "--", binary("oxd-fake-agent"));
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

/** Deletes the sessions of the workspace in `cwd` with `oxctl`. */
export async function deleteSessions(cwd: string) {
  const workspace = await workspaceId(cwd);
  for (const line of lines(await oxctl("ls"))) {
    const [id, sessionWorkspace] = line.split("\t");
    if (sessionWorkspace === workspace) await oxctl("delete", "--discard", id!);
  }
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

/** Removes the workspace in `cwd` with `oxctl`. */
export async function removeWorkspace(cwd: string) {
  await oxctl("workspace", "rm", await workspaceId(cwd));
}

async function workspaceId(cwd: string): Promise<string> {
  for (const line of lines(await oxctl("workspace", "ls"))) {
    const [id, path] = line.split("\t");
    if (path === cwd) return id!;
  }
  throw new Error(`no workspace in ${cwd}`);
}

/** Deletes every session oxd has and removes every workspace. */
export async function resetDaemon() {
  for (const line of lines(await oxctl("ls"))) {
    await oxctl("delete", "--discard", line.split("\t")[0]!);
  }
  for (const line of lines(await oxctl("workspace", "ls"))) {
    await oxctl("workspace", "rm", line.split("\t")[0]!);
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

function lines(output: string): string[] {
  return output.split("\n").filter(Boolean);
}

async function oxctl(...args: string[]): Promise<string> {
  const { stdout } = await promisify(execFile)(binary("oxctl"), args);
  return stdout;
}

/** The session list's group for the workspace in `cwd`. */
export function workspaceGroup(cwd: string) {
  return $(`//section[@class="workspace"][.//h2[@title="${cwd}"]]`);
}

/** The session list's entry for the session in `cwd`. */
export function sessionEntry(cwd: string) {
  return $(`//li[@class="sidebar-row session"][button[@class="select"][@title="${cwd}"]]`);
}

/** The session list's entry for the one session in a worktree. */
export function worktreeEntry() {
  const worktrees = join(process.env.OX_E2E_DIR!, "worktrees/");
  return $(
    `//li[@class="sidebar-row session"][button[@class="select"][starts-with(@title, "${worktrees}")]]`,
  );
}

/** Opens the New Task menu of the workspace in `cwd`. */
export async function openMenu(cwd: string) {
  await workspaceGroup(cwd).$('button[aria-label="New Task"]').click();
  const menu = workspaceGroup(cwd).$('[role="menu"]');
  await expect(menu).toBeDisplayed();
  return menu;
}

/** Starts a New Task in the workspace in `cwd`, and returns its worktree's path. */
export async function newTask(cwd: string): Promise<string> {
  await (await openMenu(cwd)).$("button=New Task").click();
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

/** Answers one native file picker or message box at the Tauri IPC boundary. */
export async function answerNativeDialog(command: "open" | "message", answer: string[] | null) {
  await browser.execute(
    (command, answer) => {
      const fetch = window.fetch;
      window.fetch = async (input, options) => {
        if (decodeURIComponent(String(input)).endsWith(`plugin:dialog|${command}`)) {
          window.fetch = fetch;
          const { message } = JSON.parse(String(options?.body)) as { message?: string };
          document.body.dataset.nativeDialog = message ?? command;
          return new Response(JSON.stringify(answer), {
            headers: { "Content-Type": "application/json", "Tauri-Response": "ok" },
          });
        }
        return fetch.call(window, input, options);
      };
      delete document.body.dataset.nativeDialog;
    },
    command,
    answer,
  );
}

/** The agent's text items in the viewed conversation. */
export function agentText() {
  return $$(".conversation > .text");
}

/**
 * Chooses the option with `value` in the select `selector` matches. The
 * embedded driver clicks with `element.click()`, which selects no option, so
 * this sets the value and dispatches `change`, as the webview does when a user
 * chooses an option.
 */
export async function choose(selector: string, value: string) {
  await $(selector).waitForExist();
  await browser.execute(
    (selector, value) => {
      const select = document.querySelector<HTMLSelectElement>(selector)!;
      select.value = value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    },
    selector,
    value,
  );
}

/**
 * Moves the slider `selector` matches to `value`. The embedded driver neither
 * drags a slider's thumb nor sends it keys, so this sets the value through the
 * input's own setter, which React's tracking does not see, and dispatches
 * `input`, as the webview does when a user moves the thumb.
 */
export async function slide(selector: string, value: number) {
  await $(selector).waitForExist();
  await browser.execute(
    (selector, value) => {
      const input = document.querySelector<HTMLInputElement>(selector)!;
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(
        input,
        String(value),
      );
      input.dispatchEvent(new Event("input", { bubbles: true }));
    },
    selector,
    value,
  );
}
