import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowUp, Paperclip, Square, X } from "lucide-react";
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { HarnessLogo, sessionAgent } from "../../components/HarnessLogo";
import { SelectPicker } from "../../components/SelectPicker";
import type {
  Command,
  ConfigOption,
  HarnessConfig,
  QueuedPrompt,
  Request,
  SessionInfo,
} from "../../lib/protocol";
import { showError } from "../../lib/showError";
import type { AgentId } from "../sessions/useAgents";
import { CommandInput } from "./CommandInput";
import { pickedSettings, sessionOptions } from "./controls";
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
 * composer is `idle` or `running`; `onSend` gets `steer` set for Cmd+Enter.
 * While `running`, the form's button sends a message that has content and
 * stops otherwise, and Escape stops. While a queued prompt is edited, Enter
 * saves it in its place with `onSaveEdit` and brings back the message it
 * replaced.
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
  const canSend = (idle || running) && hasContent;
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

/** The settings a session's composer shows before the session's config does. */
type Picked = Record<string, string | boolean | null>;

/**
 * A session's composer. On an idle session, Enter and Cmd+Enter send the
 * message. While the agent works, Enter queues it and Cmd+Enter steers the
 * running turn with it. The model, effort and mode pickers show the
 * session's config, with what `harness`, the config of the session's agent,
 * lists, and are disabled while the agent starts or has exited. A pick shows
 * at once, including a new model's effort levels and effort, and sends only
 * what it changed; a model pick also sends its effort.
 */
export const SessionComposer = memo(function SessionComposer({
  session,
  request,
  unsent,
  harness,
}: {
  session: SessionInfo;
  /** The session's oldest unanswered permission request or form. */
  request: Request | undefined;
  unsent: Unsents;
  harness: HarnessConfig | undefined;
}) {
  const running =
    session.state === "running" ||
    session.state === "awaiting_permission" ||
    session.state === "awaiting_input";
  // The settings the user has picked that `session.config` does not show
  // yet, by ID, with null for an effort a model pick removed. Each leaves
  // when the config shows its value, or when the agent refuses it.
  const [picked, setPicked] = useState<Picked>({});
  useEffect(() => {
    setPicked((current) => {
      const pending = Object.entries(current).filter(([configId, value]) => {
        const recorded = session.config.find((setting) => setting.config_id === configId)?.value;
        return value === null ? recorded !== undefined : recorded !== value;
      });
      return pending.length === Object.keys(current).length ? current : Object.fromEntries(pending);
    });
  }, [session.config]);
  const shown = useMemo(
    () => [
      ...session.config
        .filter((setting) => picked[setting.config_id] !== null)
        .map((setting) => ({ ...setting, value: picked[setting.config_id] ?? setting.value })),
      ...Object.entries(picked)
        .filter(
          (entry): entry is [string, string | boolean] =>
            entry[1] !== null && !session.config.some((setting) => setting.config_id === entry[0]),
        )
        .map(([config_id, value]) => ({ config_id, value })),
    ],
    [session.config, picked],
  );
  const options = useMemo(() => sessionOptions(harness, shown), [harness, shown]);
  const setOption = async (option: ConfigOption, value: string | boolean) => {
    const config = pickedSettings(harness, shown, option.configId, value);
    const entries: Picked = Object.fromEntries(
      config.map((setting) => [setting.config_id, setting.value]),
    );
    if (option.configId === "model" && !("effort" in entries)) entries.effort = null;
    setPicked((current) => ({ ...current, ...entries }));
    try {
      await invoke("set_config", { sessionId: session.id, config });
    } catch (error) {
      setPicked((current) =>
        Object.fromEntries(
          Object.entries(current).filter(
            ([id, picked]) => !(id in entries) || picked !== entries[id],
          ),
        ),
      );
      throw error;
    }
  };
  return (
    <Composer
      unsentId={session.id}
      unsent={unsent}
      agent={sessionAgent(session.command)}
      configOptions={options}
      commands={session.available_commands}
      request={request}
      queue={session.queue}
      running={running}
      disabled={session.state === "exited"}
      idle={session.state === "idle"}
      optionsDisabled={session.state === "starting" || session.state === "exited"}
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
      onStop={() => invoke("cancel", { sessionId: session.id })}
      onSetOption={setOption}
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
