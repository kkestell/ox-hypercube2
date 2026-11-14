import type { RootContent } from "mdast";
import { fromMarkdown } from "mdast-util-from-markdown";
import { gfmFromMarkdown } from "mdast-util-gfm";
import { gfm } from "micromark-extension-gfm";

/**
 * Parses Markdown text, with GitHub's extensions, into its top-level blocks.
 * Text that grows by appending is parsed again only from the start of its
 * last block, the one block that later text can still change; the blocks
 * before it are the same objects as before.
 *
 * TODO: A link reference definition or footnote definition in one block does
 * not resolve references in another.
 */
export class BlockParser {
  private text = "";
  /** The blocks before the last block. */
  private settled: RootContent[] = [];
  /** Where the last block starts in `text`. */
  private last = 0;
  private blocks: RootContent[] = [];

  parse(text: string): RootContent[] {
    if (text === this.text) return this.blocks;
    if (!text.startsWith(this.text)) {
      this.settled = [];
      this.last = 0;
    }
    const nodes = fromMarkdown(text.slice(this.last), {
      extensions: [gfm()],
      mdastExtensions: [gfmFromMarkdown()],
    }).children;
    const lastNode = nodes.pop();
    this.settled.push(...nodes);
    if (lastNode) this.last += lastNode.position!.start.offset!;
    this.text = text;
    this.blocks = lastNode ? [...this.settled, lastNode] : [...this.settled];
    return this.blocks;
  }
}
