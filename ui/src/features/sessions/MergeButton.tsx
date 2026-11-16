import { invoke } from "@tauri-apps/api/core";
import { GitMerge } from "lucide-react";
import { memo, useState } from "react";
import { Dialog } from "../../components/Dialog";
import type { Comparison, SessionInfo, WorktreeInfo } from "../../lib/protocol";
import { showError } from "../../lib/showError";
import { mergeBlocker } from "./localMerge";

/**
 * Merge: fast-forwards the ref's branch to the branch of the
 * session's worktree, then offers to delete the session.
 */
export const MergeButton = memo(function MergeButton({
  session,
  worktree,
  compare,
  comparison,
  deleteSession,
}: {
  session: SessionInfo;
  worktree: WorktreeInfo;
  compare: string;
  comparison: Comparison;
  deleteSession: (sessionId: string, discard: boolean) => Promise<void>;
}) {
  const [moving, setMoving] = useState(false);
  const [merged, setMerged] = useState(false);
  const blocker = mergeBlocker(compare, comparison);

  const merge = async () => {
    if (comparison.type !== "compared") return;
    setMoving(true);
    try {
      await invoke("move_branch", {
        workspaceId: session.workspace_id,
        branch: compare,
        to: comparison.tip,
        expected: comparison.compare_tip,
        sourceBranch: worktree.branch,
      });
      setMerged(true);
    } catch (e) {
      await showError(e);
    } finally {
      setMoving(false);
    }
  };

  return (
    <>
      <button
        type="button"
        className="text-button"
        title={blocker ?? `Merge into ${compare}`}
        disabled={blocker !== null || moving}
        onClick={merge}
      >
        <GitMerge />
        Merge
      </button>
      {merged && (
        <Dialog
          title={`Merged into ${compare}`}
          onClose={() => setMerged(false)}
          actions={
            <>
              <button type="button" className="text-button" onClick={() => setMerged(false)}>
                Keep Session
              </button>
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  setMerged(false);
                  deleteSession(session.id, false);
                }}
              >
                Delete Session
              </button>
            </>
          }
        >
          <p className="message">
            Delete the session? Ox removes its worktree and its branch{" "}
            <code>{worktree.branch}</code>.
          </p>
        </Dialog>
      )}
    </>
  );
});
