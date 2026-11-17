import { type ReactNode, useCallback, useRef, useSyncExternalStore } from "react";
import { ContentHeader } from "../../components/Workbench";
import { FileViewer } from "./FileViewer";
import { subscribeViewer, updateViewer, viewerState } from "./viewerState";

/**
 * A session's or draft's content, beside its open file. `toggles` end the
 * rightmost header: the file viewer's while a file is open, otherwise the
 * content's.
 */
export function SessionContent({
  viewId,
  title,
  toggles,
  children,
}: {
  viewId: string;
  title: ReactNode;
  toggles: ReactNode;
  children: ReactNode;
}) {
  const subscribe = useCallback(
    (listener: () => void) => subscribeViewer(viewId, listener),
    [viewId],
  );
  const state = useSyncExternalStore(subscribe, () => viewerState(viewId));
  const split = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);
  return (
    <div className="session-content" ref={split}>
      <main
        className="content chat-pane"
        style={state.path ? { flex: `${state.ratio} 1 0` } : undefined}
      >
        <ContentHeader title={title}>{state.path ? null : toggles}</ContentHeader>
        <div className="content-body">{children}</div>
      </main>
      {state.path && (
        <>
          <div
            className="sash vertical file-sash"
            title="Resize file viewer"
            onPointerDown={(event) => {
              if (event.button !== 0) return;
              event.preventDefault();
              event.currentTarget.setPointerCapture(event.pointerId);
              dragging.current = true;
              document.body.classList.add("resizing", "vertical");
            }}
            onPointerMove={(event) => {
              if (!dragging.current) return;
              const bounds = split.current!.getBoundingClientRect();
              const width = bounds.width - event.currentTarget.getBoundingClientRect().width;
              updateViewer(viewId, {
                ratio: Math.max(0.25, Math.min(0.75, (event.clientX - bounds.left) / width)),
              });
            }}
            onLostPointerCapture={() => {
              dragging.current = false;
              document.body.classList.remove("resizing", "vertical");
            }}
          />
          <div className="file-pane" style={{ flex: `${1 - state.ratio} 1 0` }}>
            <FileViewer key={state.path} viewId={viewId} state={state} toggles={toggles} />
          </div>
        </>
      )}
    </div>
  );
}
