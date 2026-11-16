import {
  isRequest,
  type Request,
  type RunInfo,
  type SessionInfo,
  type WorktreeInfo,
} from "../../lib/protocol";
import { OpenRequest } from "../composer/OpenRequest";
import { Transcript } from "../conversation/Conversation";
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
}: {
  run: RunInfo;
  sessions: SessionInfo[];
  /** The worktree the run owns. */
  worktree: WorktreeInfo | undefined;
  connection: number;
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
      <RunHeader run={run} worktree={worktree} />
      <Transcript viewId={run.id}>
        <RunStatus run={run} sessions={sessions} conversations={conversations} notice={notice} />
        {notice !== null && notice.index === -1 && (
          <div className="run-notice">
            <p>{notice.text}</p>
          </div>
        )}
      </Transcript>
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
