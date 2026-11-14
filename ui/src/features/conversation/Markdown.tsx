import { openUrl } from "@tauri-apps/plugin-opener";
import type { Root as HastRoot } from "hast";
import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import type { RootContent } from "mdast";
import { toHast } from "mdast-util-to-hast";
import { type ComponentProps, memo, useState } from "react";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import { visit } from "unist-util-visit";
import { showError } from "../../lib/showError";
import { BlockParser } from "./blocks";

/**
 * Transcript prose, rendered from the complete text accumulated so far. Each
 * top-level block renders on its own, so text appended to the last block
 * parses and renders only that block again.
 */
export const Markdown = memo(function Markdown({ text }: { text: string }) {
  const [parser] = useState(() => new BlockParser());
  return (
    <div className="text markdown">
      {parser.parse(text).map((block, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: blocks are only appended, and only the last changes
        <Block key={index} block={block} />
      ))}
    </div>
  );
});

/**
 * A top-level Markdown block. Raw HTML shows as its source text, and a URL
 * with a scheme other than a web, chat or email one is dropped.
 */
const Block = memo(function Block({ block }: { block: RootContent }) {
  const tree = toHast(
    { type: "root", children: [block] },
    { allowDangerousHtml: true },
  ) as HastRoot;
  visit(tree, (node, index, parent) => {
    if (node.type === "raw" && parent && index !== undefined) {
      parent.children[index] = { type: "text", value: node.value };
    } else if (node.type === "element") {
      for (const name of ["href", "src"]) {
        const value = node.properties[name];
        if (value !== undefined) node.properties[name] = safeUrl(String(value));
      }
    }
  });
  return toJsxRuntime(tree, { Fragment, jsx, jsxs, components, ignoreInvalidStyle: true });
});

/** `url` if it is relative or its scheme is safe to follow, otherwise empty. */
function safeUrl(url: string) {
  const colon = url.indexOf(":");
  const scheme = colon === -1 ? null : url.slice(0, colon);
  if (scheme === null || /[/?#]/.test(scheme) || /^(https?|ircs?|mailto|xmpp)$/i.test(scheme)) {
    return url;
  }
  return "";
}

/** Only web and email links have a destination outside the transcript. */
function Link({ href, title, children }: ComponentProps<"a">) {
  if (!href || !/^(https?:\/\/|mailto:)/i.test(href)) return <>{children}</>;
  return (
    <a
      href={href}
      title={title}
      onClick={(event) => {
        event.preventDefault();
        openUrl(event.currentTarget.href).catch(showError);
      }}
    >
      {children}
    </a>
  );
}

/** Wide tables scroll locally instead of widening the conversation. */
function Table({ children }: ComponentProps<"table">) {
  return (
    <div className="markdown-table">
      <table>{children}</table>
    </div>
  );
}

const components = { a: Link, table: Table };
