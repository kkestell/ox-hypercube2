import { EyeIcon, FileTextIcon, X } from "lucide-react";
import { type ReactNode, useLayoutEffect, useMemo, useRef } from "react";
import { Markdown } from "../conversation/Markdown";
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
  const markdown = /\.(md|markdown)$/i.test(path);
  const preview = markdown && state.preview;
  const highlighted = useMemo(
    () => (text === null || preview ? null : highlightFile(path, text)),
    [path, text, preview],
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
        {markdown && (
          <button
            type="button"
            className="icon-button preview-toggle"
            aria-label="Preview"
            title="Preview"
            aria-pressed={preview}
            onClick={() => updateViewer(viewId, { preview: !preview, scrollTop: 0, scrollLeft: 0 })}
          >
            <EyeIcon />
          </button>
        )}
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
        {text !== null && preview ? (
          <Markdown text={text} />
        ) : text !== null ? (
          <pre>{highlighted}</pre>
        ) : (
          <p className="file-message">
            {state.content?.type === "error" ? state.content.message : "Loading…"}
          </p>
        )}
      </div>
    </section>
  );
}
