import { FileTextIcon, X } from "lucide-react";
import { type ReactNode, useLayoutEffect, useMemo, useRef } from "react";
import { highlightFile } from "./highlight";
import { openFile, updateViewer, type ViewerState } from "./viewerState";

export function FileViewer({
  viewId,
  state,
  toggles,
}: {
  viewId: string;
  state: ViewerState;
  /** The panel toggles, at the header's end, or null. */
  toggles: ReactNode;
}) {
  const body = useRef<HTMLDivElement>(null);
  const path = state.path!;
  const text = state.content?.type === "text" ? state.content.text : null;
  const highlighted = useMemo(
    () => (text === null ? null : highlightFile(path, text)),
    [path, text],
  );
  useLayoutEffect(() => {
    const element = body.current!;
    element.scrollTop = state.scrollTop;
    element.scrollLeft = state.scrollLeft;
  }, [state.scrollTop, state.scrollLeft]);
  return (
    <section className="file-viewer" aria-label="File viewer">
      <header className="pane-header file-header">
        <div className="file-pill" title={path}>
          <FileTextIcon />
          <span>{path.split("/").at(-1)}</span>
          <button
            type="button"
            className="icon-button"
            aria-label="Close file"
            title="Close file"
            onClick={() => openFile(viewId, null)}
          >
            <X />
          </button>
        </div>
        {toggles}
      </header>
      <div
        className="file-body"
        ref={body}
        onScroll={(event) =>
          updateViewer(viewId, {
            scrollTop: event.currentTarget.scrollTop,
            scrollLeft: event.currentTarget.scrollLeft,
          })
        }
      >
        {state.content?.type === "text" ? (
          <pre>
            <code>{highlighted}</code>
          </pre>
        ) : (
          <p className="file-message">
            {state.content?.type === "error" ? state.content.message : "Loading…"}
          </p>
        )}
      </div>
    </section>
  );
}
