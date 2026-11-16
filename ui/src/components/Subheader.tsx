import type { ReactNode } from "react";

/**
 * A view's subheader, full width beneath the pane header: its icon and label
 * pairs, and its action at the end.
 */
export function Subheader({ action, children }: { action?: ReactNode; children: ReactNode }) {
  return (
    <header className="subheader">
      <dl className="subheader-meta">{children}</dl>
      {action}
    </header>
  );
}
