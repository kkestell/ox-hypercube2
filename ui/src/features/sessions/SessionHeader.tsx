import { Folder } from "lucide-react";
import { Subheader } from "../../components/Subheader";
import type { SessionInfo, WorktreeInfo } from "../../lib/protocol";
import { localMerge } from "./localMerge";
import { MergeButton } from "./MergeButton";
import type { SessionActions } from "./useSessionActions";
import { WorktreeBranch } from "./WorktreeBranch";

/**
 * A session's worktree branch and comparison, or the checkout directory it
 * runs in, with Merge into its comparison ref when it follows local merge.
 */
export function SessionHeader({
  session,
  worktree,
  deleteSession,
}: {
  session: SessionInfo;
  worktree: WorktreeInfo | undefined;
} & Pick<SessionActions, "deleteSession">) {
  const merge = localMerge(session, worktree);
  return (
    <Subheader
      action={
        worktree &&
        merge && (
          <MergeButton
            key={session.id}
            session={session}
            worktree={worktree}
            compare={merge.compare}
            comparison={merge.comparison}
            deleteSession={deleteSession}
          />
        )
      }
    >
      {worktree ? (
        <WorktreeBranch worktree={worktree} />
      ) : (
        <div title="The workspace's checkout">
          <Folder />
          <span className="subheader-code">{session.cwd}</span>
        </div>
      )}
    </Subheader>
  );
}
