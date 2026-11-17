import { Folder, GitBranch } from "lucide-react";
import { Subheader } from "../../components/Subheader";
import type { WorkspaceInfo } from "../../lib/protocol";
import type { Draft } from "./useDrafts";

/** Where a draft's session will run: the workspace's checkout, or a new worktree. */
export function DraftHeader({ draft, workspace }: { draft: Draft; workspace: WorkspaceInfo }) {
  return (
    <Subheader>
      {draft.checkout === "worktree" ? (
        <div>
          <GitBranch />
          <span>New worktree</span>
        </div>
      ) : (
        <div title="The workspace's checkout">
          <Folder />
          <span className="subheader-code">{workspace.path}</span>
        </div>
      )}
    </Subheader>
  );
}
