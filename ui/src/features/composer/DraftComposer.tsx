import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type {
  Branches,
  Checkout,
  ConfigSetting,
  Controls,
  SessionInfo,
  WorkspaceInfo,
} from "../../lib/protocol";
import { directoryName } from "../sessions/SessionList";
import type { Draft } from "../sessions/useDrafts";
import { Composer } from "./Composer";
import type { Values } from "./controls";
import type { Unsents } from "./useUnsent";

/**
 * A draft's composer, with the controls its agent offers in its workspace.
 * Send creates the session with the message as its first prompt and the
 * shown option values, then `onSent` gets the session and those values. A
 * worktree draft is compared with the branch the workspace's checkout is on
 * at Send.
 */
export function DraftComposer({
  draft,
  workspace,
  controls,
  unsent,
  onSetOption,
  onSent,
}: {
  draft: Draft;
  workspace: WorkspaceInfo;
  /** The controls to show, with the draft's values. */
  controls: Controls | undefined;
  unsent: Unsents;
  onSetOption: (configId: string, value: string | boolean) => void;
  onSent: (session: SessionInfo, values: Values) => void;
}) {
  const [sending, setSending] = useState(false);
  const options = controls?.config_options ?? [];

  const send = async (text: string, attachments: string[]) => {
    setSending(true);
    try {
      let checkout: Checkout = { type: "workspace" };
      if (draft.checkout === "worktree") {
        const { head } = await invoke<Branches>("workspace_branches", {
          workspaceId: workspace.id,
        });
        if (head === null) throw `${directoryName(workspace.path)} is not on a branch`;
        checkout = { type: "worktree", compare: head };
      }
      const config: ConfigSetting[] = options.map((option) => ({
        config_id: option.configId,
        value: option.currentValue,
      }));
      const session = await invoke<SessionInfo>("new_session", {
        agent: draft.agent,
        workspaceId: workspace.id,
        checkout,
        text,
        attachments,
        config,
      });
      onSent(
        session,
        Object.fromEntries(config.map((setting) => [setting.config_id, setting.value])),
      );
    } finally {
      setSending(false);
    }
  };

  return (
    <Composer
      unsentId={draft.id}
      unsent={unsent}
      agent={draft.agent}
      configOptions={options}
      commands={controls?.available_commands ?? []}
      running={false}
      disabled={false}
      idle={!sending}
      onSend={send}
      onSetOption={async (option, value) => onSetOption(option.configId, value)}
    />
  );
}
