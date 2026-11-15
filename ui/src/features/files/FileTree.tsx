import { FileIcon, FolderIcon, FolderOpenIcon } from "lucide-react";
import { type CSSProperties, memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Sidebar } from "../../components/Sidebar";
import type { DirEntry } from "../../lib/protocol";
import { loadTreeState, saveTreeState } from "./treeState";
import { useDirectory } from "./useDirectory";

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** Folders first, then by name, as VS Code's explorer sorts. */
function compareEntries(a: DirEntry, b: DirEntry) {
  if (a.kind !== b.kind) return a.kind === "directory" ? -1 : 1;
  return collator.compare(a.name, b.name);
}

function join(path: string, name: string) {
  return path.endsWith("/") ? path + name : `${path}/${name}`;
}

/** What ends a pending scroll restore: the user scrolling the tree. */
const userScrolls = ["wheel", "pointerdown", "keydown"] as const;

/**
 * The file tree of the directory at `root`, with the open folders and scroll
 * position saved for `viewId`.
 */
export const FileTree = memo(function FileTree({
  viewId,
  root,
  connection,
  onOpenFile,
}: {
  viewId: string;
  root: string;
  connection: number;
  onOpenFile: (relative: string) => void;
}) {
  const [saved] = useState(() => loadTreeState(viewId));
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set(saved.open));
  const scrollTop = useRef(saved.scrollTop);
  const restoring = useRef(saved.scrollTop > 0);
  const body = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLUListElement>(null);

  useEffect(() => {
    saveTreeState(viewId, { open: [...open], scrollTop: scrollTop.current });
  }, [viewId, open]);

  // Folders open as their listings arrive, so the saved position is set
  // again each time the tree grows, until the body reaches it or the user
  // scrolls.
  useLayoutEffect(() => {
    if (!restoring.current) return;
    const element = body.current!;
    const target = scrollTop.current;
    const observer = new ResizeObserver(() => restore());
    const stop = () => {
      observer.disconnect();
      for (const type of userScrolls) element.removeEventListener(type, end);
    };
    const end = () => {
      restoring.current = false;
      stop();
    };
    const restore = () => {
      element.scrollTop = target;
      if (element.scrollTop >= target) end();
    };
    observer.observe(content.current!);
    for (const type of userScrolls) element.addEventListener(type, end);
    restore();
    return stop;
  }, []);

  const toggle = (path: string) =>
    setOpen((open) => {
      const next = new Set(open);
      if (!next.delete(path)) next.add(path);
      return next;
    });

  return (
    <Sidebar
      ref={body}
      onScroll={(event) => {
        if (restoring.current) return;
        scrollTop.current = event.currentTarget.scrollTop;
        saveTreeState(viewId, { open: [...open], scrollTop: scrollTop.current });
      }}
    >
      <ul ref={content}>
        <Folder
          path={root}
          relative=""
          depth={0}
          onOpenFile={onOpenFile}
          connection={connection}
          open={open}
          toggle={toggle}
        />
      </ul>
    </Sidebar>
  );
});

/** The rows of an open folder's entries, each open folder's below it. */
function Folder({
  path,
  relative,
  depth,
  connection,
  onOpenFile,
  open,
  toggle,
}: {
  path: string;
  /** The folder's path relative to the tree's root; empty for the root. */
  relative: string;
  depth: number;
  connection: number;
  onOpenFile: (relative: string) => void;
  open: ReadonlySet<string>;
  toggle: (relative: string) => void;
}) {
  const entries = [...useDirectory(path, connection)].sort(compareEntries);
  const style = { "--depth": depth } as CSSProperties;
  return entries.map((entry) => {
    if (entry.kind === "file") {
      return (
        <li key={entry.name}>
          <div className="sidebar-row tree-row" style={style}>
            <button
              type="button"
              className="select"
              onClick={() => onOpenFile(relative ? `${relative}/${entry.name}` : entry.name)}
            >
              <span className="status">
                <FileIcon />
              </span>
              <span className="title">{entry.name}</span>
            </button>
          </div>
        </li>
      );
    }
    const childRelative = relative === "" ? entry.name : `${relative}/${entry.name}`;
    const expanded = open.has(childRelative);
    return (
      <li key={entry.name}>
        <div className="sidebar-row tree-row" style={style}>
          <button
            type="button"
            className="select"
            aria-expanded={expanded}
            onClick={() => toggle(childRelative)}
          >
            <span className="status">{expanded ? <FolderOpenIcon /> : <FolderIcon />}</span>
            <span className="title">{entry.name}</span>
          </button>
        </div>
        {expanded && (
          <ul>
            <Folder
              path={join(path, entry.name)}
              relative={childRelative}
              depth={depth + 1}
              onOpenFile={onOpenFile}
              connection={connection}
              open={open}
              toggle={toggle}
            />
          </ul>
        )}
      </li>
    );
  });
}
