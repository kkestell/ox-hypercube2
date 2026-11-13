import type { ReactNode, Ref, UIEvent } from "react";

/**
 * A sidebar: a pane header holding `actions`, over a scrolling body holding
 * `children`. `ref` and `onScroll` belong to the body.
 */
export function Sidebar({
  actions,
  ref,
  onScroll,
  children,
}: {
  actions?: ReactNode;
  ref?: Ref<HTMLDivElement>;
  onScroll?: (event: UIEvent<HTMLDivElement>) => void;
  children: ReactNode;
}) {
  return (
    <div className="sidebar">
      <header className="pane-header sidebar-header">{actions}</header>
      <div className="sidebar-body" ref={ref} onScroll={onScroll}>
        {children}
      </div>
    </div>
  );
}
