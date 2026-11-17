import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { renderApp, run, session, sidebarRow, view, workspace, worktree } from "../../test/core";

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

  it("shows a failed run and its failed step in red until the run is seen", async () => {
    const project = workspace({ git_working_tree: true });
    const tree = worktree();
    const failed = run(project, tree, [{ name: "Break", state: "failed", started_at: 1 }], {
      state: "failed",
      unseen: "error",
    });
    const core = await renderApp(
      { type: "workspace", workspace: project },
      { type: "worktree", worktree: tree },
      { type: "run", run: failed },
    );
    const row = sidebarRow("Implement");
    await act(async () => row.querySelector<HTMLElement>(".disclosure")!.click());
    expect(dot("Implement")).toEqual({ className: "status error", label: "Failed" });
    expect(dot("Break")).toEqual({ className: "status error", label: "Failed" });

    core.list({ type: "run", run: { ...failed, unseen: null } });
    expect(dot("Implement")).toEqual({ className: "status", label: "Failed" });
    expect(dot("Break")).toEqual({ className: "status", label: "Failed" });
  });

  it("starts a session in the checkout, or in a worktree compared with the checkout's branch", async () => {
    const plain = workspace({ path: "/src/plain" });
    const repo = workspace({ path: "/src/repo", git_working_tree: true });
    const core = await renderApp(
      { type: "workspace", workspace: plain },
      { type: "workspace", workspace: repo },
    );
    const menu = (name: string) =>
      within(screen.getByRole("region", { name })).getByRole("menu", { hidden: true });
    const items = (name: string) =>
      within(menu(name))
        .getAllByRole("menuitem", { hidden: true })
        .map((item) => item.textContent);
    expect(items("plain")).toEqual(["Session in Checkout"]);
    expect(items("repo")).toEqual(["Session in Worktree", "Session in Checkout"]);

    const started = session(repo, { title: "Started" });
    core.on("new_session", () => started);
    await userEvent.click(within(menu("repo")).getByText("Session in Worktree"));
    expect(core.invoked("new_session")).toEqual([
      { agent: "claude", workspaceId: repo.id, checkout: { type: "worktree", compare: "main" } },
    ]);
    await userEvent.click(within(menu("plain")).getByText("Session in Checkout"));
    expect(core.invoked("new_session")[1]).toEqual({
      agent: "claude",
      workspaceId: plain.id,
      checkout: { type: "workspace" },
    });
    core.list({ type: "session", session: started });
    expect(sidebarRow("Started").querySelector("button.select")).toHaveAttribute(
      "aria-current",
      "true",
    );
  });

  it("offers a choice of harness only while more than one is installed", async () => {
    const project = workspace();
    const core = await renderApp({ type: "workspace", workspace: project });
    const newSession = screen.getByRole("button", { name: "New Session" });
    expect(screen.queryByLabelText("Harness")).not.toBeInTheDocument();

    core.agents = ["claude", "pi"];
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
    const inFront = { focus: { type: "session", session_id: task.id } };
    expect(core.invoked("focus").at(-1)).toEqual(inFront);
    await act(async () => window.dispatchEvent(new Event("blur")));
    expect(core.invoked("focus").at(-1)).toEqual({ focus: null });
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
});
