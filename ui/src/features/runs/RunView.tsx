import {
  isRequest,
  type Request,
  type RunInfo,
  type SessionInfo,
  type WorktreeInfo,
} from "../../lib/protocol";
import { OpenRequest } from "../composer/OpenRequest";
import { useConversations } from "../conversation/useConversations";
import { RunHeader } from "./RunHeader";
import { RunStatus, runNotice } from "./RunStatus";

/**
 * A run's header and steps, and below them the oldest open permission
 * request or form among the children, in step order.
 */
export function RunView({
  run,
  sessions,
  worktree,
  connection,
  cancelling,
  onCancel,
}: {
  run: RunInfo;
  sessions: SessionInfo[];
  /** The worktree the run owns. */
  worktree: WorktreeInfo | undefined;
  connection: number;
  /** Whether cancelling the run is in flight. */
  cancelling: boolean;
  onCancel: () => void;
}) {
  const notice = runNotice(run);
  const childIds = run.steps.flatMap((step) => (step.session_id === null ? [] : [step.session_id]));
  const conversations = useConversations(childIds, connection);
  let request: { item: Request; sessionId: string } | undefined;
  for (const sessionId of childIds) {
    const item = conversations.get(sessionId)?.find(isRequest);
    if (item) {
      request = { item, sessionId };
      break;
    }
  }

  return (
    <>
      <RunHeader run={run} worktree={worktree} cancelling={cancelling} onCancel={onCancel} />
      <div className="conversation">
        <RunStatus run={run} sessions={sessions} conversations={conversations} notice={notice} />
        {notice !== null && notice.index === -1 && (
          <div className="run-notice">
            <p>{notice.text}</p>
          </div>
        )}
      </div>
      {request && (
        <div className="composer">
          <div className="stack">
            <OpenRequest
              key={`${request.sessionId}:${request.item.request_id}`}
              item={request.item}
              sessionId={request.sessionId}
            />
          </div>
        </div>
      )}
    </>
  );
}
