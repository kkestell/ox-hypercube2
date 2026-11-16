import { FileDiff, Files } from "lucide-react";
import type { ReactNode } from "react";
import { FileTree } from "./FileTree";

export type RightPanelTab = "files" | "changes";

const tabs: { id: RightPanelTab; title: string; icon: ReactNode }[] = [
  { id: "files", title: "Files", icon: <Files /> },
  { id: "changes", title: "Changes", icon: <FileDiff /> },
];

/**
 * The right sidebar: the viewed directory's Files, and its Git Changes, with
 * the panel toggles at its header's end.
 */
export function RightPanel({
  tab,
  onTab,
  toggles,
  viewId,
  root,
  connection,
  onOpenFile,
}: {
  tab: RightPanelTab;
  onTab: (tab: RightPanelTab) => void;
  toggles: ReactNode;
  viewId: string;
  root: string;
  connection: number;
  onOpenFile: (path: string) => void;
}) {
  return (
    <div className="sidebar">
      <header className="pane-header sidebar-header">
        <div className="panel-tabs" role="tablist">
          {tabs.map(({ id, title, icon }) => (
            <button
              key={id}
              type="button"
              className="icon-button"
              role="tab"
              title={title}
              aria-label={title}
              aria-selected={tab === id}
              onClick={() => onTab(id)}
            >
              {icon}
            </button>
          ))}
        </div>
        {toggles}
      </header>
      {tab === "files" ? (
        <FileTree
          key={viewId}
          viewId={viewId}
          root={root}
          connection={connection}
          onOpenFile={onOpenFile}
        />
      ) : (
        <div className="sidebar-body" role="tabpanel" aria-label="Changes" />
      )}
    </div>
  );
}
