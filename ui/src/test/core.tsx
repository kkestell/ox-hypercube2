/**
 * The desktop core and oxd, faked at the Tauri IPC boundary, for rendering
 * the app in jsdom. Tests push listing changes and conversation updates
 * through the channels the app opened, and read the commands it invoked.
 */
import type { Channel } from "@tauri-apps/api/core";
import { mockIPC } from "@tauri-apps/api/mocks";
import { act, render, screen } from "@testing-library/react";
import { App } from "../App";
import type {
  ConversationUpdate,
  DirEntry,
  FileContent,
  ListingChange,
  RunInfo,
  SessionInfo,
  StepInfo,
  WatchMessage,
  WorkspaceInfo,
  WorktreeInfo,
} from "../lib/protocol";

type Args = Record<string, unknown>;
type MenuItem = { text: string; enabled: boolean; handler: Channel<unknown> };

export class Core {
  /** Every command the app invoked, in order. */
  readonly calls: { cmd: string; args: Args }[] = [];
  /** The answer to `agents`. */
  agents = ["claude"];
  private readonly handlers = new Map<string, (args: Args) => unknown>();
  private watch: Channel<WatchMessage> | undefined;
  private readonly views = new Map<string, Channel<ConversationUpdate[]>>();
  private readonly directories = new Map<string, Channel<DirEntry[]>>();
  private readonly files = new Map<string, Channel<FileContent>>();
  private menu: MenuItem[] = [];

  constructor() {
    mockIPC((cmd, args) => {
      const payload = (args ?? {}) as Args;
      this.calls.push({ cmd, args: payload });
      const handler = this.handlers.get(cmd);
      if (handler) return handler(payload);
      switch (cmd) {
        case "watch":
          this.watch = payload.channel as Channel<WatchMessage>;
          return null;
        case "view_session":
          this.views.set(
            payload.sessionId as string,
            payload.channel as Channel<ConversationUpdate[]>,
          );
          return null;
        case "watch_directory":
          this.directories.set(payload.path as string, payload.channel as Channel<DirEntry[]>);
          return null;
        case "watch_file":
          this.files.set(payload.path as string, payload.channel as Channel<FileContent>);
          return null;
        case "agents":
          return this.agents;
        case "workflows":
          return [];
        case "workspace_branches":
          return { head: "main" };
        case "plugin:menu|new":
          this.menu = (payload.options as { items: MenuItem[] }).items;
          return [1, "menu"];
        default:
          return null;
      }
    });
  }

  /** Answers `cmd` with `handler` in place of the default answer. */
  on(cmd: string, handler: (args: Args) => unknown) {
    this.handlers.set(cmd, handler);
  }

  /** The arguments of each call of `cmd`. */
  invoked(cmd: string): Args[] {
    return this.calls.filter((call) => call.cmd === cmd).map((call) => call.args);
  }

  /** Sends listing changes on the app's watch. */
  list(...changes: ListingChange[]) {
    send(this.watch, { type: "changes", changes });
  }

  /** Ends the app's watch as a dropped connection to oxd does. */
  disconnect() {
    send(this.watch, { type: "disconnected" });
  }

  /** Sends one batch of conversation updates to the session's view. */
  update(sessionId: string, ...updates: ConversationUpdate[]) {
    send(this.views.get(sessionId), updates);
  }

  /** Sends a directory's entries to its watch. */
  directory(path: string, entries: DirEntry[]) {
    send(this.directories.get(path), entries);
  }

  /** Sends a file's text to its watch. */
  file(path: string, text: string) {
    send(this.files.get(path), { type: "text", text });
  }

  /**
   * Opens the context menu of `element` and returns whether each item is
   * enabled.
   */
  async contextMenu(element: Element): Promise<Record<string, boolean>> {
    this.menu = [];
    await act(async () => {
      element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
    return Object.fromEntries(this.menu.map((item) => [item.text, item.enabled]));
  }

  /** Chooses `text` from the context menu of `element`. */
  async choose(element: Element, text: string) {
    const enabled = await this.contextMenu(element);
    if (!enabled[text]) throw new Error(`${text} is missing or disabled`);
    const item = this.menu.find((item) => item.text === text)!;
    await act(async () => item.handler.onmessage(undefined));
  }
}

function send<T>(channel: Channel<T> | undefined, message: T) {
  if (!channel) throw new Error("the app has not opened this channel");
  act(() => channel.onmessage(message));
}

/** Renders the app against a new fake core that lists `changes`. */
export async function renderApp(...changes: ListingChange[]) {
  const core = new Core();
  render(<App />);
  // The app's first commands resolve in microtasks.
  await act(async () => {});
  core.list(...changes);
  await act(async () => {});
  return core;
}

/** Views the session or run titled `title` from the sidebar. */
export async function view(title: string) {
  const row = sidebarRow(title);
  await act(async () => row.querySelector<HTMLButtonElement>("button.select")!.click());
  return row;
}

/** The sidebar row of the session or run titled `title`. */
export function sidebarRow(title: string): HTMLElement {
  return screen.getByText(title, { selector: ".sidebar-row .title" }).closest("li")!;
}

let ids = 0;
const nextId = (prefix: string) => `${prefix}-${++ids}`;

export function workspace(fields: Partial<WorkspaceInfo> = {}): WorkspaceInfo {
  return { id: nextId("workspace"), path: "/src/project", git_working_tree: false, ...fields };
}

export function session(
  workspaceInfo: WorkspaceInfo,
  fields: Partial<SessionInfo> = {},
): SessionInfo {
  return {
    id: nextId("session"),
    command: ["claude-code-acp"],
    workspace_id: workspaceInfo.id,
    cwd: workspaceInfo.path,
    state: "idle",
    title: null,
    agent_session_id: "agent",
    config_options: [],
    available_commands: [],
    worktree_id: null,
    archived: false,
    created_at: ++ids,
    unseen: null,
    ...fields,
  };
}

export function worktree(fields: Partial<WorktreeInfo> = {}): WorktreeInfo {
  const id = nextId("worktree");
  return {
    id,
    path: `/src/project/.ox/${id}`,
    branch: `ox/${id}`,
    base: "main",
    created_branch: true,
    compare: "main",
    comparison: { type: "compared", tip: "b", compare_tip: "a", ahead: 0, behind: 0 },
    ...fields,
  };
}

export function run(
  workspaceInfo: WorkspaceInfo,
  worktreeInfo: WorktreeInfo,
  steps: Partial<StepInfo>[],
  fields: Partial<RunInfo> = {},
): RunInfo {
  return {
    id: nextId("run"),
    workspace_id: workspaceInfo.id,
    workflow: "implement",
    name: "Implement",
    input: "",
    worktree_id: worktreeInfo.id,
    state: "running",
    error: null,
    steps: steps.map((step, index) => ({
      name: `Step ${index + 1}`,
      state: "pending",
      started_at: null,
      session_id: null,
      ...step,
    })),
    created_at: ++ids,
    ended_at: null,
    unseen: null,
    ...fields,
  };
}
