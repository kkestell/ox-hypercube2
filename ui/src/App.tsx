import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { sessionAgent } from "./components/HarnessLogo";
import { Workbench } from "./components/Workbench";
import { SessionComposer } from "./features/composer/Composer";
import { useControls, useLastUsed, type Values, withValues } from "./features/composer/controls";
import { DraftComposer } from "./features/composer/DraftComposer";
import { useUnsent } from "./features/composer/useUnsent";
import { Conversation } from "./features/conversation/Conversation";
import { useConversations } from "./features/conversation/useConversations";
import { RightPanel, type RightPanelTab } from "./features/files/RightPanel";
import { SessionContent } from "./features/files/SessionContent";
import { forgetDeletedTrees } from "./features/files/treeState";
import { useFileViewers } from "./features/files/useFileViewers";
import { openFile } from "./features/files/viewerState";
import { RunView } from "./features/runs/RunView";
import { DraftHeader } from "./features/sessions/DraftHeader";
import { SessionHeader } from "./features/sessions/SessionHeader";
import { runTitle, SessionList } from "./features/sessions/SessionList";
import { type AgentId, useAgents } from "./features/sessions/useAgents";
import { type Draft, draftTitle, useDrafts } from "./features/sessions/useDrafts";
import { useFocus } from "./features/sessions/useFocus";
import { useSessionActions } from "./features/sessions/useSessionActions";
import { useWorkspaces } from "./features/sessions/useWorkspaces";
import { type Focus, type Item, isRequest, type SessionInfo } from "./lib/protocol";

const noItems: Item[] = [];

export function App() {
  const { workspaces, worktrees, sessions, runs, parents, connection } = useWorkspaces();
  const agents = useAgents();
  const drafts = useDrafts(workspaces, connection > 0);
  // Each session's files are under its directory, each run's under its
  // worktree, and each draft's under its workspace's directory.
  const views = useMemo(
    () => [
      ...sessions.map((session) => ({ id: session.id, cwd: session.cwd })),
      ...runs.flatMap((run) => {
        const worktree = worktrees.get(run.worktree_id);
        return worktree ? [{ id: run.id, cwd: worktree.path }] : [];
      }),
      ...drafts.list.flatMap((draft) => {
        const workspace = workspaces.find((workspace) => workspace.id === draft.workspaceId);
        return workspace ? [{ id: draft.id, cwd: workspace.path }] : [];
      }),
    ],
    [sessions, runs, worktrees, drafts.list, workspaces],
  );
  useFileViewers(views, connection);
  const unsentIds = useMemo(
    () => new Set([...sessions.map(({ id }) => id), ...drafts.list.map(({ id }) => id)]),
    [sessions, drafts.list],
  );
  const unsent = useUnsent(unsentIds, connection > 0);
  const controls = useControls(agents, workspaces);
  const lastUsed = useLastUsed(sessions, parents);
  // The viewed session, run or draft survives a webview reload.
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
  // Each listing names every session and run, so a saved tree of any other
  // ID belongs to one that was deleted.
  useEffect(() => {
    if (connection > 0) forgetDeletedTrees(views);
  }, [views, connection]);
  const [rightTab, setRightTab] = useState<RightPanelTab>("files");
  const run = runs.find((run) => run.id === viewed);
  const session = run ? undefined : sessions.find((session) => session.id === viewed);
  const draft = drafts.list.find((draft) => draft.id === viewed);
  const draftWorkspace = draft && workspaces.find(({ id }) => id === draft.workspaceId);
  const root = run ? worktrees.get(run.worktree_id)?.path : (session?.cwd ?? draftWorkspace?.path);
  // Viewing a draft asks its agent for its controls again.
  const { probe } = controls;
  const draftAgent = draft?.agent;
  useEffect(() => {
    if (draftAgent !== undefined && draftWorkspace !== undefined) probe(draftAgent, draftWorkspace);
  }, [probe, draftAgent, draftWorkspace]);
  // A draft shows its agent's controls with the draft's values, else the
  // last used.
  const draftControls = useMemo(() => {
    const cached = draft && controls.get(draft.agent, draft.workspaceId);
    if (!draft || !cached) return undefined;
    const values = [draft.config, lastUsed.get(draft.agent, draft.workspaceId)];
    return { ...cached, config_options: withValues(cached.config_options, ...values) };
  }, [draft, controls, lastUsed]);
  // A new session shows its agent's controls at the values it was sent
  // with until the agent reports its own.
  const startingAgent = session && sessionAgent(session.command);
  const startingControls = useMemo(() => {
    if (
      !session ||
      startingAgent === undefined ||
      session.state !== "starting" ||
      session.agent_session_id !== null ||
      parents.has(session.id)
    ) {
      return undefined;
    }
    const cached = controls.get(startingAgent, session.workspace_id);
    const values = lastUsed.get(startingAgent, session.workspace_id);
    return cached && { ...cached, config_options: withValues(cached.config_options, values) };
  }, [session, startingAgent, parents, controls, lastUsed]);
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
  const runId = run?.id;
  const sessionId = session?.id;
  const focus = useMemo<Focus | null>(() => {
    if (runId !== undefined) return { type: "run", run_id: runId };
    if (sessionId !== undefined) return { type: "session", session_id: sessionId };
    return null;
  }, [runId, sessionId]);
  useFocus(focus);
  const onOpenFile = useCallback(
    (path: string) => {
      if (viewed) openFile(viewed, path);
    },
    [viewed],
  );
  const worktree = session?.worktree_id == null ? undefined : worktrees.get(session.worktree_id);
  const { actions, dialogs } = useSessionActions();
  const conversations = useConversations(session ? [session.id] : [], connection);
  const items = (session && conversations.get(session.id)) || noItems;
  const request = useMemo(() => items.find(isRequest), [items]);

  const renderContent = () => {
    if (run) {
      return (toggles: ReactNode) => (
        <SessionContent key={run.id} viewId={run.id} title={runTitle(run)} toggles={toggles}>
          <RunView
            run={run}
            sessions={sessions}
            worktree={worktrees.get(run.worktree_id)}
            connection={connection}
            cancelling={actions.busy.has(run.id)}
            onCancel={() => actions.cancelRun(run.id)}
          />
        </SessionContent>
      );
    }
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
            run={parents.get(session.id)}
            request={request}
            unsent={unsent}
            starting={startingControls}
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
            controls={draftControls}
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
            runs={runs}
            parents={parents}
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
