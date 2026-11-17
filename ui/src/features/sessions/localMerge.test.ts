import { describe, expect, it } from "vitest";
import type { Comparison } from "../../lib/protocol";
import { branchStatus, mergeBlocker } from "./localMerge";

const compared = (ahead: number, behind: number): Comparison => ({
  type: "compared",
  tip: "b",
  compare_tip: "a",
  ahead,
  behind,
});
const failed: Comparison = { type: "failed", error: "bad ref" };

describe("a local merge", () => {
  it.each([
    [compared(0, 0), "No commits", null],
    [compared(0, 2), "No commits", null],
    [compared(1, 0), null, { attention: false, tooltip: "1 commit ahead of main" }],
    [
      compared(2, 0),
      "2 commits: ask the agent to squash",
      { attention: true, tooltip: "2 commits ahead of main" },
    ],
    [
      compared(1, 1),
      "Behind main: ask the agent to rebase",
      { attention: true, tooltip: "1 commit ahead, 1 behind main" },
    ],
    [failed, "bad ref", { attention: true, tooltip: "bad ref" }],
  ])("of %o is blocked by %o with icon %o", (comparison, blocker, status) => {
    expect(mergeBlocker("main", comparison)).toBe(blocker);
    expect(branchStatus("main", comparison)).toEqual(status);
  });
});
