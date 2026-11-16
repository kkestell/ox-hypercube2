import { useEffect, useLayoutEffect, useRef } from "react";
import type { Item, RunInfo, SessionInfo, StepInfo } from "../../lib/protocol";
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
 * session.
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
            followMessages={run.state === "running" && step.state === "running"}
            messages={
              step.session_id === null
                ? []
                : (conversations.get(step.session_id)?.filter(isAgentMessage) ?? [])
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

/**
 * How far the end of `message` is below the bottom of `view`. It is negative
 * when the end is above the bottom, and below `-view.clientHeight` when the
 * end is above the view.
 */
function endBelow(message: Element, view: Element) {
  return message.getBoundingClientRect().bottom - view.getBoundingClientRect().bottom;
}

/**
 * A ref for the active step's latest message that keeps the message's end in
 * the run view's scrolling view as the message grows, until the user scrolls
 * the end out of view. A new latest message is followed again.
 */
function useFollow(latest: AgentMessage | undefined) {
  const element = useRef<HTMLLIElement>(null);
  // Whether the view follows the end of the latest message.
  const following = useRef(true);
  const key = latest?.key;

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new latest message is followed
  useLayoutEffect(() => {
    following.current = true;
  }, [key]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: each change to the latest message scrolls a followed view
  useLayoutEffect(() => {
    const node = element.current;
    if (node === null || !following.current) return;
    const view = node.closest(".conversation")!;
    const below = endBelow(node, view);
    if (below > 0 || below < -view.clientHeight) view.scrollTop += below;
  }, [latest]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: each latest message is a new element
  useEffect(() => {
    const node = element.current;
    if (node === null) return;
    const view = node.closest(".conversation")!;
    const onScroll = () => {
      const below = endBelow(node, view);
      following.current = below < 1 && below >= -view.clientHeight;
    };
    view.addEventListener("scroll", onScroll);
    return () => view.removeEventListener("scroll", onScroll);
  }, [key]);

  return element;
}

function StepStatus({
  step,
  session,
  followMessages,
  messages,
  notice,
  connector,
}: {
  step: StepInfo;
  session: SessionInfo | undefined;
  followMessages: boolean;
  /** The agent messages of the step's child session, oldest first. */
  messages: AgentMessage[];
  /** The run's failure or interruption, shown after the step's messages. */
  notice: string | undefined;
  /** The connector below the step: solid, dashed between two pending steps, or none. */
  connector: "solid" | "dashed" | null;
}) {
  const status = stepOutcome(step, session);
  const latest = followMessages ? messages.at(-1) : undefined;
  const follow = useFollow(latest);
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
      <h3 className="run-status-current">{name}</h3>
      {(messages.length > 0 || notice !== undefined) && (
        <ol className="run-status-messages">
          {messages.map((message) => (
            <li key={message.key} ref={message === latest ? follow : undefined}>
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
