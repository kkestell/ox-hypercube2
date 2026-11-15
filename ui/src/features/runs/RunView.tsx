import { Check, X } from "lucide-react";
import { memo } from "react";
import type {
  CommandInfo,
  CommandKind,
  CommandState,
  Item,
  RunInfo,
  SessionInfo,
  StepInfo,
} from "../../lib/protocol";
import { Elicitation } from "../composer/Elicitation";
import { Permission } from "../composer/Permission";
import { ItemView, Transcript } from "../conversation/Conversation";
import { useConversations } from "../conversation/useConversations";
import { childStatus } from "../sessions/SessionList";

type Request = Extract<Item, { type: "permission" | "elicitation" }>;

function isRequest(item: Item): item is Request {
  return item.type === "permission" || item.type === "elicitation";
}

/** What a step's timeline shows: a command it ran, or an item of its child's conversation. */
export type TimelineEntry =
  | { type: "command"; key: string; command: CommandInfo }
  | { type: "item"; item: Item };

/**
 * A step's commands and its child's items in the order the runner ran them:
 * the preconditions, a shell step's command, the child's first turn, the
 * postconditions up to and including the first that failed, then, after one
 * failed, the repair turn and the remaining postconditions, and last the
 * child's later turns. The runner prompts a step once, and repairs it once
 * after a failed postcondition; each turn starts with a `prompt` item.
 */
export function stepTimeline(step: StepInfo, items: readonly Item[]): TimelineEntry[] {
  const commands = step.commands.map((command, index): TimelineEntry & { type: "command" } => ({
    type: "command",
    key: `command:${index}`,
    command,
  }));
  const ofKind = (kind: CommandKind) => commands.filter((entry) => entry.command.kind === kind);
  const postconditions = ofKind("postcondition");
  const failed = postconditions.findIndex((entry) => entry.command.state === "failed");
  const checked = failed === -1 ? postconditions : postconditions.slice(0, failed + 1);
  const rechecked = failed === -1 ? [] : postconditions.slice(failed + 1);
  const prompts = items.flatMap((item, index) => (item.type === "prompt" ? [index] : []));
  // Where turn `turn`, counted from 0, starts.
  const turnStart = (turn: number) => prompts[turn] ?? items.length;
  const firstTurnEnd = turnStart(1);
  const repairEnd = failed === -1 ? firstTurnEnd : turnStart(2);
  const span = (from: number, to: number) =>
    items.slice(from, to).map((item): TimelineEntry => ({ type: "item", item }));
  return [
    ...ofKind("precondition"),
    ...ofKind("shell"),
    ...span(0, firstTurnEnd),
    ...checked,
    ...span(firstTurnEnd, repairEnd),
    ...rechecked,
    ...span(repairEnd, items.length),
  ];
}

const noItems: Item[] = [];

/**
 * A run's started steps, each with its commands and its child's
 * conversation, followed live, and below them the oldest open permission
 * request or form among the children, in step order.
 */
export function RunView({
  run,
  sessions,
  connection,
}: {
  run: RunInfo;
  sessions: SessionInfo[];
  connection: number;
}) {
  const childIds = run.steps.flatMap((step) => (step.session_id === null ? [] : [step.session_id]));
  const conversations = useConversations(childIds, connection);
  let request: { item: Request; sessionId: string } | undefined;
  for (const sessionId of childIds) {
    const item = conversations.get(sessionId)?.find(isRequest);
    if (item) {
      request = { item, sessionId };
      break;
    }
  }

  return (
    <>
      <Transcript viewId={run.id}>
        {run.steps.map((step, position) => {
          if (step.state === "pending") return null;
          const session = sessions.find((session) => session.id === step.session_id);
          const status = childStatus(step, session);
          return (
            <StepSection
              // biome-ignore lint/suspicious/noArrayIndexKey: a run's steps never move
              key={position}
              step={step}
              items={(step.session_id !== null && conversations.get(step.session_id)) || noItems}
              status={status.className}
              label={status.label}
            />
          );
        })}
        {run.state === "failed" && run.error !== null && <p className="turn-error">{run.error}</p>}
        {run.state === "interrupted" && (
          <p className="agent-exited">The run was interrupted when oxd stopped</p>
        )}
      </Transcript>
      {request && (
        <div className="composer">
          <div className="stack">
            {request.item.type === "permission" ? (
              <Permission
                key={`${request.sessionId}:${request.item.request_id}`}
                item={request.item}
                sessionId={request.sessionId}
              />
            ) : (
              <Elicitation
                key={`${request.sessionId}:${request.item.request_id}`}
                item={request.item}
                sessionId={request.sessionId}
              />
            )}
          </div>
        </div>
      )}
    </>
  );
}

/** A started step: its heading, then its timeline. */
const StepSection = memo(function StepSection({
  step,
  items,
  status,
  label,
}: {
  step: StepInfo;
  /** The child's items. */
  items: Item[];
  /** The dot's class and its label, from `childStatus`. */
  status: string;
  label: string;
}) {
  return (
    <section className="step" aria-label={step.name}>
      <h3 className="step-heading">
        <span className={`status hollow ${status}`} role="img" aria-label={label} title={label} />
        <span>{step.name}</span>
      </h3>
      {stepTimeline(step, items).map((entry) =>
        entry.type === "command" ? (
          <CommandView key={entry.key} command={entry.command} />
        ) : (
          <ItemView key={entry.item.key} item={entry.item} />
        ),
      )}
    </section>
  );
});

const headings: Record<CommandKind, string> = {
  precondition: "Precondition",
  shell: "Command",
  postcondition: "Postcondition",
};

function CommandStatus({ state }: { state: CommandState }) {
  switch (state) {
    case "running":
      return <span className="status running" role="img" aria-label="Running" title="Running" />;
    case "succeeded":
      return (
        <span className="status completed" role="img" aria-label="Succeeded" title="Succeeded">
          <Check />
        </span>
      );
    case "failed":
      return (
        <span className="status failed" role="img" aria-label="Failed" title="Failed">
          <X />
        </span>
      );
    case "cancelled":
      return <span className="status" role="img" aria-label="Cancelled" title="Cancelled" />;
    case "interrupted":
      return <span className="status" role="img" aria-label="Interrupted" title="Interrupted" />;
  }
}

/** A check or a shell step's command, shown as a tool call is. */
function CommandView({ command }: { command: CommandInfo }) {
  return (
    <details className="tool-call command">
      <summary className="heading">
        <CommandStatus state={command.state} />
        <span className="title">
          {headings[command.kind]}
          <code>{command.command.split("\n")[0]}</code>
        </span>
      </summary>
      <div className="details">
        <section>
          <p className="label">Command</p>
          <pre>{command.command}</pre>
        </section>
        {command.exit_code !== null && (
          <section>
            <p>Exited with status {command.exit_code}</p>
          </section>
        )}
        {command.output !== "" && (
          <section>
            <p className="label">{command.truncated ? "Output (last 64 KiB)" : "Output"}</p>
            <pre>{command.output}</pre>
          </section>
        )}
      </div>
    </details>
  );
}
