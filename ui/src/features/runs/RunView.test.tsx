import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { RunInfo } from "../../lib/protocol";
import { renderApp, run, session, view, workspace, worktree } from "../../test/core";

const repo = workspace({ git_working_tree: true });
const tree = worktree();

/** Renders the app with `steps`' run viewed, each agent step with its child session. */
async function viewRun(steps: RunInfo["steps"], fields: Partial<RunInfo> = {}) {
  const children = steps.flatMap((step) =>
    step.session_id === null ? [] : [session(repo, { id: step.session_id, worktree_id: tree.id })],
  );
  const viewed = run(repo, tree, steps, fields);
  const core = await renderApp(
    { type: "workspace", workspace: repo },
    { type: "worktree", worktree: tree },
    ...children.map((child) => ({ type: "session" as const, session: child })),
    { type: "run", run: viewed },
  );
  await view("Implement");
  return { core, run: viewed };
}

const step = (name: string) => screen.getByRole("region", { name });
const toggle = (name: string) => within(step(name)).getByRole("button");
const messages = (name: string) =>
  [...step(name).querySelectorAll(".run-status-messages li")].map((item) => item.textContent);
const say = (key: string, text: string) => ({ type: "text", key, text, replace: false }) as const;

describe("A run's steps", () => {
  it("show their agent messages and collapse once a later step reports", async () => {
    const first = {
      name: "Report",
      state: "running",
      started_at: 1,
      session_id: "child-1",
    } as const;
    const second = {
      name: "Check",
      state: "pending",
      started_at: null,
      session_id: "child-2",
    } as const;
    const { core, run: viewed } = await viewRun([first, second]);
    core.update("child-1", say("a", "Reading the code"));
    core.update("child-1", say("b", "Writing the fix"));
    expect(messages("Report")).toEqual(["Reading the code", "Writing the fix"]);

    // A succeeded step stays expanded until a later step reports.
    core.list({
      type: "run",
      run: {
        ...viewed,
        steps: [
          { ...first, state: "succeeded" },
          { ...second, state: "running", started_at: 2 },
        ],
      },
    });
    expect(toggle("Report")).toHaveAttribute("aria-expanded", "true");
    core.update("child-2", say("c", "Checking the fix"));
    expect(toggle("Report")).toHaveAttribute("aria-expanded", "false");
    expect(messages("Report")).toEqual([]);
    expect(messages("Check")).toEqual(["Checking the fix"]);

    await userEvent.click(toggle("Report"));
    expect(messages("Report")).toEqual(["Reading the code", "Writing the fix"]);
  });

  it("show a failure under the step that failed", async () => {
    await viewRun([{ name: "Guarded", state: "failed", started_at: 1, session_id: null }], {
      state: "failed",
      error: "a precondition exited with status 3",
    });
    expect(messages("Guarded")).toEqual(["a precondition exited with status 3"]);
  });

  it("take an answer to a child's permission request", async () => {
    const { core } = await viewRun([
      { name: "Ask", state: "running", started_at: 1, session_id: "child-3" },
    ]);
    core.update("child-3", {
      type: "permission",
      key: "request:1",
      request_id: "1",
      title: "Run `ls`",
      options: [{ optionId: "yes", name: "Allow", kind: "allow_once" }],
    });
    await userEvent.click(screen.getByRole("button", { name: "Allow" }));
    expect(core.invoked("permit")).toEqual([
      { sessionId: "child-3", requestId: "1", optionId: "yes" },
    ]);
  });
});
