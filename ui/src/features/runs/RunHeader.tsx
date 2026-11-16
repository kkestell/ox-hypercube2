import { GitBranch, History, Hourglass, Workflow } from "lucide-react";
import { useEffect, useState } from "react";
import type { RunInfo, WorktreeInfo } from "../../lib/protocol";
import { branchStatus } from "../sessions/localMerge";

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** A span of milliseconds as `42s`, `3m 12s` or `1h 4m`. */
function elapsed(ms: number) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

/** A past time as `12 minutes ago`, `yesterday` or `3 days ago`. */
function ago(time: number, now: number) {
  const seconds = Math.round((time - now) / 1000);
  const units = [
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ] as const;
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) {
      return relative.format(Math.round(seconds / size), unit);
    }
  }
  return relative.format(seconds, "second");
}

/** The current time, updated each second while `live` is set. */
function useNow(live: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);
  return now;
}

/**
 * A run's workflow and state, when it started and how long it has run, and
 * its worktree's branch and comparison with its base ref.
 */
export function RunHeader({ run, worktree }: { run: RunInfo; worktree?: WorktreeInfo }) {
  const running = run.state === "running";
  const now = useNow(running);
  const started = new Date(run.created_at).toLocaleString();
  const branch =
    worktree?.compare != null && worktree.comparison != null
      ? branchStatus(worktree.compare, worktree.comparison)
      : null;

  return (
    <header className="run-header">
      <dl className="run-header-meta">
        <div title={started}>
          {running ? <Hourglass /> : <History />}
          <span className={running ? "run-header-elapsed" : undefined}>
            {running ? elapsed(now - run.created_at) : ago(run.created_at, now)}
          </span>
        </div>
        {worktree && (
          <div>
            <GitBranch />
            <span className="run-header-branch">{worktree.branch}</span>
            {branch && (
              <span className={branch.attention ? "attention" : undefined}>{branch.tooltip}</span>
            )}
          </div>
        )}
        <div className="run-header-workflow">
          <Workflow />
          <span>{run.name}</span>
        </div>
      </dl>
    </header>
  );
}
