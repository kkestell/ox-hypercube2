import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import {
  claudeConfig,
  piConfig,
  renderApp,
  session,
  sidebarRow,
  view,
  workspace,
} from "../../test/core";

/** The status dot of the sidebar row titled `title`: its class and label. */
function dot(title: string) {
  const status = sidebarRow(title).querySelector(".status:not(.branch-status)")!;
  return { className: status.className, label: status.getAttribute("aria-label") };
}

describe("The session list", () => {
  it("groups a workspace's sessions under its folder's name, newest first", async () => {
    const project = workspace({ path: "/src/Project" });
    await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: session(project, { title: "Older" }) },
      { type: "session", session: session(project) },
    );
    const group = screen.getByRole("region", { name: "Project" });
    const titles = within(group)
      .getAllByRole("listitem")
      .map((row) => row.querySelector(".title")!.textContent);
    expect(titles).toEqual(["New session", "Older"]);
  });

  it("shows what each session needs from the user", async () => {
    const project = workspace();
    const cases = [
      ["Waiting", { state: "awaiting_permission" }, "status attention", "Awaiting permission"],
      ["Broken", { unseen: "error" }, "status error", "Error"],
      ["Working", { state: "running", unseen: "finished" }, "status running", "Running"],
      ["Done", { unseen: "finished" }, "status finished", "Finished"],
      ["Seen", {}, "status", "Idle"],
    ] as const;
    await renderApp(
      { type: "workspace", workspace: project },
      ...cases.map(([title, fields]) => ({
        type: "session" as const,
        session: session(project, { title, ...fields }),
      })),
    );
    for (const [title, , className, label] of cases) {
      expect(dot(title)).toEqual({ className, label });
    }
  });

  it("offers a choice of harness only while more than one is installed", async () => {
    const project = workspace();
    const core = await renderApp({ type: "workspace", workspace: project });
    const newSession = screen.getByRole("button", { name: "New Session" });
    expect(screen.queryByLabelText("Harness")).not.toBeInTheDocument();

    core.agents = [claudeConfig, piConfig];
    await act(async () => window.dispatchEvent(new Event("focus")));
    expect(screen.getByLabelText("Harness")).toBeInTheDocument();

    core.agents = [];
    await act(async () => window.dispatchEvent(new Event("focus")));
    expect(newSession).toBeDisabled();
  });

  it("tells oxd which session is in front while the window is", async () => {
    const project = workspace();
    const task = session(project, { title: "Task" });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: task },
    );
    await view("Task");
    const inFront = { sessionId: task.id };
    expect(core.invoked("focus").at(-1)).toEqual(inFront);
    await act(async () => window.dispatchEvent(new Event("blur")));
    expect(core.invoked("focus").at(-1)).toEqual({ sessionId: null });
    await act(async () => window.dispatchEvent(new Event("focus")));
    expect(core.invoked("focus").at(-1)).toEqual(inFront);
  });

  it("views a saved session by loading its agent", async () => {
    const project = workspace();
    const saved = session(project, { title: "Saved", state: "exited" });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: saved },
    );
    await view("Saved");
    expect(core.invoked("load_session")).toEqual([{ sessionId: saved.id }]);
  });
});

describe("A session's context menu", () => {
  it("offers only Delete while the session starts", async () => {
    const project = workspace();
    const starting = session(project, { title: "Starting", state: "starting" });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: starting },
    );
    expect(await core.contextMenu(sidebarRow("Starting"))).toEqual({
      "Archive Session": false,
      "Delete Session": true,
    });
  });

  it("asks before archiving unsaved work, then archives discarding it", async () => {
    const project = workspace();
    const target = session(project, { title: "Task" });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: target },
    );
    core.on("archive_session", ({ discard }) =>
      discard ? { type: "archived" } : { type: "unsaved_work", message: "notes.txt is unsaved" },
    );
    await core.choose(sidebarRow("Task"), "Archive Session");
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("notes.txt is unsaved");
    await userEvent.click(within(dialog).getByRole("button", { name: "Discard and Archive" }));
    expect(core.invoked("archive_session")).toEqual([
      { sessionId: target.id, discard: false },
      { sessionId: target.id, discard: true },
    ]);
  });

  it("shows archived sessions only while the workspace shows them", async () => {
    const project = workspace({ path: "/src/project" });
    const archived = session(project, { title: "Archived", archived: true });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: archived },
    );
    expect(screen.queryByText("Archived")).not.toBeInTheDocument();
    const header = screen.getByRole("heading", { name: "project" });
    await core.choose(header, "Show Archived");
    await core.choose(sidebarRow("Archived"), "Unarchive Session");
    expect(core.invoked("unarchive_session")).toEqual([{ sessionId: archived.id }]);
    await core.choose(header, "Show Archived");
    expect(screen.queryByText("Archived")).not.toBeInTheDocument();
  });

  it("imports an added workspace's Claude Code sessions", async () => {
    const core = await renderApp();
    const project = workspace({ path: "/src/project" });
    core.on("plugin:dialog|open", () => project.path);
    core.on("add_workspace", () => project);
    await userEvent.click(screen.getByRole("button", { name: "Add Workspace" }));
    expect(core.invoked("add_workspace")).toEqual([{ path: project.path }]);
    expect(core.invoked("import_claude_sessions")).toEqual([{ workspaceId: project.id }]);
  });

  it("reports how many Claude Code sessions the heading's import added", async () => {
    const project = workspace({ path: "/src/project" });
    const core = await renderApp({ type: "workspace", workspace: project });
    const header = screen.getByRole("heading", { name: "project" });
    for (const [imported, text] of [
      [2, "Imported 2 sessions"],
      [0, "No new Claude Code sessions"],
    ] as const) {
      core.on("import_claude_sessions", () => imported);
      await core.choose(header, "Import Claude Code Sessions");
      expect(core.invoked("plugin:dialog|message").at(-1)).toMatchObject({
        message: text,
        kind: "info",
      });
    }
    expect(core.invoked("import_claude_sessions")).toEqual([
      { workspaceId: project.id },
      { workspaceId: project.id },
    ]);
  });

  it("removes a workspace of archived sessions once the user confirms", async () => {
    const project = workspace({ path: "/src/project" });
    const archived = { archived: true };
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: session(project, archived) },
      { type: "session", session: session(project, archived) },
    );
    const header = screen.getByRole("heading", { name: "project" });
    expect((await core.contextMenu(header))["Remove Workspace"]).toBe(true);

    // The dialog answers with the label of the button chosen.
    core.on("plugin:dialog|message", () => "Cancel");
    await core.choose(header, "Remove Workspace");
    expect(core.invoked("plugin:dialog|message").at(-1)).toMatchObject({
      message: "Remove this workspace and delete its 2 archived sessions?",
    });
    expect(core.invoked("remove_workspace")).toEqual([]);

    core.on("plugin:dialog|message", () => "Remove");
    await core.choose(header, "Remove Workspace");
    expect(core.invoked("remove_workspace")).toEqual([{ workspaceId: project.id }]);
  });

  it("keeps a workspace with a session that is not archived", async () => {
    const project = workspace({ path: "/src/project" });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "session", session: session(project, { archived: true }) },
      { type: "session", session: session(project) },
    );
    const header = screen.getByRole("heading", { name: "project" });
    expect((await core.contextMenu(header))["Remove Workspace"]).toBe(false);
  });
});
