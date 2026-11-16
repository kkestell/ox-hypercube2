import { ChevronRight } from "lucide-react";
import { type CSSProperties, useState } from "react";
import type { Item, RunInfo, SessionInfo, StepInfo } from "../../lib/protocol";
import { useFollowEnd } from "../conversation/Conversation";
import { Markdown } from "../conversation/Markdown";
import {
  awaiting,
  type Status,
  StatusIcon,
  stateLabels,
  stepLabels,
  working,
} from "../sessions/SessionList";

type AgentMessage = Extract<Item, { type: "text" }>;

/** Whether `item` is an agent message with text. */
function isAgentMessage(item: Item): item is AgentMessage {
  return item.type === "text" && item.text.trim() !== "";
}

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

/**
 * A step's outcome, from its state and its child session's, if it has one:
 * what the session awaits, running, green succeeded, red failed, or gray.
 */
function stepOutcome(step: StepInfo, session: SessionInfo | undefined): Status {
  if (session && awaiting(session)) {
    return { className: "attention", label: stateLabels[session.state] };
  }
  if (step.state === "running" || working(session))
    return { className: "running", label: "Running" };
  const label = stepLabels[step.state];
  if (step.state === "succeeded") return { className: "finished", label };
  if (step.state === "failed") return { className: "error", label };
  return { className: null, label };
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
 * Every step of the run, in order, with the agent messages of its child
 * session. The steps fill the view's height: a step with messages or the
 * run's notice expands and collapses, is expanded until it has succeeded and
 * a later step has messages or the notice, unless the user toggles it, and
 * shares the height left by the collapsed steps with the other expanded steps.
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
  // Whether each step the user toggled is expanded, by index.
  const [toggled, setToggled] = useState<ReadonlyMap<number, boolean>>(new Map());
  const shown = run.steps.map((step, index) => {
    const messages =
      step.session_id === null
        ? []
        : (conversations.get(step.session_id)?.filter(isAgentMessage) ?? []);
    const stepNotice = notice?.index === index ? notice.text : undefined;
    return {
      step,
      messages,
      notice: stepNotice,
      expandable: messages.length > 0 || stepNotice !== undefined,
    };
  });
  const steps = shown.map((shownStep, index) => {
    // A succeeded step stays expanded until a later step has something to show.
    const done =
      shownStep.step.state === "succeeded" &&
      shown.slice(index + 1).some((later) => later.expandable);
    return {
      ...shownStep,
      expanded: shownStep.expandable && (toggled.get(index) ?? !done),
    };
  });
  const expanded = steps.filter((step) => step.expanded).length;
  const style = {
    gridTemplateRows: steps
      .map((step) => (step.expanded ? "minmax(0, max-content)" : "max-content"))
      .join(" "),
    "--run-status-collapsed": steps.length - expanded,
    "--run-status-expanded": expanded,
  } as CSSProperties;

  return (
    <div className="run-status" style={style}>
      {steps.map(({ step, messages, notice, expandable, expanded }, index) => {
        const next = run.steps[index + 1];
        return (
          <StepStatus
            // biome-ignore lint/suspicious/noArrayIndexKey: a run's steps never move
            key={index}
            step={step}
            session={sessions.find((session) => session.id === step.session_id)}
            messages={messages}
            notice={notice}
            expanded={expandable ? expanded : null}
            onToggle={() => setToggled((current) => new Map(current).set(index, !expanded))}
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

function StepStatus({
  step,
  session,
  messages,
  notice,
  expanded,
  onToggle,
  connector,
}: {
  step: StepInfo;
  session: SessionInfo | undefined;
  /** The agent messages of the step's child session, oldest first. */
  messages: AgentMessage[];
  /** The run's failure or interruption, shown after the step's messages. */
  notice: string | undefined;
  /** Whether the step shows its messages, or null when it has none to show. */
  expanded: boolean | null;
  onToggle: () => void;
  /** The connector below the step: solid, dashed between two pending steps, or none. */
  connector: "solid" | "dashed" | null;
}) {
  const status = stepOutcome(step, session);
  const follow = useFollowEnd<HTMLOListElement>(expanded);
  const name = (
    <>
      <StatusIcon status={status} />
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
      <h3 className="run-status-current">
        {expanded === null ? (
          name
        ) : (
          <button
            type="button"
            className="run-status-toggle"
            aria-expanded={expanded}
            onClick={onToggle}
          >
            {name}
            <ChevronRight />
          </button>
        )}
      </h3>
      {expanded && (
        <ol className="run-status-messages" ref={follow.ref} onScroll={follow.onScroll}>
          {messages.map((message) => (
            <li key={message.key}>
              <Markdown text={message.text} />
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
