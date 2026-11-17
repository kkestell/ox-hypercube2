import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { ArrowUp, Paperclip, Square, X } from "lucide-react";
import { memo } from "react";
import { HarnessLogo, sessionAgent } from "../../components/HarnessLogo";
import { SelectPicker } from "../../components/SelectPicker";
import type {
  Command,
  ConfigOption,
  Controls,
  Request,
  RunInfo,
  SessionInfo,
} from "../../lib/protocol";
import { showError } from "../../lib/showError";
import type { AgentId } from "../sessions/useAgents";
import { CommandInput } from "./CommandInput";
import { ModelPicker, type SelectConfig } from "./ModelPicker";
import { OpenRequest } from "./OpenRequest";
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
 * A composer: the entry with its attachments and slash commands, and the
 * agent's config options. `unsentId` names the unsent text it edits. The
 * entry and attachments are disabled while `disabled`, Send is enabled
 * while `idle` and there is something to send, and Stop shows while
 * `running`.
 */
export function Composer({
  unsentId,
  unsent,
  agent,
  configOptions,
  commands,
  request,
  running,
  disabled,
  idle,
  optionsDisabled = false,
  onSend,
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
  running: boolean;
  disabled: boolean;
  idle: boolean;
  /** Whether the config options show without being settable. */
  optionsDisabled?: boolean;
  onSend: (text: string, attachments: string[]) => Promise<unknown>;
  onStop?: () => Promise<unknown>;
  onSetOption: (option: ConfigOption, value: string | boolean) => Promise<unknown>;
}) {
  const current = unsent.get(unsentId);
  const { text, attachments } = current;
  const canSend = idle && (text.trim() !== "" || attachments.length > 0);

  const send = () =>
    attempt(async () => {
      await onSend(text.trim(), attachments);
      unsent.update(unsentId, (latest) => (latest === current ? undefined : latest));
    });

  const stop = () => attempt(() => onStop?.() ?? Promise.resolve());

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
        <div className="entry">
          <form
            className="box"
            onSubmit={(event) => {
              event.preventDefault();
              if (running) stop();
              else if (canSend) send();
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
                title={running ? "Stop" : "Send"}
                aria-label={running ? "Stop" : "Send"}
                disabled={!running && !canSend}
              >
                {running ? <Square /> : <ArrowUp />}
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
 * A session's composer. While the run a child session belongs to is running,
 * the entry and attachments are disabled and Stop cancels the run. A new
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
      running={running}
      disabled={session.state === "exited" || runRunning}
      idle={session.state === "idle" && !runRunning}
      optionsDisabled={starting !== undefined}
      onSend={(text, attachments) => invoke("prompt", { sessionId: session.id, text, attachments })}
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
