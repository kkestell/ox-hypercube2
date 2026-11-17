import { Check, ChevronRight, X } from "lucide-react";
import { memo, type ReactNode, useLayoutEffect, useMemo, useRef, useState } from "react";
import { CodeSpans } from "../../components/CodeSpans";
import type { Item, ToolCallStatus } from "../../lib/protocol";
import { Markdown } from "./Markdown";

/**
 * A scrolling transcript that follows its end as it grows, from when `viewId`
 * changes until the user scrolls away from the end.
 */
function Transcript({ viewId, children }: { viewId: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  // Whether the view follows its end as its content grows.
  const following = useRef(true);

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new view is followed from its end
  useLayoutEffect(() => {
    following.current = true;
  }, [viewId]);

  // Each render follows a change to the content, which scrolls a followed view.
  useLayoutEffect(() => {
    const node = ref.current!;
    if (following.current) node.scrollTop = node.scrollHeight;
  });

  return (
    <div
      className="conversation"
      ref={ref}
      onScroll={(event) => {
        const node = event.currentTarget;
        following.current = node.scrollHeight - node.scrollTop - node.clientHeight < 1;
      }}
    >
      {children}
    </div>
  );
}

/** Whether `ItemView` renders anything for `item`. */
function shown(item: Item): boolean {
  switch (item.type) {
    case "text":
    case "thought":
      return item.text.trim() !== "";
    // Permission requests and forms show in the composer while they await
    // an answer.
    case "permission":
    case "elicitation":
    case "turn_completed":
      return false;
    default:
      return true;
  }
}

/** A row of the transcript: one item, or a completed turn's work before its reply. */
type Row = { type: "item"; item: Item } | { type: "worked"; key: string; items: Item[] };

/**
 * The transcript's rows. A turn's work is the items after the previous
 * prompt or turn end. When the turn completes and its last shown item is a
 * text reply with shown work before it, that work becomes one `worked` row;
 * otherwise each item is its own row. Steered prompts the agent has not
 * taken in come after every other row.
 */
function rows(items: Item[]): Row[] {
  const rows: Row[] = [];
  const steered: Item[] = [];
  let turn: Item[] = [];
  const flush = (from: number) => {
    for (const item of turn.slice(from)) rows.push({ type: "item", item });
    turn = [];
  };
  for (const item of items) {
    switch (item.type) {
      case "turn_completed": {
        let reply = turn.length - 1;
        while (reply >= 0 && !shown(turn[reply]!)) reply--;
        const work = turn.slice(0, Math.max(reply, 0));
        if (turn[reply]?.type === "text" && work.some(shown)) {
          rows.push({ type: "worked", key: `worked:${work[0]!.key}`, items: work });
          flush(reply);
        } else {
          flush(0);
        }
        rows.push({ type: "item", item });
        break;
      }
      case "prompt":
      case "turn_error":
      case "agent_exited":
      case "session_closed":
        flush(0);
        rows.push({ type: "item", item });
        break;
      case "steered":
        steered.push(item);
        break;
      default:
        turn.push(item);
    }
  }
  flush(0);
  for (const item of steered) rows.push({ type: "item", item });
  return rows;
}

/**
 * A session's items, with each completed turn's work under a Worked group.
 * An item or group renders again only when it changes, so appending to the
 * conversation leaves the rest of it alone.
 */
export const Conversation = memo(function Conversation({
  sessionId,
  items,
}: {
  sessionId: string;
  items: Item[];
}) {
  const transcript = useMemo(() => rows(items), [items]);
  return (
    <Transcript viewId={sessionId}>
      {transcript.map((row) =>
        row.type === "item" ? (
          <ItemView key={row.item.key} item={row.item} />
        ) : (
          <WorkedGroup key={row.key} items={row.items} />
        ),
      )}
    </Transcript>
  );
});

/** A completed turn's work, closed until the user opens it. */
const WorkedGroup = memo(
  function WorkedGroup({ items }: { items: Item[] }) {
    const [open, setOpen] = useState(false);
    return (
      <div className="worked">
        <button
          type="button"
          className="worked-toggle"
          aria-expanded={open}
          onClick={() => setOpen(!open)}
        >
          Worked
          <ChevronRight />
        </button>
        {open && (
          <div className="worked-items">
            {items.map((item) => (
              <ItemView key={item.key} item={item} />
            ))}
          </div>
        )}
      </div>
    );
  },
  (previous, next) =>
    previous.items.length === next.items.length &&
    previous.items.every((item, index) => item === next.items[index]),
);

const ItemView = memo(function ItemView({ item }: { item: Item }) {
  if (!shown(item)) return null;
  switch (item.type) {
    case "prompt":
    case "steered": {
      const prompt = (
        <div className={item.type === "steered" ? "prompt steered" : "prompt"}>
          <Markdown text={item.text} />
          {item.attachments.map((name, index) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: a prompt's attachments never change, and names can repeat
            <span key={index} className="attachment">
              {name}
            </span>
          ))}
        </div>
      );
      if (!item.from) return prompt;
      return (
        <div className="peer-message">
          <p className="sender">
            {item.from.title ?? `Session ${item.from.session_id.slice(0, 8)}`}
          </p>
          {prompt}
        </div>
      );
    }
    case "text":
      return <Markdown text={item.text} />;
    case "thought":
      return (
        <details className="thought">
          <summary>Thinking</summary>
          <Markdown text={item.text} />
        </details>
      );
    case "tool_call":
      return <ToolCall item={item} />;
    case "permission":
    case "elicitation":
    case "turn_completed":
      return null;
    case "turn_error":
      return <p className="turn-error">{item.message}</p>;
    case "agent_exited":
      return (
        <p className="agent-exited">
          {item.error ? `The agent exited: ${item.error}` : "The agent exited"}
        </p>
      );
    case "session_closed":
      return <p className="session-closed">Session closed</p>;
  }
});

function ToolStatus({ status }: { status: ToolCallStatus }) {
  switch (status) {
    case "pending":
    case "in_progress":
      return <span className="status running" role="img" aria-label="Running" title="Running" />;
    case "completed":
      return (
        <span className="status completed" role="img" aria-label="Completed" title="Completed">
          <Check />
        </span>
      );
    case "failed":
      return (
        <span className="status error" role="img" aria-label="Failed" title="Failed">
          <X />
        </span>
      );
  }
}

function ToolCall({ item }: { item: Extract<Item, { type: "tool_call" }> }) {
  const heading = (
    <>
      <ToolStatus status={item.status} />
      <span className="title">
        <CodeSpans text={item.title} />
      </span>
    </>
  );
  const input = item.raw_input;
  if (input === null && item.content.length === 0) {
    return (
      <div className="tool-call">
        <div className="heading">{heading}</div>
      </div>
    );
  }
  return (
    <details className="tool-call">
      <summary className="heading">{heading}</summary>
      <div className="details">
        {input !== null && (
          <section>
            <p className="label">Arguments</p>
            <pre>{JSON.stringify(input, null, 2)}</pre>
          </section>
        )}
        {item.content.length > 0 && (
          <section>
            <p className="label">Output</p>
            {item.content.map((content, index) =>
              content.type === "text" ? (
                // biome-ignore lint/suspicious/noArrayIndexKey: a tool call's content is replaced whole
                <pre key={index}>{content.text}</pre>
              ) : (
                <img
                  // biome-ignore lint/suspicious/noArrayIndexKey: a tool call's content is replaced whole
                  key={index}
                  alt=""
                  src={`data:${content.mime_type};base64,${content.data}`}
                />
              ),
            )}
          </section>
        )}
      </div>
    </details>
  );
}
