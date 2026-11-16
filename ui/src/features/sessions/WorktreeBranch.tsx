import { GitBranch, GitCompareArrows } from "lucide-react";
import type { WorktreeInfo } from "../../lib/protocol";
import { branchStatus } from "./localMerge";

/** A worktree's branch, and its comparison with its comparison ref, as a subheader's pairs. */
export function WorktreeBranch({ worktree }: { worktree: WorktreeInfo }) {
  const status =
    worktree.compare != null && worktree.comparison != null
      ? branchStatus(worktree.compare, worktree.comparison)
      : null;
  return (
    <>
      <div>
        <GitBranch />
        <span className="subheader-code">{worktree.branch}</span>
      </div>
      {status && (
        <div className={status.attention ? "attention" : undefined}>
          <GitCompareArrows />
          <span>{status.tooltip}</span>
        </div>
      )}
    </>
  );
}
