import {
  CircleCheck,
  CircleDashed,
  CircleDot,
  CirclePause,
  CircleSlash,
  CircleX,
} from "lucide-react";
import type { ComponentType } from "react";
import type { Item, RunInfo, SessionInfo, StepInfo } from "../../lib/protocol";
import { childStatus } from "../sessions/SessionList";

type StatusUpdate = Extract<Item, { type: "status_update" }>;

function isStatusUpdate(item: Item): item is StatusUpdate {
  return item.type === "status_update";
}

const icons: Record<string, ComponentType> = {
  running: CircleDot,
  attention: CircleDot,
  succeeded: CircleCheck,
  failed: CircleX,
  pending: CircleDashed,
  cancelled: CircleSlash,
  interrupted: CirclePause,
};

/**
 * The run's failure or interruption, and the index of the step it belongs
 * to: the first failed step, or the first interrupted one. The index is -1
 * when no step matches.
 */
export function runNotice(run: RunInfo): { text: string; index: number } | null {
  const text =
    run.state === "failed" && run.error !== null
      ? run.error
      : run.state === "interrupted"
        ? "The run was interrupted when oxd stopped"
        : null;
  if (text === null) return null;
  const state = run.state === "failed" ? "failed" : "interrupted";
  return { text, index: run.steps.findIndex((step) => step.state === state) };
}

/** A time as `HH:MM`. */
function clock(time: number) {
  return new Date(time).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
}

/**
 * Every step of the run, in order, with the status updates its child
 * session's agent posted.
 */
export function RunStatus({
  run,
  sessions,
  conversations,
  notice,
}: {
  run: RunInfo;
  sessions: SessionInfo[];
  /** The child sessions' conversations, by session ID. */
  conversations: Map<string, Item[]>;
  /** The run's `runNotice`. */
  notice: { text: string; index: number } | null;
}) {
  return (
    <div className="run-status">
      {run.steps.map((step, index) => {
        const next = run.steps[index + 1];
        return (
          <StepStatus
            // biome-ignore lint/suspicious/noArrayIndexKey: a run's steps never move
            key={index}
            step={step}
            session={sessions.find((session) => session.id === step.session_id)}
            followUpdates={run.state === "running" && step.state === "running"}
            updates={
              step.session_id === null
                ? []
                : (conversations.get(step.session_id)?.filter(isStatusUpdate) ?? [])
            }
            notice={notice?.index === index ? notice.text : undefined}
            // The connector below the step, or null for the last step.
            connector={
              next === undefined
                ? null
                : step.state === "pending" && next.state === "pending"
                  ? "dashed"
                  : "solid"
            }
          />
        );
      })}
    </div>
  );
}

function revealUpdate(element: HTMLLIElement | null) {
  element?.scrollIntoView({ block: "nearest" });
}

function StepStatus({
  step,
  session,
  followUpdates,
  updates,
  notice,
  connector,
}: {
  step: StepInfo;
  session: SessionInfo | undefined;
  followUpdates: boolean;
  /** The status updates of the step's child session, oldest first. */
  updates: StatusUpdate[];
  /** The run's failure or interruption, shown after the step's updates. */
  notice: string | undefined;
  /** The connector below the step: solid, dashed between two pending steps, or none. */
  connector: "solid" | "dashed" | null;
}) {
  const status = childStatus(step, session);
  const Icon = icons[status.className || step.state] ?? CircleDashed;
  const latest = followUpdates ? updates.at(-1) : undefined;
  const name = (
    <>
      <span className={`status ${status.className}`} role="img" aria-label={status.label}>
        <Icon />
      </span>
      <span className="title">{step.name}</span>
    </>
  );

  return (
    <section
      className={`run-status-step ${step.state}${connector ? ` connector ${connector}` : ""}`}
      aria-label={step.name}
    >
      {step.started_at !== null && (
        <time className="run-status-time" dateTime={new Date(step.started_at).toISOString()}>
          {clock(step.started_at)}
        </time>
      )}
      <h3 className="run-status-current">{name}</h3>
      {(updates.length > 0 || notice !== undefined) && (
        <ol className="run-status-updates">
          {updates.map((update) => (
            <li key={update.key} ref={update === latest ? revealUpdate : undefined}>
              <time dateTime={new Date(update.posted_at).toISOString()}>
                {clock(update.posted_at)}
              </time>
              <p>{update.text}</p>
            </li>
          ))}
          {notice !== undefined && (
            <li className="run-status-error">
              <p>{notice}</p>
            </li>
          )}
        </ol>
      )}
    </section>
  );
}
