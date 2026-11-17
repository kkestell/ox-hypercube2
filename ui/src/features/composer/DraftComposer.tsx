import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import type {
  Branches,
  Checkout,
  Command,
  ConfigOption,
  SessionInfo,
  WorkspaceInfo,
} from "../../lib/protocol";
import { directoryName } from "../sessions/SessionList";
import type { Draft } from "../sessions/useDrafts";
import { Composer } from "./Composer";
import { settingsOf, type Values } from "./controls";
import type { Unsents } from "./useUnsent";

const noCommands: Command[] = [];

/**
 * A draft's composer, with the options Ox's config lists for its agent and
 * no slash commands, which an agent reports once it has started. Send
 * creates the session with the message as its first prompt and the shown
 * option values, then `onSent` gets the session and those values. A
 * worktree draft is compared with the branch the workspace's checkout is on
 * at Send.
 */
export function DraftComposer({
  draft,
  workspace,
  options,
  unsent,
  onSetOption,
  onSent,
}: {
  draft: Draft;
  workspace: WorkspaceInfo;
  /** The options to show, with the draft's values. */
  options: ConfigOption[];
  unsent: Unsents;
  onSetOption: (configId: string, value: string | boolean) => void;
  onSent: (session: SessionInfo, values: Values) => void;
}) {
  const [sending, setSending] = useState(false);

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
      const config = settingsOf(options);
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
      commands={noCommands}
      running={false}
      disabled={false}
      idle={!sending}
      onSend={send}
      onSetOption={async (option, value) => onSetOption(option.configId, value)}
    />
  );
}
