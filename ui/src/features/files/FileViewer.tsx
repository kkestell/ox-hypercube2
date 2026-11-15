import { FileTextIcon, X } from "lucide-react";
import { useLayoutEffect, useRef } from "react";
import { openFile, updateViewer, type ViewerState } from "./viewerState";

export function FileViewer({ sessionId, state }: { sessionId: string; state: ViewerState }) {
  const body = useRef<HTMLDivElement>(null);
  const path = state.path!;
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
            onClick={() => openFile(sessionId, null)}
          >
            <X />
          </button>
        </div>
      </header>
      <div
        className="file-body"
        ref={body}
        onScroll={(event) =>
          updateViewer(sessionId, {
            scrollTop: event.currentTarget.scrollTop,
            scrollLeft: event.currentTarget.scrollLeft,
          })
        }
      >
        {state.content?.type === "text" ? (
          <pre>{state.content.text}</pre>
        ) : (
          <p className="file-message">
            {state.content?.type === "error" ? state.content.message : "Loading…"}
          </p>
        )}
      </div>
    </section>
  );
}
