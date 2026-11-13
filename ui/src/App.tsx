import { useEffect, useState } from "react";
import { Workbench } from "./components/Workbench";
import { Composer } from "./features/composer/Composer";
import { Conversation } from "./features/conversation/Conversation";
import { useConversation } from "./features/conversation/useConversation";
import { localMerge } from "./features/sessions/localMerge";
import { MergeButton } from "./features/sessions/MergeButton";
import { SessionList } from "./features/sessions/SessionList";
import { useSessionActions } from "./features/sessions/useSessionActions";
import { useWorkspaces } from "./features/sessions/useWorkspaces";
import type { Item } from "./lib/protocol";

export function App() {
  const { workspaces, sessions, connection } = useWorkspaces();
  // The viewed session survives a webview reload.
  const [viewed, setViewed] = useState(() => sessionStorage.getItem("viewed"));
  useEffect(() => {
    if (viewed === null) sessionStorage.removeItem("viewed");
    else sessionStorage.setItem("viewed", viewed);
  }, [viewed]);
  const session = sessions.find((session) => session.id === viewed);
  const merge = session && localMerge(session);
  const actions = useSessionActions();
  const items = useConversation(session?.id ?? null, connection);
  const request = items.find(
    (item): item is Extract<Item, { type: "permission" | "elicitation" }> =>
      (item.type === "permission" || item.type === "elicitation") && !item.answered,
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
        {session && (
          <>
            <Conversation sessionId={session.id} items={items} />
            <Composer key={session.id} session={session} request={request} />
          </>
        )}
      </Workbench>
      {actions.dialogs}
    </>
  );
}
