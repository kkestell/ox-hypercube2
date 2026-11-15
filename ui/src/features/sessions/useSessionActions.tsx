import { invoke } from "@tauri-apps/api/core";
import { useCallback, useMemo, useState } from "react";
import { Dialog } from "../../components/Dialog";
import type { Archival, Deletion } from "../../lib/protocol";
import { showError } from "../../lib/showError";

/**
 * Deleting, archiving and unarchiving sessions, and cancelling and deleting
 * runs, with the dialogs that ask before losing unsaved work and report a
 * kept branch. `actions.busy` holds the sessions and runs whose actions are
 * in flight, and `actions` changes only with it; the caller renders
 * `dialogs`.
 */
export function useSessionActions() {
  // The sessions and runs whose actions are in flight.
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  // An archive or delete of session or run `id` that would lose the unsaved
  // work `message` lists.
  const [unsaved, setUnsaved] = useState<{
    id: string;
    action: "archive" | "delete" | "delete_run";
    message: string;
  } | null>(null);
  const [keptBranch, setKeptBranch] = useState<string | null>(null);

  const deleteSession = useCallback(
    (sessionId: string, discard: boolean) =>
      withBusy(setBusy, sessionId, async () => {
        const deletion = await invoke<Deletion>("delete_session", { sessionId, discard });
        if (deletion.type === "unsaved_work") {
          setUnsaved({ id: sessionId, action: "delete", message: deletion.message });
        } else if (deletion.kept_branch !== null) {
          setKeptBranch(deletion.kept_branch);
        }
      }),
    [],
  );

  const archiveSession = useCallback(
    (sessionId: string, discard: boolean) =>
      withBusy(setBusy, sessionId, async () => {
        const archival = await invoke<Archival>("archive_session", { sessionId, discard });
        if (archival.type === "unsaved_work") {
          setUnsaved({ id: sessionId, action: "archive", message: archival.message });
        }
      }),
    [],
  );

  const unarchiveSession = useCallback(
    (sessionId: string) =>
      withBusy(setBusy, sessionId, () => invoke("unarchive_session", { sessionId })),
    [],
  );

  const cancelRun = useCallback(
    (runId: string) => withBusy(setBusy, runId, () => invoke("cancel_run", { runId })),
    [],
  );

  const deleteRun = useCallback(
    (runId: string, discard: boolean) =>
      withBusy(setBusy, runId, async () => {
        const deletion = await invoke<Deletion>("delete_run", { runId, discard });
        if (deletion.type === "unsaved_work") {
          setUnsaved({ id: runId, action: "delete_run", message: deletion.message });
        } else if (deletion.kept_branch !== null) {
          setKeptBranch(deletion.kept_branch);
        }
      }),
    [],
  );

  const actions = useMemo(
    () => ({ busy, deleteSession, archiveSession, unarchiveSession, cancelRun, deleteRun }),
    [busy, deleteSession, archiveSession, unarchiveSession, cancelRun, deleteRun],
  );

  const dialogs = (
    <>
      {unsaved !== null && (
        <Dialog
          title="Discard unsaved work?"
          onClose={() => setUnsaved(null)}
          actions={
            <>
              <button type="button" className="text-button" onClick={() => setUnsaved(null)}>
                Cancel
              </button>
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  setUnsaved(null);
                  if (unsaved.action === "archive") archiveSession(unsaved.id, true);
                  else if (unsaved.action === "delete") deleteSession(unsaved.id, true);
                  else deleteRun(unsaved.id, true);
                }}
              >
                {unsaved.action === "archive" ? "Discard and Archive" : "Discard and Delete"}
              </button>
            </>
          }
        >
          <p className="message">{unsaved.message}</p>
        </Dialog>
      )}
      {keptBranch !== null && (
        <Dialog
          title="Branch kept"
          onClose={() => setKeptBranch(null)}
          actions={
            <button type="button" className="text-button" onClick={() => setKeptBranch(null)}>
              OK
            </button>
          }
        >
          <p className="message">
            Ox kept the branch <code>{keptBranch}</code>.
          </p>
        </Dialog>
      )}
    </>
  );

  return { actions, dialogs };
}

export type SessionActions = ReturnType<typeof useSessionActions>["actions"];

/** Runs `action` with the actions of session or run `id` disabled. */
async function withBusy(
  setBusy: (change: (busy: ReadonlySet<string>) => ReadonlySet<string>) => void,
  id: string,
  action: () => Promise<unknown>,
) {
  setBusy((busy) => new Set(busy).add(id));
  try {
    await action();
  } catch (e) {
    await showError(e);
  } finally {
    setBusy((busy) => {
      const rest = new Set(busy);
      rest.delete(id);
      return rest;
    });
  }
}
