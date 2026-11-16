import { Square } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { Workbench } from "./components/Workbench";
import { Composer } from "./features/composer/Composer";
import { useDrafts } from "./features/composer/useDrafts";
import { Conversation } from "./features/conversation/Conversation";
import { useConversations } from "./features/conversation/useConversations";
import { FileTree } from "./features/files/FileTree";
import { SessionContent } from "./features/files/SessionContent";
import { forgetDeletedTrees } from "./features/files/treeState";
import { useFileViewers } from "./features/files/useFileViewers";
import { openFile } from "./features/files/viewerState";
import { RunView } from "./features/runs/RunView";
import { localMerge } from "./features/sessions/localMerge";
import { MergeButton } from "./features/sessions/MergeButton";
import { runTitle, SessionList } from "./features/sessions/SessionList";
import { useSessionActions } from "./features/sessions/useSessionActions";
import { useWorkspaces } from "./features/sessions/useWorkspaces";
import { type Item, isRequest } from "./lib/protocol";

const noItems: Item[] = [];

export function App() {
  const { workspaces, worktrees, sessions, runs, parents, connection } = useWorkspaces();
  // Each session's files are under its directory, and each run's under its
  // worktree.
  const views = useMemo(
    () => [
      ...sessions.map((session) => ({ id: session.id, cwd: session.cwd })),
      ...runs.flatMap((run) => {
        const worktree = worktrees.get(run.worktree_id);
        return worktree ? [{ id: run.id, cwd: worktree.path }] : [];
      }),
    ],
    [sessions, runs, worktrees],
  );
  useFileViewers(views, connection);
  const drafts = useDrafts(sessions, connection > 0);
  // The viewed session or run survives a webview reload.
  const [viewed, setViewed] = useState(() => sessionStorage.getItem("viewed"));
  useEffect(() => {
    if (viewed === null) sessionStorage.removeItem("viewed");
    else sessionStorage.setItem("viewed", viewed);
  }, [viewed]);
  // Each listing names every session and run, so a saved tree of any other
  // ID belongs to one that was deleted.
  useEffect(() => {
    if (connection > 0) forgetDeletedTrees(views);
  }, [views, connection]);
  const run = runs.find((run) => run.id === viewed);
  const session = run ? undefined : sessions.find((session) => session.id === viewed);
  const root = run ? worktrees.get(run.worktree_id)?.path : session?.cwd;
  const onOpenFile = useCallback(
    (path: string) => {
      if (viewed) openFile(viewed, path);
    },
    [viewed],
  );
  const worktree = session?.worktree_id == null ? undefined : worktrees.get(session.worktree_id);
  const merge = session && localMerge(session, worktree);
  const { actions, dialogs } = useSessionActions();
  const conversations = useConversations(session ? [session.id] : [], connection);
  const items = (session && conversations.get(session.id)) || noItems;
  const request = useMemo(() => items.find(isRequest), [items]);

  const renderContent = () => {
    if (run) {
      return (header: ReactNode) => (
        <SessionContent key={run.id} viewId={run.id} header={header}>
          <RunView
            run={run}
            sessions={sessions}
            worktree={worktrees.get(run.worktree_id)}
            connection={connection}
          />
        </SessionContent>
      );
    }
    if (session) {
      return (header: ReactNode) => (
        <SessionContent key={session.id} viewId={session.id} header={header}>
          <Conversation sessionId={session.id} items={items} />
          <Composer
            key={session.id}
            session={session}
            run={parents.get(session.id)}
            request={request}
            drafts={drafts}
          />
        </SessionContent>
      );
    }
    return undefined;
  };

  const headerActions = () => {
    if (run?.state === "running") {
      return (
        <button
          type="button"
          className="icon-button"
          title="Cancel Run"
          aria-label="Cancel Run"
          disabled={actions.busy.has(run.id)}
          onClick={() => actions.cancelRun(run.id)}
        >
          <Square />
        </button>
      );
    }
    return (
      session &&
      worktree &&
      merge && (
        <MergeButton
          key={session.id}
          session={session}
          worktree={worktree}
          compare={merge.compare}
          comparison={merge.comparison}
          deleteSession={actions.deleteSession}
        />
      )
    );
  };

  return (
    <>
      <Workbench
        left={
          <SessionList
            workspaces={workspaces}
            worktrees={worktrees}
            sessions={sessions}
            runs={runs}
            parents={parents}
            selected={viewed}
            onSelect={setViewed}
            actions={actions}
          />
        }
        right={
          viewed !== null &&
          root !== undefined && (
            <FileTree
              key={viewed}
              viewId={viewed}
              root={root}
              connection={connection}
              onOpenFile={onOpenFile}
            />
          )
        }
        renderContent={renderContent()}
        actions={headerActions()}
        title={run ? runTitle(run) : session ? (session.title ?? "New session") : undefined}
      />
      {dialogs}
    </>
  );
}
