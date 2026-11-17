import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowUp, Paperclip, Square, X } from "lucide-react";
import { memo, useEffect, useRef } from "react";
import { HarnessLogo, sessionAgent } from "../../components/HarnessLogo";
import { SelectPicker } from "../../components/SelectPicker";
import type {
  Command,
  ConfigOption,
  Controls,
  QueuedPrompt,
  Request,
  RunInfo,
  SessionInfo,
} from "../../lib/protocol";
import { showError } from "../../lib/showError";
import type { AgentId } from "../sessions/useAgents";
import { CommandInput } from "./CommandInput";
import { ModelPicker, type SelectConfig } from "./ModelPicker";
import { OpenRequest } from "./OpenRequest";
import { QueuedPrompts, queuedMessage } from "./QueuedPrompts";
import type { Unsents } from "./useUnsent";

/** Runs `action`, shows its error if it fails, and resolves to whether it succeeded. */
async function attempt(action: () => Promise<unknown>) {
  try {
    await action();
    return true;
  } catch (e) {
    await showError(e);
    return false;
  }
}

/**
 * A composer: the entry with its attachments and slash commands, the queued
 * prompts of `queue`, and the agent's config options. `unsentId` names the
 * unsent text it edits. The entry and attachments are disabled while
 * `disabled`. Send is enabled while there is something to send and the
 * composer is `idle` or `running` and not `disabled`; `onSend` gets `steer`
 * set for Cmd+Enter. While `running`, the form's button sends a message that
 * has content and stops otherwise, and Escape stops. While a queued prompt is
 * edited, Enter saves it in its place with `onSaveEdit` and brings back the
 * message it replaced.
 */
export function Composer({
  unsentId,
  unsent,
  agent,
  configOptions,
  commands,
  request,
  queue,
  running,
  disabled,
  idle,
  optionsDisabled = false,
  onSend,
  onSaveEdit,
  onStop,
  onSetOption,
}: {
  unsentId: string;
  unsent: Unsents;
  agent: AgentId | undefined;
  configOptions: ConfigOption[];
  commands: Command[];
  /** The session's oldest unanswered permission request or form. */
  request?: Request;
  queue?: QueuedPrompt[];
  running: boolean;
  disabled: boolean;
  idle: boolean;
  /** Whether the config options show without being settable. */
  optionsDisabled?: boolean;
  onSend: (text: string, attachments: string[], steer: boolean) => Promise<unknown>;
  onSaveEdit?: (promptId: string, text: string, attachments: string[]) => Promise<unknown>;
  onStop?: () => Promise<unknown>;
  onSetOption: (option: ConfigOption, value: string | boolean) => Promise<unknown>;
}) {
  const current = unsent.get(unsentId);
  const { text, attachments } = current;
  // An edit of a prompt that has left the queue is sent as a new message.
  const editing = queue?.some(({ id }) => id === current.editing?.promptId)
    ? current.editing
    : undefined;
  const hasContent = text.trim() !== "" || attachments.length > 0;
  const canSend = (idle || (running && !disabled)) && hasContent;
  const showStop = running && !hasContent;

  const send = (steer: boolean) =>
    attempt(async () => {
      if (editing) await onSaveEdit?.(editing.promptId, text.trim(), attachments);
      else await onSend(text.trim(), attachments, steer);
      unsent.update(unsentId, (latest) =>
        latest === current ? (current.editing?.held ?? undefined) : latest,
      );
    });

  const stop = () => attempt(() => onStop?.() ?? Promise.resolve());

  // Escape stops the work, unless something else handled it first or it
  // closes an open popover, such as the model menu, or a dialog.
  const stopRef = useRef(stop);
  stopRef.current = stop;
  useEffect(() => {
    if (!running) return;
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
  }, [running]);

  const edit = (queued: QueuedPrompt) =>
    unsent.update(unsentId, (latest) => ({
      ...queuedMessage(queued.prompt),
      editing: {
        promptId: queued.id,
        held: latest.editing?.held ?? { text: latest.text, attachments: latest.attachments },
      },
    }));

  const setOption = (option: ConfigOption, value: string | boolean) =>
    attempt(() => onSetOption(option, value));

  // The model picker shows the model and effort options; the rest show as
  // pickers of their own.
  const selects = configOptions.filter(
    (option): option is SelectConfig => option.type === "select",
  );
  const model = selects.find((option) => option.category === "model");
  const effort = model && selects.find((option) => option.category === "thought_level");
  const others = configOptions.filter((option) => option !== model && option !== effort);

  const attach = async () => {
    const paths = await open({ multiple: true, title: "Attach Files" });
    if (paths !== null) {
      unsent.update(unsentId, (latest) => ({
        ...latest,
        attachments: [...new Set([...latest.attachments, ...paths])],
      }));
    }
  };

  return (
    <div className="composer">
      <div className="stack">
        {request && <OpenRequest key={request.request_id} item={request} sessionId={unsentId} />}
        {queue && <QueuedPrompts sessionId={unsentId} queue={queue} onEdit={edit} />}
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
                        unsent.update(unsentId, (latest) => ({
                          ...latest,
                          attachments: latest.attachments.filter((p) => p !== path),
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
                commands={commands}
                disabled={disabled}
                canSend={canSend}
                onChange={(text) => unsent.update(unsentId, (latest) => ({ ...latest, text }))}
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
            <HarnessLogo agent={agent} />
            <ModelPicker
              model={model}
              effort={effort}
              disabled={optionsDisabled}
              onChange={setOption}
            />
          </div>
        )}
        {others.map((option) => (
          <ConfigPicker
            key={option.configId}
            option={option}
            disabled={optionsDisabled}
            onChange={(value) => setOption(option, value)}
          />
        ))}
      </div>
    </div>
  );
}

/**
 * A session's composer. On an idle session, Enter and Cmd+Enter send the
 * message. While the agent works, Enter queues it and Cmd+Enter steers the
 * running turn with it. While the run a child session belongs to is running,
 * the entry and attachments are disabled and Stop and Escape cancel the run. A new
 * session whose agent is starting shows `starting`, the controls it was sent
 * with, disabled, until its agent reports its own.
 */
export const SessionComposer = memo(function SessionComposer({
  session,
  run: parent,
  request,
  unsent,
  starting,
}: {
  session: SessionInfo;
  /** The run the session is a child of. */
  run: RunInfo | undefined;
  /** The session's oldest unanswered permission request or form. */
  request: Request | undefined;
  unsent: Unsents;
  starting: Controls | undefined;
}) {
  const running =
    session.state === "running" ||
    session.state === "awaiting_permission" ||
    session.state === "awaiting_input";
  const runRunning = parent?.state === "running";
  return (
    <Composer
      unsentId={session.id}
      unsent={unsent}
      agent={sessionAgent(session.command)}
      configOptions={starting?.config_options ?? session.config_options}
      commands={starting?.available_commands ?? session.available_commands}
      request={request}
      queue={session.queue}
      running={running || runRunning}
      disabled={session.state === "exited" || runRunning}
      idle={session.state === "idle" && !runRunning}
      optionsDisabled={starting !== undefined}
      onSend={(text, attachments, steer) =>
        invoke(steer || !running ? "prompt" : "queue_add", {
          sessionId: session.id,
          text,
          attachments,
        })
      }
      onSaveEdit={(promptId, text, attachments) =>
        invoke("queue_edit", { sessionId: session.id, promptId, text, attachments })
      }
      onStop={() =>
        runRunning
          ? invoke("cancel_run", { runId: parent.id })
          : invoke("cancel", { sessionId: session.id })
      }
      onSetOption={(option, value) =>
        invoke("set_config_option", {
          sessionId: session.id,
          configId: option.configId,
          value,
        })
      }
    />
  );
});

function ConfigPicker({
  option,
  disabled,
  onChange,
}: {
  option: ConfigOption;
  disabled: boolean;
  onChange: (value: string | boolean) => void;
}) {
  if (option.type === "boolean") {
    return (
      <label className="choice">
        <input
          type="checkbox"
          checked={option.currentValue}
          disabled={disabled}
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
      disabled={disabled}
      onChange={onChange}
    />
  );
}
