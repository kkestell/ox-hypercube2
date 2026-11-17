import { invoke } from "@tauri-apps/api/core";
import { ArrowUp, Pencil, Trash2 } from "lucide-react";
import type { ContentBlock, QueuedPrompt } from "../../lib/protocol";
import { showError } from "../../lib/showError";
import type { Message } from "./useUnsent";

/** A queued prompt's text blocks, joined, and the names of its attachments. */
function summary(prompt: ContentBlock[]) {
  const texts: string[] = [];
  const names: string[] = [];
  for (const block of prompt) {
    if (block.type === "text" && "text" in block) texts.push(block.text);
    else if (block.type === "resource_link" && "name" in block) names.push(block.name);
  }
  return { text: texts.join("\n\n"), names };
}

/** A queued prompt as a message to edit: its text and its attached files' paths. */
export function queuedMessage(prompt: ContentBlock[]): Message {
  const attachments: string[] = [];
  for (const block of prompt) {
    if (block.type === "resource_link" && "uri" in block && block.uri.startsWith("file://")) {
      attachments.push(decodeURIComponent(block.uri.slice("file://".length)));
    }
  }
  return { text: summary(prompt).text, attachments };
}

/** A session's queued prompts, each with Edit, Send Now and Delete. */
export function QueuedPrompts({
  sessionId,
  queue,
  onEdit,
}: {
  sessionId: string;
  queue: QueuedPrompt[];
  onEdit: (queued: QueuedPrompt) => void;
}) {
  if (queue.length === 0) return null;
  const act = (cmd: string, promptId: string) =>
    invoke(cmd, { sessionId, promptId }).catch(showError);
  return (
    <ul className="queue" aria-label="Queued Messages">
      {queue.map((queued) => {
        const { text, names } = summary(queued.prompt);
        return (
          <li key={queued.id}>
            <span className="text">{text}</span>
            {names.map((name, index) => (
              // biome-ignore lint/suspicious/noArrayIndexKey: a queued prompt's attachments are replaced whole, and names can repeat
              <span key={index} className="attachment">
                {name}
              </span>
            ))}
            <button
              type="button"
              className="icon-button"
              title="Edit"
              aria-label="Edit"
              onClick={() => onEdit(queued)}
            >
              <Pencil />
            </button>
            <button
              type="button"
              className="icon-button"
              title="Send Now"
              aria-label="Send Now"
              onClick={() => act("queue_send", queued.id)}
            >
              <ArrowUp />
            </button>
            <button
              type="button"
              className="icon-button"
              title="Delete"
              aria-label="Delete"
              onClick={() => act("queue_remove", queued.id)}
            >
              <Trash2 />
            </button>
          </li>
        );
      })}
    </ul>
  );
}
