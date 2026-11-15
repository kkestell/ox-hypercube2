import { useCallback, useEffect, useMemo, useState } from "react";
import { Workbench } from "./components/Workbench";
import { Composer } from "./features/composer/Composer";
import { useDrafts } from "./features/composer/useDrafts";
import { Conversation } from "./features/conversation/Conversation";
import { useConversation } from "./features/conversation/useConversation";
import { FileTree } from "./features/files/FileTree";
import { SessionContent } from "./features/files/SessionContent";
import { forgetDeletedTrees } from "./features/files/treeState";
import { useFileViewers } from "./features/files/useFileViewers";
import { openFile } from "./features/files/viewerState";
import { localMerge } from "./features/sessions/localMerge";
import { MergeButton } from "./features/sessions/MergeButton";
import { SessionList } from "./features/sessions/SessionList";
import { useSessionActions } from "./features/sessions/useSessionActions";
import { useWorkspaces } from "./features/sessions/useWorkspaces";
import type { Item } from "./lib/protocol";

export function App() {
  const { workspaces, sessions, connection } = useWorkspaces();
  useFileViewers(sessions, connection);
  const drafts = useDrafts(sessions, connection > 0);
  // The viewed session survives a webview reload.
  const [viewed, setViewed] = useState(() => sessionStorage.getItem("viewed"));
  useEffect(() => {
    if (viewed === null) sessionStorage.removeItem("viewed");
    else sessionStorage.setItem("viewed", viewed);
  }, [viewed]);
  // Each listing names every session, so a saved tree of any other session
  // belongs to one that was deleted.
  useEffect(() => {
    if (connection > 0) forgetDeletedTrees(sessions);
  }, [sessions, connection]);
  const session = sessions.find((session) => session.id === viewed);
  const onOpenFile = useCallback(
    (path: string) => {
      if (viewed) openFile(viewed, path);
    },
    [viewed],
  );
  const merge = session && localMerge(session);
  const { actions, dialogs } = useSessionActions();
  const items = useConversation(session?.id ?? null, connection);
  const request = useMemo(
    () =>
      items.find(
        (item): item is Extract<Item, { type: "permission" | "elicitation" }> =>
          item.type === "permission" || item.type === "elicitation",
      ),
    [items],
  );

  return (
    <>
      <Workbench
        left={
          <SessionList
            workspaces={workspaces}
            sessions={sessions}
            selected={viewed}
            onSelect={setViewed}
            actions={actions}
          />
        }
        right={
          session && (
            <FileTree
              key={session.id}
              sessionId={session.id}
              root={session.cwd}
              connection={connection}
              onOpenFile={onOpenFile}
            />
          )
        }
        renderContent={
          session
            ? (header) => (
                <SessionContent key={session.id} sessionId={session.id} header={header}>
                  <Conversation sessionId={session.id} items={items} />
                  <Composer key={session.id} session={session} request={request} drafts={drafts} />
                </SessionContent>
              )
            : undefined
        }
        actions={
          session &&
          merge && (
            <MergeButton
              key={session.id}
              session={session}
              compare={merge.compare}
              comparison={merge.comparison}
              deleteSession={actions.deleteSession}
            />
          )
        }
      >
        {null}
      </Workbench>
      {dialogs}
    </>
  );
}
