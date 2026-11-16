import type { Request } from "../../lib/protocol";
import { Elicitation } from "./Elicitation";
import { Permission } from "./Permission";

/** A session's open permission request or form. */
export function OpenRequest({ item, sessionId }: { item: Request; sessionId: string }) {
  return item.type === "permission" ? (
    <Permission item={item} sessionId={sessionId} />
  ) : (
    <Elicitation item={item} sessionId={sessionId} />
  );
}
