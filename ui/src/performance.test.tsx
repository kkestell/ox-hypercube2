/**
 * The work a change costs follows what changed, however many sessions and
 * replies there are (docs/dev/performance.md). Each test counts the work
 * itself: Markdown parses (`fromMarkdown`), Markdown block renders
 * (`toHast`), and the renders of the rows of sessions out of view
 * (`localMerge`, which each row and the viewed session's header call once
 * per render). A change that renders or parses in proportion to the session
 * list or the transcript fails by hundreds.
 */
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { fromMarkdown } from "mdast-util-from-markdown";
import { toHast } from "mdast-util-to-hast";
import { afterEach, describe, expect, it, vi } from "vitest";
import { localMerge } from "./features/sessions/localMerge";
import type { ConversationUpdate, SessionInfo } from "./lib/protocol";
import { renderApp, session, view, workspace } from "./test/core";

vi.mock("mdast-util-from-markdown", async (original) => {
  const module = await original<typeof import("mdast-util-from-markdown")>();
  return { ...module, fromMarkdown: vi.fn(module.fromMarkdown) };
});
vi.mock("mdast-util-to-hast", async (original) => {
  const module = await original<typeof import("mdast-util-to-hast")>();
  return { ...module, toHast: vi.fn(module.toHast) };
});
vi.mock("./features/sessions/localMerge", async (original) => {
  const module = await original<typeof import("./features/sessions/localMerge")>();
  return { ...module, localMerge: vi.fn(module.localMerge) };
});

const SESSIONS = 300;
const REPLIES = 300;
const project = workspace();

/** The session in view, whose row and header are left out of `rows`. */
let viewed: string;

/** Counts the work `action` does. */
async function work(action: () => unknown) {
  for (const fn of [fromMarkdown, toHast, localMerge]) vi.mocked(fn).mockClear();
  await action();
  return {
    parses: vi.mocked(fromMarkdown).mock.calls.length,
    blocks: vi.mocked(toHast).mock.calls.length,
    rows: vi.mocked(localMerge).mock.calls.filter(([session]) => session.id !== viewed).length,
  };
}

/** A completed turn for each of `count` two-paragraph replies. */
function replies(count: number): ConversationUpdate[] {
  return Array.from({ length: count }, (_, index): ConversationUpdate[] => [
    {
      type: "prompt",
      recorded_at: 0,
      key: `prompt:${index}`,
      text: `Prompt ${index}`,
      attachments: [],
    },
    {
      type: "text",
      recorded_at: 0,
      key: `reply:${index}`,
      text: `First ${index}\n\nReply **${index}**`,
      replace: false,
    },
    { type: "turn_completed", recorded_at: 0, key: `end:${index}` },
  ]).flat();
}

/** A workspace of `SESSIONS` sessions, the first, "Long", viewed with `REPLIES` replies. */
async function bigWorkspace() {
  const sessions: SessionInfo[] = Array.from({ length: SESSIONS }, (_, index) =>
    session(project, { title: index === 0 ? "Long" : `Session ${index}` }),
  );
  const core = await renderApp(
    { type: "workspace", workspace: project },
    ...sessions.map((session) => ({ type: "session" as const, session })),
  );
  await view("Long");
  viewed = sessions[0]!.id;
  const opened = await work(() => core.update(sessions[0]!.id, ...replies(REPLIES)));
  return { core, sessions, opened };
}

afterEach(() => {
  vi.clearAllMocks();
});

describe("Work", () => {
  it("renders each reply once when a transcript opens", async () => {
    const { opened } = await bigWorkspace();
    // A prompt and a reply each parse once, into one block and two.
    expect(opened).toEqual({ parses: 2 * REPLIES, blocks: 3 * REPLIES, rows: 0 });
  });

  it("renders only the session row that changed", async () => {
    const { core, sessions } = await bigWorkspace();
    const other = sessions[SESSIONS - 1]!;
    expect(
      await work(() => core.list({ type: "session", session: { ...other, title: "Renamed" } })),
    ).toEqual({ parses: 0, blocks: 0, rows: 1 });
  });

  it("parses and renders only the last block of a growing reply", async () => {
    const { core, sessions } = await bigWorkspace();
    const growing: ConversationUpdate = {
      type: "text",
      recorded_at: 0,
      key: `reply:${REPLIES - 1}`,
      text: " and more",
      replace: false,
    };
    expect(await work(() => core.update(sessions[0]!.id, growing))).toEqual({
      parses: 1,
      blocks: 1,
      rows: 0,
    });
  });

  it("renders nothing in the transcript while a draft is typed", async () => {
    await bigWorkspace();
    expect(
      await work(() => userEvent.type(screen.getByRole("combobox", { name: "Message" }), "hi")),
    ).toEqual({ parses: 0, blocks: 0, rows: 0 });
  });
});
