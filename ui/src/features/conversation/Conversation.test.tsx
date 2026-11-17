import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ConversationUpdate } from "../../lib/protocol";
import { renderApp, session, view, workspace } from "../../test/core";

const project = workspace();

/** Views a session whose conversation receives `updates` in one batch. */
async function conversation(...updates: ConversationUpdate[]) {
  const task = session(project, { title: "Task" });
  const core = await renderApp(
    { type: "workspace", workspace: project },
    { type: "session", session: task },
  );
  await view("Task");
  core.update(task.id, ...updates);
  return { core, task, transcript: document.querySelector(".conversation")! };
}

const text = (key: string, value: string, replace = false): ConversationUpdate => ({
  type: "text",
  recorded_at: 0,
  key,
  text: value,
  replace,
});

describe("Transcript Markdown", () => {
  it("renders blocks and GitHub's extensions, with code kept literal", async () => {
    const { transcript } = await conversation(
      text(
        "reply",
        "# Heading\n\nFirst *emphasis* and ~~removed~~.\nsoft break\n\nhard  \nbreak\n\n" +
          "> A quote\n\n1. First\n2. Second\n\n- [ ] Todo\n- [x] Done\n\n" +
          "```rust\nfn main() {\n    **literal**();\n}\n```\n\n" +
          "| Name | Value |\n| --- | --- |\n| one | two |\n\nhttps://example.com",
      ),
    );
    const reply = transcript.querySelector(".markdown")!;
    expect(reply.querySelector("h1")).toHaveTextContent("Heading");
    expect(reply.querySelector("em")).toHaveTextContent("emphasis");
    expect(reply.querySelector("del")).toHaveTextContent("removed");
    expect(reply.querySelector("blockquote")).toHaveTextContent("A quote");
    expect(reply.querySelectorAll("ol li")).toHaveLength(2);
    expect(reply.querySelector("input:checked")).toBeDisabled();
    expect(reply.querySelector("pre code")!.textContent).toBe(
      "fn main() {\n    **literal**();\n}\n",
    );
    expect(reply.querySelector("td")).toHaveTextContent("one");
    expect(reply.querySelector("a")).toHaveAttribute("href", "https://example.com");
    // A soft break is whitespace; only a hard break adds br.
    expect(reply.querySelectorAll("br")).toHaveLength(1);
  });

  it("shows HTML as text and keeps links other than web and email inert", async () => {
    const { transcript } = await conversation(
      text(
        "reply",
        "<script>alert(1)</script>\n\n[unsafe](javascript:alert%281%29) [file](file:///tmp/a) " +
          "[relative](src/main.rs) [web](https://example.com) [email](mailto:a@example.com)",
      ),
    );
    const reply = transcript.querySelector(".markdown")!;
    expect(reply).toHaveTextContent("<script>alert(1)</script>");
    expect(reply.querySelector("script")).toBeNull();
    expect([...reply.querySelectorAll("a")].map((a) => a.getAttribute("href"))).toEqual([
      "https://example.com",
      "mailto:a@example.com",
    ]);
  });

  it("opens web links with the system's opener", async () => {
    const { core } = await conversation(text("reply", "[Docs](HtTpS://EXAMPLE.COM/docs)"));
    await userEvent.click(screen.getByRole("link", { name: "Docs" }));
    expect(core.invoked("plugin:opener|open_url")).toEqual([{ url: "https://example.com/docs" }]);
  });

  it("completes Markdown split across streamed chunks", async () => {
    const { core, task, transcript } = await conversation(text("reply", "**"), {
      type: "thought",
      recorded_at: 0,
      key: "thought",
      text: "**Reason",
      replace: false,
    });
    expect(transcript.querySelector(".markdown")).toHaveTextContent("**");
    core.update(task.id, text("reply", "done**"), {
      type: "thought",
      recorded_at: 0,
      key: "thought",
      text: "ing**",
      replace: false,
    });
    expect(transcript.querySelector(":scope > .markdown strong")).toHaveTextContent("done");
    expect(transcript.querySelector(".thought strong")).toHaveTextContent("Reasoning");
    core.update(task.id, text("reply", "Replaced", true));
    expect(transcript.querySelector(":scope > .markdown")).toHaveTextContent(/^Replaced$/);
  });
});

describe("A conversation", () => {
  it("is replayed once on a new watch after oxd's connection drops", async () => {
    const { core, task, transcript } = await conversation(text("reply", "Before"));
    core.disconnect();
    core.list({ type: "session", session: { ...task, state: "exited" } });
    core.update(task.id, text("reply", "Before"));
    expect(transcript.querySelectorAll(".markdown")).toHaveLength(1);
    expect(transcript).toHaveTextContent(/^Before$/);
  });
});

describe("A turn", () => {
  const work: ConversationUpdate[] = [
    { type: "prompt", recorded_at: 0, key: "prompt", text: "Fix it", attachments: [] },
    { type: "thought", recorded_at: 0, key: "thought", text: "Planning", replace: false },
    text("progress", "Checking the tests"),
    {
      type: "tool_call",
      recorded_at: 0,
      key: "tool",
      title: "Read `a.rs`",
      status: "completed",
      raw_input: { path: "a.rs" },
      content: [
        { type: "text", text: "**literal** `code`" },
        { type: "image", mime_type: "image/png", data: "AAAA" },
      ],
    },
  ];
  const children = (transcript: Element) =>
    [...transcript.children].map((child) => child.className);

  it("folds its work under Worked once it completes with a reply", async () => {
    const { transcript } = await conversation(...work, text("reply", "Done"), {
      type: "turn_completed",
      recorded_at: 0,
      key: "end",
    });
    expect(children(transcript)).toEqual(["prompt", "worked", "text markdown"]);
    const toggle = screen.getByRole("button", { name: "Worked" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(toggle);
    const items = transcript.querySelector(".worked-items")!;
    expect([...items.children].map((child) => child.className)).toEqual([
      "thought",
      "text markdown",
      "tool-call",
    ]);
  });

  it("keeps its work in view when it ends in an error", async () => {
    const { transcript } = await conversation(...work, {
      type: "turn_error",
      recorded_at: 0,
      key: "error",
      message: "broken",
    });
    expect(children(transcript)).toEqual([
      "prompt",
      "thought",
      "text markdown",
      "tool-call",
      "turn-error",
    ]);
    expect(screen.getByText("broken")).toBeInTheDocument();
  });

  it("shows a tool call's arguments and output literally", async () => {
    const { transcript } = await conversation(...work);
    const tool = transcript.querySelector(".tool-call")!;
    expect(tool.querySelector(".title code")).toHaveTextContent("a.rs");
    expect(tool.querySelector('[aria-label="Completed"]')).toBeInTheDocument();
    const [input, output] = tool.querySelectorAll("pre");
    expect(input!.textContent).toBe('{\n  "path": "a.rs"\n}');
    expect(output!.textContent).toBe("**literal** `code`");
    expect(tool.querySelector("img")).toHaveAttribute("src", "data:image/png;base64,AAAA");
  });

  it("reports that the agent exited", async () => {
    await conversation({ type: "agent_exited", recorded_at: 0, key: "exit", error: "status 3" });
    expect(screen.getByText("The agent exited: status 3")).toBeInTheDocument();
  });
});
