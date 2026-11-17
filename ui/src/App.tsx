import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { sessionAgent } from "./components/HarnessLogo";
import { Workbench } from "./components/Workbench";
import { SessionComposer } from "./features/composer/Composer";
import { draftOptions, useLastUsed, type Values } from "./features/composer/controls";
import { DraftComposer } from "./features/composer/DraftComposer";
import { useUnsent } from "./features/composer/useUnsent";
import { Conversation } from "./features/conversation/Conversation";
import { useConversation } from "./features/conversation/useConversation";
import { RightPanel, type RightPanelTab } from "./features/files/RightPanel";
import { SessionContent } from "./features/files/SessionContent";
import { forgetDeletedTrees } from "./features/files/treeState";
import { useFileViewers } from "./features/files/useFileViewers";
import { openFile } from "./features/files/viewerState";
import { DraftHeader } from "./features/sessions/DraftHeader";
import { SessionHeader } from "./features/sessions/SessionHeader";
import { SessionList } from "./features/sessions/SessionList";
import { type AgentId, useAgents } from "./features/sessions/useAgents";
import { type Draft, draftTitle, useDrafts } from "./features/sessions/useDrafts";
import { useFocus } from "./features/sessions/useFocus";
import { useSessionActions } from "./features/sessions/useSessionActions";
import { useWorkspaces } from "./features/sessions/useWorkspaces";
import { type Item, isRequest, type SessionInfo } from "./lib/protocol";

const noItems: Item[] = [];

export function App() {
  const { workspaces, worktrees, sessions, connection } = useWorkspaces();
  const harnesses = useAgents();
  const agents = useMemo(() => harnesses.map(({ harness }) => harness), [harnesses]);
  const drafts = useDrafts(workspaces, connection > 0);
  // Each session's files are under its directory, and each draft's under its
  // workspace's directory.
  const views = useMemo(
    () => [
      ...sessions.map((session) => ({ id: session.id, cwd: session.cwd })),
      ...drafts.list.flatMap((draft) => {
        const workspace = workspaces.find((workspace) => workspace.id === draft.workspaceId);
        return workspace ? [{ id: draft.id, cwd: workspace.path }] : [];
      }),
    ],
    [sessions, drafts.list, workspaces],
  );
  useFileViewers(views, connection);
  const unsentIds = useMemo(
    () => new Set([...sessions.map(({ id }) => id), ...drafts.list.map(({ id }) => id)]),
    [sessions, drafts.list],
  );
  const unsent = useUnsent(unsentIds, connection > 0);
  const lastUsed = useLastUsed(sessions);
  // The viewed session or draft survives a webview reload.
  const [viewed, setViewed] = useState(() => sessionStorage.getItem("viewed"));
  useEffect(() => {
    if (viewed === null) sessionStorage.removeItem("viewed");
    else sessionStorage.setItem("viewed", viewed);
  }, [viewed]);
  // A draft with nothing typed goes once something else is viewed.
  const { dropEmpty } = drafts;
  useEffect(() => {
    dropEmpty(viewed, (id) => {
      const { text, attachments } = unsent.get(id);
      return text === "" && attachments.length === 0;
    });
  }, [dropEmpty, viewed, unsent]);
  // The draft titles change only when one of them does.
  const titles = JSON.stringify(
    drafts.list.map((draft) => [draft.id, draftTitle(unsent.get(draft.id).text)]),
  );
  const draftTitles = useMemo(() => new Map<string, string>(JSON.parse(titles)), [titles]);
  const { create: createDraft, remove: removeDraft } = drafts;
  const newDraft = useCallback(
    (workspaceId: string, agent: AgentId, checkout: Draft["checkout"]) =>
      setViewed(createDraft(workspaceId, agent, checkout)),
    [createDraft],
  );
  const deleteDraft = useCallback(
    (id: string) => {
      removeDraft(id);
      setViewed((viewed) => (viewed === id ? null : viewed));
    },
    [removeDraft],
  );
  // Each listing names every session, so a saved tree of any other ID
  // belongs to one that was deleted.
  useEffect(() => {
    if (connection > 0) forgetDeletedTrees(views);
  }, [views, connection]);
  const [rightTab, setRightTab] = useState<RightPanelTab>("files");
  const session = sessions.find((session) => session.id === viewed);
  const draft = drafts.list.find((draft) => draft.id === viewed);
  const draftWorkspace = draft && workspaces.find(({ id }) => id === draft.workspaceId);
  const root = session?.cwd ?? draftWorkspace?.path;
  // A draft offers what the config lists for its agent, at the draft's
  // values, else the last used.
  const draftHarness = draft && harnesses.find(({ harness }) => harness === draft.agent);
  const draftConfig = useMemo(() => {
    if (!draft || !draftHarness) return [];
    return draftOptions(draftHarness, draft.config, lastUsed.get(draft.agent, draft.workspaceId));
  }, [draft, draftHarness, lastUsed]);
  // A session offers what the config lists for its agent. A new session
  // shows the values it was sent with until its agent reports its own.
  const sessionAgentId = session && sessionAgent(session.command);
  const sessionHarness = harnesses.find(({ harness }) => harness === sessionAgentId);
  const startingConfig = useMemo(() => {
    if (
      !session ||
      !sessionHarness ||
      session.state !== "starting" ||
      session.agent_session_id !== null
    ) {
      return undefined;
    }
    return draftOptions(sessionHarness, lastUsed.get(sessionHarness.harness, session.workspace_id));
  }, [session, sessionHarness, lastUsed]);
  const { setConfig } = drafts;
  const { remember } = lastUsed;
  const draftSent = useCallback(
    (sent: Draft, session: SessionInfo, values: Values) => {
      remember(sent.agent, sent.workspaceId, values);
      removeDraft(sent.id);
      setViewed(session.id);
    },
    [remember, removeDraft],
  );
  useFocus(session?.id ?? null);
  const onOpenFile = useCallback(
    (path: string) => {
      if (viewed) openFile(viewed, path);
    },
    [viewed],
  );
  const worktree = session?.worktree_id == null ? undefined : worktrees.get(session.worktree_id);
  const { actions, dialogs } = useSessionActions();
  const items = useConversation(session?.id ?? null, connection);
  const request = useMemo(() => items.find(isRequest), [items]);

  const renderContent = () => {
    if (session) {
      return (toggles: ReactNode) => (
        <SessionContent
          key={session.id}
          viewId={session.id}
          title={session.title ?? "New session"}
          toggles={toggles}
        >
          <SessionHeader
            session={session}
            worktree={worktree}
            deleteSession={actions.deleteSession}
          />
          <Conversation sessionId={session.id} items={items} />
          <SessionComposer
            key={session.id}
            session={session}
            request={request}
            unsent={unsent}
            harness={sessionHarness}
            starting={startingConfig}
          />
        </SessionContent>
      );
    }
    if (draft && draftWorkspace) {
      return (toggles: ReactNode) => (
        <SessionContent
          key={draft.id}
          viewId={draft.id}
          title={draftTitles.get(draft.id)}
          toggles={toggles}
        >
          <DraftHeader draft={draft} workspace={draftWorkspace} />
          <Conversation sessionId={draft.id} items={noItems} />
          <DraftComposer
            key={draft.id}
            draft={draft}
            workspace={draftWorkspace}
            options={draftConfig}
            unsent={unsent}
            onSetOption={(configId, value) => setConfig(draft.id, configId, value)}
            onSent={(session, values) => draftSent(draft, session, values)}
          />
        </SessionContent>
      );
    }
    return undefined;
  };

  return (
    <>
      <Workbench
        left={
          <SessionList
            agents={agents}
            workspaces={workspaces}
            worktrees={worktrees}
            sessions={sessions}
            drafts={drafts.list}
            draftTitles={draftTitles}
            selected={viewed}
            onSelect={setViewed}
            onNewDraft={newDraft}
            onDeleteDraft={deleteDraft}
            actions={actions}
          />
        }
        right={
          viewed !== null && root !== undefined
            ? (toggles: ReactNode) => (
                <RightPanel
                  tab={rightTab}
                  onTab={setRightTab}
                  toggles={toggles}
                  viewId={viewed}
                  root={root}
                  connection={connection}
                  onOpenFile={onOpenFile}
                />
              )
            : undefined
        }
        renderContent={renderContent()}
      />
      {dialogs}
    </>
  );
}
