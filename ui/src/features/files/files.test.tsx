import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { renderApp, session, view, workspace } from "../../test/core";
import { highlightFile } from "./highlight";

/** The file tree's row named `name`. */
const row = (name: string) =>
  screen.getByText(name, { selector: ".tree-row .title" }).closest("button")!;
const body = () => document.querySelector(".file-body")!;

/** Two sessions, each in a folder of its own, with the first viewed. */
async function twoSessions() {
  const first = workspace({ path: "/src/first" });
  const second = workspace({ path: "/src/second" });
  const core = await renderApp(
    { type: "workspace", workspace: first },
    { type: "workspace", workspace: second },
    { type: "session", session: session(first, { title: "First" }) },
    { type: "session", session: session(second, { title: "Second" }) },
  );
  await view("First");
  return core;
}

describe("The file tree", () => {
  it("follows the viewed session's folder and keeps the folders it opened", async () => {
    const core = await twoSessions();
    core.directory("/src/first", [
      { name: "sub", kind: "directory" },
      { name: "a.txt", kind: "file" },
    ]);
    core.directory("/src/first", [
      { name: "sub", kind: "directory" },
      { name: "a.txt", kind: "file" },
      { name: "b.txt", kind: "file" },
    ]);
    expect(row("b.txt")).toBeInTheDocument();
    await userEvent.click(row("sub"));
    core.directory("/src/first/sub", [{ name: "inner.txt", kind: "file" }]);
    expect(row("inner.txt")).toBeInTheDocument();

    await view("Second");
    expect(screen.queryByText("sub")).not.toBeInTheDocument();
    await view("First");
    // A new watch lists the folder first.
    core.directory("/src/first", [{ name: "sub", kind: "directory" }]);
    expect(row("sub")).toHaveAttribute("aria-expanded", "true");
  });
});

describe("The file viewer", () => {
  it.each([
    ["main.rs", "fn main() {}", ".hljs-keyword", "fn"],
    ["App.tsx", "const x = 1;", ".hljs-keyword", "const"],
    ["Makefile", "all: build\n\techo hi", ".hljs-section", "all: build"],
  ])("highlights %s by its name", (name, text, selector, token) => {
    const { container } = render(<pre>{highlightFile(name, text)}</pre>);
    expect(container.querySelector(selector)).toHaveTextContent(token);
    expect(container.textContent).toBe(text);
  });

  it("shows other files as plain text", () => {
    const { container } = render(<pre>{highlightFile("notes.txt", "const fn")}</pre>);
    expect(container.querySelector('[class*="hljs-"]')).toBeNull();
  });

  it("follows the open file's edits and keeps each session's file", async () => {
    const core = await twoSessions();
    core.directory("/src/first", [{ name: "main.rs", kind: "file" }]);
    await userEvent.click(row("main.rs"));
    core.file("/src/first/main.rs", "fn main() {}\n");
    expect(body().querySelector("pre")!.textContent).toBe("fn main() {}\n");
    core.file("/src/first/main.rs", "fn main() { let value = 1; }\n");
    expect(body().querySelector("pre")!.textContent).toBe("fn main() { let value = 1; }\n");

    await view("Second");
    expect(document.querySelector(".file-viewer")).toBeNull();
    await view("First");
    expect(body().querySelector("pre")!.textContent).toBe("fn main() { let value = 1; }\n");

    await userEvent.click(screen.getByRole("button", { name: "Close file" }));
    expect(document.querySelector(".file-viewer")).toBeNull();
    const watch = core.invoked("watch_file")[0]!.channel as { id: number };
    expect(core.invoked("unwatch")).toContainEqual({ channelId: watch.id });
  });

  it("previews Markdown and flips to its source", async () => {
    const core = await twoSessions();
    core.directory("/src/first", [{ name: "notes.md", kind: "file" }]);
    await userEvent.click(row("notes.md"));
    core.file("/src/first/notes.md", "# Title\n");
    expect(body().querySelector("h1")).toHaveTextContent("Title");
    await userEvent.click(screen.getByRole("button", { name: "Preview" }));
    expect(body().querySelector("pre")!.textContent).toBe("# Title\n");
    await userEvent.click(screen.getByRole("button", { name: "Preview" }));
    expect(body().querySelector("h1")).toHaveTextContent("Title");
  });
});
