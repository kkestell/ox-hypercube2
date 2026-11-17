import { act, cleanup, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type {
  Command,
  ConfigOption,
  Controls,
  SessionInfo,
  WorkspaceInfo,
} from "../../lib/protocol";
import { renderApp, session, sidebarRow, view, workspace } from "../../test/core";

const model: ConfigOption = {
  configId: "model",
  name: "Model",
  category: "model",
  type: "select",
  currentValue: "smart",
  options: [
    { value: "fast", name: "Fast" },
    { value: "smart", name: "Smart" },
  ],
};
const mode: ConfigOption = {
  configId: "mode",
  name: "Mode",
  type: "select",
  currentValue: "ask",
  options: [
    { value: "ask", name: "Ask" },
    { value: "code", name: "Code" },
  ],
};
const review: Command = { type: "command", name: "review", description: "Review a file" };
const controls: Controls = { config_options: [model, mode], available_commands: [review] };

/** Caches `controls` as Claude's controls in `workspaceInfo`. */
function cache(workspaceInfo: WorkspaceInfo) {
  localStorage.setItem(`controls:claude:${workspaceInfo.id}`, JSON.stringify(controls));
}

/** Chooses `item` from the New Session menu of the workspace named `name`. */
async function newDraft(name: string, item = "Session in Checkout") {
  const menu = within(screen.getByRole("region", { name })).getByRole("menu", { hidden: true });
  await userEvent.click(within(menu).getByText(item));
}

/** The titles of the rows of the workspace named `name`, in order. */
function rows(name: string) {
  return within(screen.getByRole("region", { name }))
    .getAllByRole("listitem")
    .map((row) => row.querySelector(".title")!.textContent);
}

const message = () => screen.getByRole("combobox", { name: "Message" });
const viewedRow = (title: string) =>
  sidebarRow(title).querySelector("button.select")!.getAttribute("aria-current");
const picker = () => document.querySelector<HTMLButtonElement>("button.model-picker")!;

describe("A draft", () => {
  it("opens from New Session at the top of its workspace, with nothing started", async () => {
    const plain = workspace({ path: "/src/plain" });
    const repo = workspace({ path: "/src/repo", git_working_tree: true });
    const core = await renderApp(
      { type: "workspace", workspace: plain },
      { type: "workspace", workspace: repo },
      { type: "session", session: session(repo, { title: "Older" }) },
    );
    const items = (name: string) =>
      within(within(screen.getByRole("region", { name })).getByRole("menu", { hidden: true }))
        .getAllByRole("menuitem", { hidden: true })
        .map((item) => item.textContent);
    expect(items("plain")).toEqual(["Session in Checkout"]);
    expect(items("repo")).toEqual(["Session in Worktree", "Session in Checkout"]);

    await newDraft("repo", "Session in Worktree");
    expect(rows("repo")).toEqual(["New session", "Older"]);
    expect(viewedRow("New session")).toBe("true");
    expect(screen.getByText("New worktree")).toBeInTheDocument();

    await newDraft("plain");
    expect(rows("plain")).toEqual(["New session"]);
    expect(screen.getByText("/src/plain")).toBeInTheDocument();
    expect(core.invoked("new_session")).toEqual([]);
    expect(core.invoked("workspace_branches")).toEqual([]);
  });

  it("is labeled with its text and keeps it while other sessions are viewed", async () => {
    const project = workspace({ path: "/src/project" });
    await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: session(project, { title: "Other" }) },
    );
    await newDraft("project");
    await userEvent.type(message(), "Fix   the{Shift>}{Enter}{/Shift}bug");
    expect(rows("project")).toEqual(["Fix the bug", "Other"]);
    expect(screen.getByRole("heading", { name: "Fix the bug" })).toBeInTheDocument();

    await view("Other");
    expect(message()).toHaveValue("");
    await view("Fix the bug");
    expect(message()).toHaveValue("Fix   the\nbug");
  });

  it("goes when left empty or deleted", async () => {
    const project = workspace({ path: "/src/project" });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: session(project, { title: "Other" }) },
    );
    await newDraft("project");
    await view("Other");
    expect(rows("project")).toEqual(["Other"]);

    await newDraft("project");
    await userEvent.type(message(), "Keep me");
    await view("Other");
    expect(rows("project")).toEqual(["Keep me", "Other"]);
    await core.choose(sidebarRow("Keep me"), "Delete Draft");
    expect(rows("project")).toEqual(["Other"]);
  });

  it("is one of its own for each New Session, with its own agent and checkout", async () => {
    const repo = workspace({ path: "/src/repo", git_working_tree: true });
    const core = await renderApp({ type: "workspace", workspace: repo });
    core.on("new_session", ({ text }) => session(repo, { title: text as string }));
    core.agents = ["claude", "pi"];
    await act(async () => window.dispatchEvent(new Event("focus")));
    await newDraft("repo", "Session in Worktree");
    await userEvent.type(message(), "First");
    await userEvent.selectOptions(screen.getByLabelText("Harness"), "pi");
    await newDraft("repo");
    await userEvent.type(message(), "Second");
    expect(rows("repo")).toEqual(["Second", "First"]);

    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    await view("First");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(
      core.invoked("new_session").map(({ agent, checkout, text }) => [agent, checkout, text]),
    ).toEqual([
      ["pi", { type: "workspace" }, "Second"],
      ["claude", { type: "worktree", compare: "main" }, "First"],
    ]);
  });

  it("creates the session with its first message when sent", async () => {
    const repo = workspace({ path: "/src/repo", git_working_tree: true });
    cache(repo);
    const core = await renderApp({ type: "workspace", workspace: repo });
    core.on("agent_controls", () => controls);
    const started = session(repo, {
      title: "Fix the bug",
      state: "starting",
      agent_session_id: null,
    });
    core.on("new_session", () => started);
    core.on("plugin:dialog|open", () => ["/src/repo/notes.txt"]);
    await newDraft("repo", "Session in Worktree");
    await userEvent.type(message(), " Fix the bug ");
    await userEvent.click(screen.getByRole("button", { name: "Attach Files" }));
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(core.invoked("workspace_branches")).toEqual([{ workspaceId: repo.id }]);
    expect(core.invoked("new_session")).toEqual([
      {
        agent: "claude",
        workspaceId: repo.id,
        checkout: { type: "worktree", compare: "main" },
        text: "Fix the bug",
        attachments: ["/src/repo/notes.txt"],
        config: [
          { config_id: "model", value: "smart" },
          { config_id: "mode", value: "ask" },
        ],
      },
    ]);
    core.list({ type: "session", session: started });
    expect(rows("repo")).toEqual(["Fix the bug"]);
    expect(viewedRow("Fix the bug")).toBe("true");
    expect(message()).toHaveValue("");
  });

  it("keeps its text when it cannot be sent", async () => {
    const repo = workspace({ path: "/src/repo", git_working_tree: true });
    const core = await renderApp({ type: "workspace", workspace: repo });
    core.on("workspace_branches", () => ({ head: null }));
    await newDraft("repo", "Session in Worktree");
    await userEvent.type(message(), "Detached");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(core.invoked("new_session")).toEqual([]);
    expect(JSON.stringify(core.invoked("plugin:dialog|message"))).toContain(
      "repo is not on a branch",
    );

    core.on("workspace_branches", () => ({ head: "main" }));
    core.on("new_session", () => Promise.reject("no agent"));
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(JSON.stringify(core.invoked("plugin:dialog|message"))).toContain("no agent");
    expect(rows("repo")).toEqual(["Detached"]);
    expect(message()).toHaveValue("Detached");
  });
});

describe("A draft's controls", () => {
  it("are probed one at a time for each agent and workspace with none cached", async () => {
    const first = workspace({ path: "/src/first" });
    const second = workspace({ path: "/src/second" });
    const core = await renderApp();
    core.agents = ["claude", "pi"];
    await act(async () => window.dispatchEvent(new Event("focus")));
    const answers: ((controls: Controls) => void)[] = [];
    core.on("agent_controls", () => new Promise((resolve) => answers.push(resolve)));
    core.list({ type: "workspace", workspace: first }, { type: "workspace", workspace: second });
    await act(async () => {});
    expect(core.invoked("agent_controls")).toEqual([{ agent: "claude", cwd: "/src/first" }]);
    for (let answered = 0; answered < 3; answered++) {
      await act(async () => answers[answered]!(controls));
    }
    expect(core.invoked("agent_controls")).toEqual([
      { agent: "claude", cwd: "/src/first" },
      { agent: "claude", cwd: "/src/second" },
      { agent: "pi", cwd: "/src/first" },
      { agent: "pi", cwd: "/src/second" },
    ]);
  });

  it("probes no pair that is cached until a draft of it is viewed", async () => {
    const project = workspace({ path: "/src/project" });
    cache(project);
    const core = await renderApp({ type: "workspace", workspace: project });
    expect(core.invoked("agent_controls")).toEqual([]);
    await newDraft("project");
    expect(core.invoked("agent_controls")).toEqual([{ agent: "claude", cwd: "/src/project" }]);
  });

  it("show at once, from another workspace while the draft's has none, until a probe replaces them", async () => {
    const cached = workspace({ path: "/src/cached" });
    const fresh = workspace({ path: "/src/fresh" });
    cache(cached);
    const core = await renderApp({ type: "workspace", workspace: cached });
    let answer: (controls: Controls) => void = () => {};
    core.on("agent_controls", () => new Promise((resolve) => (answer = resolve)));
    core.list({ type: "workspace", workspace: fresh });
    await newDraft("fresh");
    expect(picker()).toHaveTextContent("Smart");
    await userEvent.type(message(), "/");
    expect(document.querySelector(".menu-item-text > span")).toHaveTextContent("/review");

    await act(async () =>
      answer({ config_options: [{ ...model, currentValue: "fast" }], available_commands: [] }),
    );
    expect(picker()).toHaveTextContent("Fast");
    expect(screen.queryByRole("combobox", { name: "Mode" })).not.toBeInTheDocument();
  });

  it("start at the values of the agent's newest session in the workspace", async () => {
    const project = workspace({ path: "/src/project" });
    cache(project);
    const core = await renderApp(
      { type: "workspace", workspace: project },
      {
        type: "session",
        session: session(project, {
          title: "Old",
          config_options: [{ ...model, currentValue: "smart" }, mode],
        }),
      },
      {
        type: "session",
        session: session(project, {
          title: "New",
          config_options: [
            { ...model, currentValue: "fast" },
            { ...mode, currentValue: "code" },
          ],
        }),
      },
    );
    core.on("agent_controls", () => controls);
    core.on("new_session", () => session(project, { title: "Go" }));
    await newDraft("project");
    expect(picker()).toHaveTextContent("Fast");
    expect(screen.getByRole("combobox", { name: "Mode" })).toHaveValue("code");

    await userEvent.click(
      within(screen.getByRole("listbox", { name: "Model", hidden: true })).getByText("Smart"),
    );
    expect(picker()).toHaveTextContent("Smart");
    await userEvent.type(message(), "Go{Enter}");
    expect(core.invoked("new_session")[0]!.config).toEqual([
      { config_id: "model", value: "smart" },
      { config_id: "mode", value: "code" },
    ]);
  });

  it("stay disabled at the sent values while the new session's agent starts", async () => {
    const project = workspace({ path: "/src/project" });
    cache(project);
    const core = await renderApp({ type: "workspace", workspace: project });
    core.on("agent_controls", () => controls);
    const started: SessionInfo = session(project, {
      title: "Go",
      state: "starting",
      agent_session_id: null,
    });
    core.on("new_session", () => started);
    await newDraft("project");
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Mode" }), "code");
    await userEvent.type(message(), "Go{Enter}");
    core.list({ type: "session", session: started });
    expect(picker()).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "Mode" })).toHaveValue("code");
    expect(screen.getByRole("combobox", { name: "Mode" })).toBeDisabled();

    core.list({
      type: "session",
      session: {
        ...started,
        state: "idle",
        agent_session_id: "agent",
        config_options: [{ ...model, currentValue: "fast" }],
      },
    });
    expect(picker()).toBeEnabled();
    expect(picker()).toHaveTextContent("Fast");
    expect(screen.queryByRole("combobox", { name: "Mode" })).not.toBeInTheDocument();
  });
});

describe("A draft's files", () => {
  it("are the workspace's, beside the draft", async () => {
    const project = workspace({ path: "/src/project" });
    const core = await renderApp({ type: "workspace", workspace: project });
    await newDraft("project");
    core.directory("/src/project", [{ name: "main.rs", kind: "file" }]);
    await userEvent.click(
      screen.getByText("main.rs", { selector: ".tree-row .title" }).closest("button")!,
    );
    core.file("/src/project/main.rs", "fn main() {}\n");
    expect(document.querySelector(".file-body pre")!.textContent).toBe("fn main() {}\n");
  });
});

describe("Unsent work", () => {
  it("survives restarting the app", async () => {
    const project = workspace({ path: "/src/project" });
    const task = session(project, { title: "Task" });
    const listing = [
      { type: "workspace" as const, workspace: project },
      { type: "session" as const, session: task },
    ];
    let core = await renderApp();
    core.on("agent_controls", () => controls);
    core.list(...listing);
    await view("Task");
    await userEvent.type(message(), "Session text");
    await newDraft("project");
    await userEvent.type(message(), "Draft text");

    cleanup();
    sessionStorage.clear();
    core = await renderApp(...listing);
    expect(core.invoked("agent_controls")).toEqual([]);
    // The probe on viewing the draft stays unanswered.
    core.on("agent_controls", () => new Promise(() => {}));
    expect(rows("project")).toEqual(["Draft text", "Task"]);
    await view("Draft text");
    expect(message()).toHaveValue("Draft text");
    expect(picker()).toHaveTextContent("Smart");
    await view("Task");
    expect(message()).toHaveValue("Session text");
  });
});
