import { act, fireEvent, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Command, ConfigSetting, Field, SessionInfo } from "../../lib/protocol";
import { type Core, renderApp, session, view, workspace } from "../../test/core";

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

  it("queues with Enter and steers with Cmd+Enter while the agent works", async () => {
    const task = session(project, { title: "Task", state: "running" });
    const core = await viewSession(task);
    await userEvent.type(message(), "next{Enter}");
    expect(core.invoked("queue_add")).toEqual([
      { sessionId: task.id, text: "next", attachments: [] },
    ]);
    expect(message()).toHaveValue("");
    await userEvent.type(message(), "now{Meta>}{Enter}{/Meta}");
    expect(core.invoked("prompt")).toEqual([{ sessionId: task.id, text: "now", attachments: [] }]);
  });

  it("sends with Cmd+Enter on an idle session", async () => {
    const task = session(project, { title: "Task" });
    const core = await viewSession(task);
    await userEvent.type(message(), "now{Meta>}{Enter}{/Meta}");
    expect(core.invoked("prompt")).toEqual([{ sessionId: task.id, text: "now", attachments: [] }]);
    expect(core.invoked("queue_add")).toEqual([]);
  });

  it("offers Send for a draft and Stop without one while the agent works", async () => {
    const task = session(project, { title: "Task", state: "awaiting_input" });
    const core = await viewSession(task);
    expect(screen.getByRole("button", { name: "Stop" })).toBeEnabled();
    await userEvent.type(message(), "later");
    expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(core.invoked("queue_add")).toEqual([
      { sessionId: task.id, text: "later", attachments: [] },
    ]);
    expect(core.invoked("cancel")).toEqual([]);
  });

  it("stops the turn with Escape, unless Escape closes the command menu", async () => {
    const task = session(project, {
      title: "Task",
      state: "running",
      available_commands: [{ type: "command", name: "echo", description: "Echo" }],
    });
    const core = await viewSession(task);
    await userEvent.type(message(), "/");
    expect(screen.getByRole("listbox", { name: "Commands" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("listbox", { name: "Commands" })).not.toBeInTheDocument();
    expect(core.invoked("cancel")).toEqual([]);
    await userEvent.keyboard("{Escape}");
    expect(core.invoked("cancel")).toEqual([{ sessionId: task.id }]);
  });

  it("leaves the turn running when Escape closes a dialog", async () => {
    const core = await viewSession(session(project, { title: "Task", state: "running" }));
    const dialog = document.body.appendChild(document.createElement("dialog"));
    dialog.showModal();
    await userEvent.keyboard("{Escape}");
    expect(core.invoked("cancel")).toEqual([]);
    dialog.remove();
  });

  it("is disabled once the agent has exited", async () => {
    await viewSession(session(project, { title: "Task", state: "exited", agent_session_id: null }));
    expect(message()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Attach Files" })).toBeDisabled();
  });
});

describe("The queue", () => {
  const queued = (id: string, text: string, ...paths: string[]) => ({
    id,
    prompt: [
      { type: "text", text },
      ...paths.map((path) => ({
        type: "resource_link",
        name: path.split("/").pop()!,
        uri: `file://${path.split("/").map(encodeURIComponent).join("/")}`,
      })),
    ],
  });
  const row = (text: string) =>
    within(screen.getByRole("list", { name: "Queued Messages" }))
      .getByText(text)
      .closest("li")!;

  it("sends a queued prompt now or deletes it", async () => {
    const task = session(project, {
      title: "Task",
      state: "running",
      queue: [queued("q1", "First"), queued("q2", "Second")],
    });
    const core = await viewSession(task);
    await userEvent.click(within(row("First")).getByRole("button", { name: "Send Now" }));
    expect(core.invoked("queue_send")).toEqual([{ sessionId: task.id, promptId: "q1" }]);
    await userEvent.click(within(row("Second")).getByRole("button", { name: "Delete" }));
    expect(core.invoked("queue_remove")).toEqual([{ sessionId: task.id, promptId: "q2" }]);
  });

  it("edits a queued prompt in place and brings the draft back", async () => {
    const task = session(project, {
      title: "Task",
      state: "running",
      queue: [queued("q1", "First", "/src/my notes.txt")],
    });
    const core = await viewSession(task);
    await userEvent.type(message(), "draft");
    await userEvent.click(within(row("First")).getByRole("button", { name: "Edit" }));
    expect(message()).toHaveValue("First");
    expect(screen.getByTitle("/src/my notes.txt")).toBeInTheDocument();
    await userEvent.type(message(), " edited{Enter}");
    expect(core.invoked("queue_edit")).toEqual([
      {
        sessionId: task.id,
        promptId: "q1",
        text: "First edited",
        attachments: ["/src/my notes.txt"],
      },
    ]);
    expect(core.invoked("queue_add")).toEqual([]);
    expect(message()).toHaveValue("draft");
    expect(screen.queryByTitle("/src/my notes.txt")).not.toBeInTheDocument();
  });

  it("keeps editing when the edit cannot be saved", async () => {
    const task = session(project, {
      title: "Task",
      state: "running",
      queue: [queued("q1", "First")],
    });
    const core = await viewSession(task);
    core.on("queue_edit", () => {
      throw "prompt q1 is not queued";
    });
    await userEvent.click(within(row("First")).getByRole("button", { name: "Edit" }));
    await userEvent.clear(message());
    await userEvent.type(message(), "Changed{Enter}");
    expect(core.invoked("queue_edit")).toHaveLength(1);
    expect(message()).toHaveValue("Changed");
  });

  it("queues an edit whose prompt was sent meanwhile as a new message", async () => {
    const task = session(project, {
      title: "Task",
      state: "running",
      queue: [queued("q1", "First")],
    });
    const core = await viewSession(task);
    await userEvent.type(message(), "draft");
    await userEvent.click(within(row("First")).getByRole("button", { name: "Edit" }));
    core.list({ type: "session", session: { ...task, queue: [] } });
    await userEvent.type(message(), "!{Enter}");
    expect(core.invoked("queue_edit")).toEqual([]);
    expect(core.invoked("queue_add")).toEqual([
      { sessionId: task.id, text: "First!", attachments: [] },
    ]);
    expect(message()).toHaveValue("draft");
  });
});

describe("A permission request", () => {
  it("is answered from above the composer", async () => {
    const task = session(project, { title: "Task", state: "awaiting_permission" });
    const core = await viewSession(task);
    core.update(task.id, {
      type: "permission",
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
    core.update(task.id, { type: "removed", key: "request:7" });
    expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
  });

  it("shows its description under its title", async () => {
    const task = session(project, { title: "Task", state: "awaiting_permission" });
    const core = await viewSession(task);
    core.update(task.id, {
      type: "permission",
      key: "request:1",
      request_id: "1",
      title: "Run `rm -rf build`",
      description: "Deletes the build directory.\nIt is rebuilt on the next build.",
      options: [{ optionId: "yes", name: "Allow", kind: "allow_once" }],
    });
    const panel = document.querySelector(".request-panel")!;
    expect(panel.querySelector(".title")).toHaveTextContent("Run rm -rf build");
    expect(panel.querySelector(".title + .description")!.textContent).toBe(
      "Deletes the build directory.\nIt is rebuilt on the next build.",
    );
  });
});

describe("A session request", () => {
  it("shows the requested session's model, mode, checkout and prompt", async () => {
    const task = session(project, { title: "Task", state: "awaiting_permission" });
    const core = await viewSession(task);
    core.update(task.id, {
      type: "permission",
      key: "request:1",
      request_id: "1",
      title: "Start a session in ox?",
      session_request: {
        harness: "claude",
        model: "Opus",
        provider: "Anthropic",
        effort: "High",
        mode: "Manual",
        checkout: { type: "worktree", branch: null, base: "main" },
        prompt: "Review the change.",
      },
      options: [{ optionId: "start", name: "Start Session", kind: "allow_once" }],
    });
    const panel = document.querySelector(".request-panel")!;
    const pairs = [...panel.querySelectorAll(".subheader-meta > div")].map(
      (pair) => pair.textContent,
    );
    expect(pairs).toEqual(["Claude CodeOpusAnthropicHighManual", "New worktree frommain"]);
    expect(panel.querySelector(".subheader-meta + .markdown")).toHaveTextContent(
      "Review the change.",
    );
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
  const setting = (config_id: string, value: string): ConfigSetting => ({ config_id, value });
  const config = [setting("model", "smart"), setting("effort", "medium"), setting("mode", "ask")];
  const picker = () => document.querySelector("button.model-picker")!;
  const models = () => screen.getByRole("listbox", { name: "Model", hidden: true });
  const choose = (name: string) => userEvent.click(within(models()).getByText(name));
  const sent = (core: Core) =>
    core
      .invoked("set_config")
      .map(({ config }) =>
        (config as ConfigSetting[]).map(({ config_id, value }) => `${config_id} ${value}`),
      );

  async function withConfig() {
    const task = session(project, { title: "Task", config });
    const core = await viewSession(task);
    return { core, task };
  }

  it("shows the session's config", async () => {
    await withConfig();
    expect(picker()).toHaveTextContent("SmartMedium");
    expect(picker()).toBeEnabled();
    expect(screen.getByRole("combobox", { name: "Mode" })).toHaveDisplayValue("Ask");
  });

  it("shows a starting session's config disabled", async () => {
    const task = session(project, {
      title: "Task",
      state: "starting",
      agent_session_id: null,
      config: [setting("model", "fast"), setting("effort", "low"), setting("mode", "code")],
    });
    await viewSession(task);
    expect(picker()).toBeDisabled();
    expect(picker()).toHaveTextContent("FastOpenRouterLow");
    expect(screen.getByRole("combobox", { name: "Mode" })).toHaveDisplayValue("Code");
    expect(screen.getByRole("combobox", { name: "Mode" })).toBeDisabled();
  });

  it("shows an exited session's config disabled", async () => {
    await viewSession(session(project, { title: "Task", state: "exited", config }));
    expect(picker()).toBeDisabled();
    expect(picker()).toHaveTextContent("SmartMedium");
    expect(screen.getByRole("combobox", { name: "Mode" })).toBeDisabled();
  });

  it("shows a chosen model at once, until the agent refuses it", async () => {
    const { core, task } = await withConfig();
    let refuse: (error: string) => void = () => {};
    core.on(
      "set_config",
      () => new Promise((_, reject) => (refuse = reject as (error: string) => void)),
    );
    await choose("Fast");
    expect(picker()).toHaveTextContent("FastOpenRouterMedium");
    expect(core.invoked("set_config")).toEqual([
      {
        sessionId: task.id,
        config: [setting("model", "fast"), setting("effort", "medium"), setting("mode", "ask")],
      },
    ]);
    await act(async () => refuse("unavailable"));
    expect(picker()).toHaveTextContent("SmartMedium");
  });

  it("sends a new model with the effort the draft rule picks for it", async () => {
    const { core } = await withConfig();
    await choose("Deep");
    await choose("Tiny");
    expect(sent(core)).toEqual([
      ["model deep", "effort high", "mode ask"],
      ["model tiny", "mode ask"],
    ]);
  });

  it("chooses with the keyboard and sets the effort with the slider", async () => {
    const { core } = await withConfig();
    for (const key of ["ArrowDown", "ArrowUp", "Enter"]) fireEvent.keyDown(models(), { key });
    expect(picker()).toHaveTextContent("FastOpenRouterMedium");
    const slider = screen.getByRole("slider", { name: "Effort", hidden: true });
    fireEvent.change(slider, { target: { value: "2" } });
    expect(picker()).toHaveTextContent("FastOpenRouterHigh");
    // The effort builds on the model sent before the session's config has it.
    expect(sent(core)).toEqual([
      ["model fast", "effort medium", "mode ask"],
      ["model fast", "effort high", "mode ask"],
    ]);
  });

  it("offers only the models the config lists, at the one the session records", async () => {
    const { core, task } = await withConfig();
    expect([...models().querySelectorAll("button")].map((button) => button.textContent)).toEqual([
      "FastOpenRouter",
      "Smart",
      "Deep",
      "Tiny",
    ]);
    core.list({ type: "session", session: { ...task, config: [setting("model", "other/one")] } });
    expect(picker()).toHaveTextContent("other/one");
    expect(models().querySelectorAll("button")).toHaveLength(4);
  });

  it("shows a recorded effort or mode the config does not list as its ID", async () => {
    const { core, task } = await withConfig();
    core.list({
      type: "session",
      session: {
        ...task,
        config: [setting("model", "deep"), setting("effort", "medium"), setting("mode", "plan")],
      },
    });
    expect(picker()).toHaveTextContent("Deepmedium");
    expect(screen.getByRole("slider", { name: "Effort", hidden: true })).toHaveValue("2");
    expect(screen.getByRole("combobox", { name: "Mode" })).toHaveDisplayValue("plan");
  });

  it("keeps recorded settings the config does not list when another changes", async () => {
    const { core, task } = await withConfig();
    core.list({
      type: "session",
      session: {
        ...task,
        config: [setting("model", "other/one"), setting("effort", "max"), setting("mode", "plan")],
      },
    });
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Mode" }), "code");
    expect(sent(core)).toEqual([["model other/one", "effort max", "mode code"]]);
  });

  it("shows the mode as its own picker", async () => {
    const { core, task } = await withConfig();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Mode" }), "code");
    expect(core.invoked("set_config")).toEqual([
      {
        sessionId: task.id,
        config: [setting("model", "smart"), setting("effort", "medium"), setting("mode", "code")],
      },
    ]);
  });
});
