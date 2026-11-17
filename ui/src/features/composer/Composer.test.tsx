import { act, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Command, ConfigOption, Field, SessionInfo } from "../../lib/protocol";
import { renderApp, run, session, view, workspace, worktree } from "../../test/core";

const project = workspace();

/** Renders the app with `sessions` in one workspace and views the first. */
async function viewSession(...sessions: SessionInfo[]) {
  const core = await renderApp(
    { type: "workspace", workspace: project },
    ...sessions.map((session) => ({ type: "session" as const, session })),
  );
  await view(sessions[0]!.title!);
  return core;
}

const message = () => screen.getByRole("combobox", { name: "Message" });

describe("The composer", () => {
  it("sends the trimmed message with Enter and starts a new line with Shift+Enter", async () => {
    const task = session(project, { title: "Task" });
    const core = await viewSession(task);
    await userEvent.type(message(), " first{Shift>}{Enter}{/Shift}second {Enter}");
    expect(core.invoked("prompt")).toEqual([
      { sessionId: task.id, text: "first\nsecond", attachments: [] },
    ]);
    expect(message()).toHaveValue("");
  });

  it("keeps each session's text and attachments until they are sent", async () => {
    const first = session(project, { title: "First" });
    const second = session(project, { title: "Second" });
    const core = await viewSession(first, second);
    core.on("plugin:dialog|open", () => ["/src/project/notes.txt"]);
    await userEvent.type(message(), "draft");
    await userEvent.click(screen.getByRole("button", { name: "Attach Files" }));
    expect(screen.getByTitle("/src/project/notes.txt")).toBeInTheDocument();

    await view("Second");
    expect(message()).toHaveValue("");
    expect(screen.queryByTitle("/src/project/notes.txt")).not.toBeInTheDocument();
    await userEvent.type(message(), "other draft");

    await view("First");
    expect(message()).toHaveValue("draft");
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(core.invoked("prompt")).toEqual([
      { sessionId: first.id, text: "draft", attachments: ["/src/project/notes.txt"] },
    ]);
    expect(message()).toHaveValue("");
    expect(screen.queryByTitle("/src/project/notes.txt")).not.toBeInTheDocument();
    await view("Second");
    expect(message()).toHaveValue("other draft");
  });

  it("stops a running turn", async () => {
    const task = session(project, { title: "Task", state: "running" });
    const core = await viewSession(task);
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(core.invoked("cancel")).toEqual([{ sessionId: task.id }]);
  });

  it("is disabled once the agent has exited", async () => {
    await viewSession(session(project, { title: "Task", state: "exited", agent_session_id: null }));
    expect(message()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Attach Files" })).toBeDisabled();
  });

  it("is disabled in a running run's child, whose Stop cancels the run", async () => {
    const repo = workspace({ git_working_tree: true });
    const tree = worktree();
    const child = session(repo, { title: "Child", state: "running", worktree_id: tree.id });
    const parent = run(repo, tree, [{ name: "Work", state: "running", session_id: child.id }]);
    const core = await renderApp(
      { type: "workspace", workspace: repo },
      { type: "worktree", worktree: tree },
      { type: "session", session: child },
      { type: "run", run: parent },
    );
    // The child's row shows under its expanded run, titled with its step.
    await userEvent.dblClick(screen.getByText("Implement"));
    await view("Work");
    expect(message()).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Stop" }));
    expect(core.invoked("cancel_run")).toEqual([{ runId: parent.id }]);
  });
});

describe("A permission request", () => {
  it("is answered from above the composer", async () => {
    const task = session(project, { title: "Task", state: "awaiting_permission" });
    const core = await viewSession(task);
    core.update(task.id, {
      type: "permission",
      recorded_at: 0,
      key: "request:7",
      request_id: "7",
      title: "Run `ls`",
      options: [
        { optionId: "yes", name: "Allow", kind: "allow_once" },
        { optionId: "no", name: "Reject", kind: "reject_once" },
      ],
    });
    await userEvent.click(screen.getByRole("button", { name: "Allow" }));
    expect(core.invoked("permit")).toEqual([
      { sessionId: task.id, requestId: "7", optionId: "yes" },
    ]);
    core.update(task.id, { type: "request_resolved", recorded_at: 0, request_id: "7" });
    expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
  });
});

describe("An agent's form", () => {
  const fields: Field[] = [
    {
      name: "color",
      type: "string",
      title: "Color",
      description: "Pick a color",
      required: true,
      oneOf: [
        { const: "red", title: "Red", description: "Warm" },
        { const: "blue", title: "Blue", description: "Cool" },
      ],
    },
    {
      name: "extras",
      type: "array",
      title: "Extras",
      description: "Pick up to two",
      required: false,
      maxItems: 2,
      items: {
        anyOf: [
          { const: "sprinkles", title: "Sprinkles" },
          { const: "nuts", title: "Nuts" },
          { const: "cherries", title: "Cherries" },
        ],
      },
    },
    { name: "note", type: "string", title: "Note", required: false, default: "first" },
  ];

  async function openForm() {
    const task = session(project, { title: "Task", state: "awaiting_input" });
    const core = await viewSession(task);
    core.update(task.id, {
      type: "elicitation",
      recorded_at: 0,
      key: "request:9",
      request_id: "9",
      message: "Order",
      fields,
    });
    return { core, task };
  }

  const button = (name: string) => screen.getByRole("button", { name });
  const step = () => document.querySelector(".request-panel .step")?.textContent;

  it("asks one question per page, keeps answers going back, and sends them all", async () => {
    const { core, task } = await openForm();
    expect(step()).toBe("1 of 2");
    expect(screen.queryByText("Extras")).not.toBeInTheDocument();
    // The colour is required, so the form stays on its page.
    await userEvent.click(button("Next"));
    expect(step()).toBe("1 of 2");
    await userEvent.click(screen.getByLabelText(/Blue/));
    await userEvent.click(button("Next"));
    expect(step()).toBe("2 of 2");
    // A field without a description shares the page before it.
    await userEvent.click(screen.getByLabelText("Nuts"));
    await userEvent.type(screen.getByRole("textbox", { name: "Note" }), " edited");
    await userEvent.click(button("Back"));
    expect(screen.getByLabelText(/Blue/)).toBeChecked();
    await userEvent.click(button("Next"));
    expect(screen.getByLabelText("Nuts")).toBeChecked();
    expect(core.invoked("respond_elicitation")).toEqual([]);
    await userEvent.click(button("Submit"));
    expect(core.invoked("respond_elicitation")).toEqual([
      {
        sessionId: task.id,
        requestId: "9",
        action: {
          action: "accept",
          content: { color: "blue", extras: ["nuts"], note: "first edited" },
        },
      },
    ]);
  });

  it("holds a multiple choice to its bounds before sending", async () => {
    const { core } = await openForm();
    await userEvent.click(screen.getByLabelText(/Blue/));
    await userEvent.click(button("Next"));
    for (const extra of ["Sprinkles", "Nuts", "Cherries"]) {
      await userEvent.click(screen.getByLabelText(extra));
    }
    await userEvent.click(button("Submit"));
    expect(core.invoked("respond_elicitation")).toEqual([]);
    await userEvent.click(screen.getByLabelText("Sprinkles"));
    await userEvent.click(button("Submit"));
    expect(core.invoked("respond_elicitation")[0]!.action).toEqual({
      action: "accept",
      content: { color: "blue", extras: ["nuts", "cherries"], note: "first" },
    });
  });

  it("declines", async () => {
    const { core, task } = await openForm();
    await userEvent.click(button("Decline"));
    expect(core.invoked("respond_elicitation")).toEqual([
      { sessionId: task.id, requestId: "9", action: { action: "decline" } },
    ]);
  });
});

describe("Slash command completion", () => {
  const commands: Command[] = [
    { type: "command", name: "echo", description: "Echo", input: { type: "text", hint: "text" } },
    { type: "command", name: "review", description: "Review a file" },
    { type: "command", name: "plugin:check", description: "Check" },
  ];
  const options = () =>
    screen
      .queryAllByRole("option")
      .map((option) => option.querySelector(".menu-item-text > span")!.textContent!.split(" ")[0]);

  async function withCommands() {
    const task = session(project, { title: "Task", available_commands: commands });
    const core = await viewSession(task);
    return { core, task };
  }

  it("completes a command from the keyboard, then sends its arguments", async () => {
    const { core, task } = await withCommands();
    await userEvent.type(message(), "/");
    expect(options()).toEqual(["/echo", "/review", "/plugin:check"]);
    await userEvent.keyboard("{ArrowDown}{Enter}");
    expect(message()).toHaveValue("/review ");
    expect(options()).toEqual([]);
    await userEvent.type(message(), "src/main.rs{Enter}");
    expect(core.invoked("prompt")).toEqual([
      { sessionId: task.id, text: "/review src/main.rs", attachments: [] },
    ]);
  });

  it("filters without case and completes with Tab or a click", async () => {
    await withCommands();
    await userEvent.type(message(), "/RE");
    expect(options()).toEqual(["/review"]);
    await userEvent.click(screen.getByRole("option"));
    expect(message()).toHaveValue("/review ");
    expect(message()).toHaveFocus();
    await userEvent.clear(message());
    await userEvent.type(message(), "/plugin:{Tab}");
    expect(message()).toHaveValue("/plugin:check ");
  });

  it("keeps the arguments when completing a command being edited", async () => {
    await withCommands();
    await userEvent.type(message(), "/re existing");
    expect(options()).toEqual([]);
    await userEvent.type(message(), "{ArrowLeft>9/}");
    expect(options()).toEqual(["/review"]);
    await userEvent.keyboard("{Enter}");
    expect(message()).toHaveValue("/review existing");
  });

  it("leaves Escape, prose, unknown commands and new lines alone", async () => {
    const { core } = await withCommands();
    await userEvent.type(message(), "/{Escape}");
    expect(options()).toEqual([]);
    expect(message()).toHaveValue("/");
    await userEvent.clear(message());
    await userEvent.type(message(), "text /re");
    expect(options()).toEqual([]);
    await userEvent.clear(message());
    await userEvent.type(message(), "/unknown");
    expect(options()).toEqual([]);
    await userEvent.clear(message());
    await userEvent.type(message(), "/{Shift>}{Enter}{/Shift}");
    expect(message()).toHaveValue("/\n");
    expect(core.invoked("prompt")).toEqual([]);
  });

  it("follows the agent's catalog", async () => {
    const { core, task } = await withCommands();
    core.list({ type: "session", session: { ...task, available_commands: [commands[0]!] } });
    await userEvent.type(message(), "/");
    expect(options()).toEqual(["/echo"]);
    core.list({ type: "session", session: { ...task, available_commands: [] } });
    expect(options()).toEqual([]);
  });
});

describe("The model picker", () => {
  const model: ConfigOption = {
    configId: "model",
    name: "Model",
    category: "model",
    type: "select",
    currentValue: "smart",
    options: [
      { value: "fast", name: "Fast", _meta: { "ox/provider": "OpenRouter" } },
      { value: "smart", name: "Smart" },
    ],
  };
  const effort: ConfigOption = {
    configId: "effort",
    name: "Effort",
    category: "thought_level",
    type: "select",
    currentValue: "medium",
    options: ["low", "medium", "high"].map((value) => ({
      value,
      name: value[0]!.toUpperCase() + value.slice(1),
    })),
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
  const picker = () => document.querySelector("button.model-picker")!;

  async function withOptions() {
    const task = session(project, { title: "Task", config_options: [model, effort, mode] });
    const core = await viewSession(task);
    return { core, task };
  }

  it("shows a chosen model at once, until the agent refuses it", async () => {
    const { core, task } = await withOptions();
    expect(picker()).toHaveTextContent("SmartMedium");
    let refuse: (error: string) => void = () => {};
    core.on(
      "set_config_option",
      () => new Promise((_, reject) => (refuse = reject as (error: string) => void)),
    );
    await userEvent.click(
      within(screen.getByRole("listbox", { name: "Model", hidden: true })).getByText("Fast"),
    );
    expect(picker()).toHaveTextContent("FastOpenRouterMedium");
    expect(core.invoked("set_config_option")).toEqual([
      { sessionId: task.id, configId: "model", value: "fast" },
    ]);
    await act(async () => refuse("unavailable"));
    expect(picker()).toHaveTextContent("SmartMedium");
  });

  it("chooses with the keyboard and sets the effort with the slider", async () => {
    const { core } = await withOptions();
    const list = screen.getByRole("listbox", { name: "Model", hidden: true });
    for (const key of ["ArrowDown", "ArrowUp", "Enter"]) fireEvent.keyDown(list, { key });
    expect(picker()).toHaveTextContent("FastOpenRouterMedium");
    const slider = screen.getByRole("slider", { name: "Effort", hidden: true });
    fireEvent.change(slider, { target: { value: "2" } });
    expect(picker()).toHaveTextContent("FastOpenRouterHigh");
    expect(
      core.invoked("set_config_option").map(({ configId, value }) => [configId, value]),
    ).toEqual([
      ["model", "fast"],
      ["effort", "high"],
    ]);
  });

  it("shows other options as their own pickers", async () => {
    const { core, task } = await withOptions();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Mode" }), "code");
    expect(core.invoked("set_config_option")).toEqual([
      { sessionId: task.id, configId: "mode", value: "code" },
    ]);
  });
});
