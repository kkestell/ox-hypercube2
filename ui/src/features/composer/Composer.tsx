import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowUp, Paperclip, Square, X } from "lucide-react";
import { memo, useEffect, useRef } from "react";
import { HarnessLogo, sessionAgent } from "../../components/HarnessLogo";
import { SelectPicker } from "../../components/SelectPicker";
import type { ConfigOption, QueuedPrompt, Request, RunInfo, SessionInfo } from "../../lib/protocol";
import { showError } from "../../lib/showError";
import { CommandInput } from "./CommandInput";
import { ModelPicker, type SelectConfig } from "./ModelPicker";
import { OpenRequest } from "./OpenRequest";
import { QueuedPrompts, queuedMessage } from "./QueuedPrompts";
import type { Drafts } from "./useDrafts";

/**
 * A session's composer. On an idle session, Enter and Cmd+Enter send the
 * message. While the agent works, Enter queues it and Cmd+Enter steers the
 * running turn with it; the form's button sends a message that has content
 * and stops the turn otherwise, and Escape stops the turn. While a queued
 * prompt is edited, Enter saves it in its place and brings back the draft it
 * replaced. While the run a child session belongs to is running, the entry
 * and attachments are disabled and Stop and Escape cancel the run.
 */
export const Composer = memo(function Composer({
  session,
  run: parent,
  request,
  drafts,
}: {
  session: SessionInfo;
  /** The run the session is a child of. */
  run: RunInfo | undefined;
  /** The session's oldest unanswered permission request or form. */
  request: Request | undefined;
  drafts: Drafts;
}) {
  const draft = drafts.get(session.id);
  const { text, attachments } = draft;
  // An edit of a prompt that has left the queue is sent as a new message.
  const editing = session.queue.some(({ id }) => id === draft.editing?.promptId)
    ? draft.editing
    : undefined;

  const running =
    session.state === "running" ||
    session.state === "awaiting_permission" ||
    session.state === "awaiting_input";
  const runRunning = parent?.state === "running";
  const hasContent = text.trim() !== "" || attachments.length > 0;
  const canSend = (session.state === "idle" || running) && !runRunning && hasContent;
  const disabled = session.state === "exited" || runRunning;
  const showStop = (running || runRunning) && !hasContent;

  /** Runs `action`, shows its error if it fails, and resolves to whether it succeeded. */
  const run = async (action: () => Promise<unknown>) => {
    try {
      await action();
      return true;
    } catch (e) {
      await showError(e);
      return false;
    }
  };

  /** Saves an edit, sends the message now when `steer` is set or the agent is idle, or queues it. */
  const send = (steer: boolean) =>
    run(async () => {
      const message = { sessionId: session.id, text: text.trim(), attachments };
      if (editing) await invoke("queue_edit", { ...message, promptId: editing.promptId });
      else if (steer || !running) await invoke("prompt", message);
      else await invoke("queue_add", message);
      drafts.update(session.id, (current) =>
        current === draft ? (draft.editing?.held ?? undefined) : current,
      );
    });

  const stop = () =>
    run(() =>
      runRunning
        ? invoke("cancel_run", { runId: parent.id })
        : invoke("cancel", { sessionId: session.id }),
    );

  // Escape stops the work, unless something else handled it first or it
  // closes an open popover, such as the model menu, or a dialog.
  const stopRef = useRef(stop);
  stopRef.current = stop;
  useEffect(() => {
    if (!running && !runRunning) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        event.key === "Escape" &&
        !event.defaultPrevented &&
        !document.querySelector(":popover-open, dialog[open]")
      ) {
        stopRef.current();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [running, runRunning]);

  const edit = (queued: QueuedPrompt) =>
    drafts.update(session.id, (current) => ({
      ...queuedMessage(queued.prompt),
      editing: {
        promptId: queued.id,
        held: current.editing?.held ?? { text: current.text, attachments: current.attachments },
      },
    }));

  const setOption = (option: ConfigOption, value: string | boolean) =>
    run(() =>
      invoke("set_config_option", { sessionId: session.id, configId: option.configId, value }),
    );

  // The model picker shows the model and effort options; the rest show as
  // pickers of their own.
  const selects = session.config_options.filter(
    (option): option is SelectConfig => option.type === "select",
  );
  const model = selects.find((option) => option.category === "model");
  const effort = model && selects.find((option) => option.category === "thought_level");
  const others = session.config_options.filter((option) => option !== model && option !== effort);

  const attach = async () => {
    const paths = await open({ multiple: true, title: "Attach Files" });
    if (paths !== null) {
      drafts.update(session.id, (current) => ({
        ...current,
        attachments: [...new Set([...current.attachments, ...paths])],
      }));
    }
  };

  return (
    <div className="composer">
      <div className="stack">
        {request && <OpenRequest key={request.request_id} item={request} sessionId={session.id} />}
        <QueuedPrompts sessionId={session.id} queue={session.queue} onEdit={edit} />
        <div className="entry">
          <form
            className="box"
            onSubmit={(event) => {
              event.preventDefault();
              if (showStop) stop();
              else if (canSend) send(false);
            }}
          >
            {attachments.length > 0 && (
              <ul className="attachments">
                {attachments.map((path) => (
                  <li key={path} title={path}>
                    {path.split("/").pop()}
                    <button
                      type="button"
                      className="icon-button"
                      aria-label="Remove"
                      onClick={() =>
                        drafts.update(session.id, (current) => ({
                          ...current,
                          attachments: current.attachments.filter((p) => p !== path),
                        }))
                      }
                    >
                      <X />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            <div className="input">
              <button
                type="button"
                className="icon-button"
                title="Attach Files"
                aria-label="Attach Files"
                disabled={disabled}
                onClick={attach}
              >
                <Paperclip />
              </button>
              <CommandInput
                value={text}
                commands={session.available_commands}
                disabled={disabled}
                canSend={canSend}
                onChange={(text) => drafts.update(session.id, (current) => ({ ...current, text }))}
                onSubmit={send}
              />
              <button
                type="submit"
                className="icon-button"
                title={showStop ? "Stop" : "Send"}
                aria-label={showStop ? "Stop" : "Send"}
                disabled={!showStop && !canSend}
              >
                {showStop ? <Square /> : <ArrowUp />}
              </button>
            </div>
          </form>
        </div>
      </div>
      <div className="settings">
        {model && (
          <div className="model-setting">
            <HarnessLogo agent={sessionAgent(session.command)} />
            <ModelPicker model={model} effort={effort} onChange={setOption} />
          </div>
        )}
        {others.map((option) => (
          <ConfigPicker
            key={option.configId}
            option={option}
            onChange={(value) => setOption(option, value)}
          />
        ))}
      </div>
    </div>
  );
});

function ConfigPicker({
  option,
  onChange,
}: {
  option: ConfigOption;
  onChange: (value: string | boolean) => void;
}) {
  if (option.type === "boolean") {
    return (
      <label className="choice">
        <input
          type="checkbox"
          checked={option.currentValue}
          onChange={(event) => onChange(event.target.checked)}
        />
        {option.name}
      </label>
    );
  }
  if (option.type !== "select") return null;
  return (
    <SelectPicker
      label={option.name}
      value={option.currentValue}
      options={option.options}
      onChange={onChange}
    />
  );
}
