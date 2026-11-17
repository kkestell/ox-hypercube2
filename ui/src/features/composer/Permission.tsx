import { invoke } from "@tauri-apps/api/core";
import { Folder, GitBranch } from "lucide-react";
import { useState } from "react";
import { CodeSpans } from "../../components/CodeSpans";
import { HarnessLogo } from "../../components/HarnessLogo";
import type { Item, SessionRequest } from "../../lib/protocol";
import { showError } from "../../lib/showError";
import { Markdown } from "../conversation/Markdown";

/** A permission request awaiting its answer, with the options the agent offers. */
export function Permission({
  item,
  sessionId,
}: {
  item: Extract<Item, { type: "permission" }>;
  sessionId: string;
}) {
  const [pending, setPending] = useState(false);

  const permit = async (optionId: string) => {
    setPending(true);
    try {
      await invoke("permit", { sessionId, requestId: item.request_id, optionId });
    } catch (e) {
      setPending(false);
      await showError(e);
    }
  };

  return (
    <div className="request-panel">
      <div className="scroll">
        <p className="title">
          <CodeSpans text={item.title} />
        </p>
        {item.session_request ? (
          <SessionRequestView request={item.session_request} />
        ) : (
          item.description && <p className="description">{item.description}</p>
        )}
      </div>
      <div className="actions">
        {item.options.map((option) => (
          <button
            key={option.optionId}
            type="button"
            className="text-button"
            data-kind={option.kind}
            disabled={pending}
            onClick={() => permit(option.optionId)}
          >
            {option.name}
          </button>
        ))}
      </div>
    </div>
  );
}

/**
 * A requested session as a new session shows it: its harness, model and
 * effort as the composer shows them, then its mode, where it runs as a
 * subheader shows it, and its first prompt.
 */
function SessionRequestView({ request }: { request: SessionRequest }) {
  const { checkout } = request;
  return (
    <>
      <dl className="subheader-meta">
        <div>
          <HarnessLogo agent={request.harness} />
          <span>{request.model}</span>
          {request.provider && <span className="provider-name">{request.provider}</span>}
          {request.effort && <span className="effort-name">{request.effort}</span>}
          {request.mode && <span className="mode-name">{request.mode}</span>}
        </div>
        {checkout.type === "worktree" ? (
          <div>
            <GitBranch />
            <span>{checkout.branch ? "New worktree on" : "New worktree from"}</span>
            <span className="subheader-code">{checkout.branch ?? checkout.base}</span>
          </div>
        ) : (
          <div title="The workspace's checkout">
            <Folder />
            <span className="subheader-code">{checkout.path}</span>
          </div>
        )}
      </dl>
      <Markdown text={request.prompt} />
    </>
  );
}
