import { useEffect, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { useTimelineStore } from "../timeline/store";

type VeditCommitEntry = {
  commitHash: string;
  timelineHash: string;
  timestamp: string;
  header: string;
  fullMessage: string;
  parents: string[];
};

type VeditDiffResponse = {
  fromRef: string;
  toRef: string;
  changeCount: number;
  changedClipCount: number;
  changedClipIds: string[];
  changes: unknown;
  animationChanges: unknown;
};

export function VeditPanel() {
  const [entries, setEntries] = useState<VeditCommitEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [diffs, setDiffs] = useState<Record<string, VeditDiffResponse>>({});
  const [diffLoading, setDiffLoading] = useState<string | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<string | null>(null);
  const [restoreBusy, setRestoreBusy] = useState<string | null>(null);
  const refreshTimeline = useTimelineStore((s) => s.refresh);

  function load() {
    let cancelled = false;
    setLoading(true);
    if (!isTauri()) {
      setEntries([]);
      setError(null);
      setLoading(false);
      return () => {
        cancelled = true;
      };
    }
    invoke<VeditCommitEntry[]>("list_vedit_commits", { limit: 30 })
      .then((next) => {
        if (!cancelled) {
          setEntries(next);
          setError(null);
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }

  useEffect(() => {
    return load();
  }, []);

  const latest = entries[0] ?? null;

  async function inspectDiff(entry: VeditCommitEntry) {
    setDiffLoading(entry.commitHash);
    try {
      const diff = await invoke<VeditDiffResponse>("diff_vedit_refs", {
        fromRef: entry.parents[0] ?? null,
        toRef: entry.commitHash,
      });
      setDiffs((prev) => ({ ...prev, [entry.commitHash]: diff }));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setDiffLoading(null);
    }
  }

  async function restore(entry: VeditCommitEntry) {
    setRestoreBusy(entry.commitHash);
    try {
      await invoke("restore_vedit_ref", { refstr: entry.commitHash });
      setRestoreTarget(null);
      await refreshTimeline();
      load();
    } catch (e) {
      setError(String(e));
    } finally {
      setRestoreBusy(null);
    }
  }

  return (
    <div className="sidebar-pane vedit-panel">
      <header className="vedit-header">
        <div>
          <span className="vedit-kicker">Edits</span>
          <strong>Timeline history</strong>
        </div>
        <span className="vedit-count">
          {loading ? "Loading" : `${entries.length} saved`}
        </span>
        <button type="button" className="glass-ghost px-2 py-1 text-[11px] font-semibold" onClick={load}>
          Refresh
        </button>
      </header>
      {latest && (
        <section className="vedit-current">
          <span className="vedit-kicker">Current cut</span>
          <strong>{latest.header || "Untitled edit"}</strong>
          <div className="vedit-current-meta">
            <code>{shortHash(latest.commitHash)}</code>
            <span>{formatTimestamp(latest.timestamp)}</span>
          </div>
        </section>
      )}
      {loading && <p className="chat-empty chat-empty-loaded">Loading history...</p>}
      {error && <p className="item item-error">{error}</p>}
      {!loading && !error && entries.length === 0 && (
        <p className="chat-empty chat-empty-loaded">
          No vedit commits yet. Accepted edits will appear here.
        </p>
      )}
      <div className="vedit-list">
        {entries.map((entry) => (
          <details key={entry.commitHash} className="vedit-entry">
            <summary>
              <span className="vedit-entry-title">
                {entry.header || "Untitled edit"}
              </span>
              <span className="vedit-entry-meta">
                <span>{formatCompactTimestamp(entry.timestamp)}</span>
                <code>{shortHash(entry.commitHash)}</code>
              </span>
            </summary>
            <div className="vedit-entry-body">
              <dl>
                <dt>Saved</dt>
                <dd>{formatTimestamp(entry.timestamp)}</dd>
                <dt>Timeline</dt>
                <dd>
                  <code>{shortHash(entry.timelineHash)}</code>
                </dd>
                {entry.parents.length > 0 && (
                  <>
                    <dt>Parent</dt>
                    <dd>{entry.parents.map(shortHash).join(", ")}</dd>
                  </>
                )}
              </dl>
              {entry.fullMessage && <pre>{entry.fullMessage}</pre>}
              <div className="vedit-entry-actions">
                <button
                  type="button"
                  onClick={() => inspectDiff(entry)}
                  disabled={diffLoading === entry.commitHash}
                >
                  {diffLoading === entry.commitHash ? "Inspecting..." : "Inspect diff"}
                </button>
                {latest?.commitHash !== entry.commitHash && (
                  <button
                    type="button"
                    className="vedit-restore-button"
                    onClick={() =>
                      setRestoreTarget((current) =>
                        current === entry.commitHash ? null : entry.commitHash,
                      )
                    }
                    disabled={restoreBusy !== null}
                  >
                    Restore this cut
                  </button>
                )}
              </div>
              {diffs[entry.commitHash] && (
                <section className="vedit-diff">
                  <div className="vedit-diff-meta">
                    <span>{diffs[entry.commitHash].changeCount} changes</span>
                    <code>
                      {shortHash(diffs[entry.commitHash].fromRef)}...{shortHash(diffs[entry.commitHash].toRef)}
                    </code>
                  </div>
                  <DiffPreview changes={diffs[entry.commitHash].changes} />
                </section>
              )}
              {restoreTarget === entry.commitHash && (
                <section className="vedit-restore-confirm">
                  <strong>Restore this timeline state?</strong>
                  <p>
                    This writes the selected timeline to the project and records a new history commit.
                  </p>
                  <div>
                    <button
                      type="button"
                      className="vedit-restore-button"
                      onClick={() => restore(entry)}
                      disabled={restoreBusy !== null}
                    >
                      {restoreBusy === entry.commitHash ? "Restoring..." : "Confirm restore"}
                    </button>
                    <button
                      type="button"
                      onClick={() => setRestoreTarget(null)}
                      disabled={restoreBusy !== null}
                    >
                      Cancel
                    </button>
                  </div>
                </section>
              )}
            </div>
          </details>
        ))}
      </div>
    </div>
  );
}

function DiffPreview({ changes }: { changes: unknown }) {
  const list = Array.isArray(changes) ? changes : null;
  if (!list) {
    return <pre>{JSON.stringify(changes, null, 2)}</pre>;
  }
  return (
    <div className="vedit-diff-list">
      {list.slice(0, 12).map((change, index) => {
        const record =
          change !== null && typeof change === "object"
            ? (change as Record<string, unknown>)
            : null;
        const kind = typeof record?.kind === "string" ? record.kind : "change";
        const path =
          typeof record?.path === "string"
            ? record.path
            : typeof record?.clip_uuid === "string"
            ? record.clip_uuid
            : `entry ${index + 1}`;
        return (
          <div key={index} className="vedit-diff-row">
            <span>{kind.replace(/_/g, " ")}</span>
            <code>{path}</code>
          </div>
        );
      })}
      {list.length > 12 && (
        <div className="vedit-diff-row vedit-diff-row-muted">
          <span>additional</span>
          <code>{list.length - 12} more</code>
        </div>
      )}
      <details className="vedit-diff-json">
        <summary>Raw JSON</summary>
        <pre>{JSON.stringify(changes, null, 2)}</pre>
      </details>
    </div>
  );
}

function shortHash(hash: string): string {
  return hash.replace(/^sha256:/, "").slice(0, 8);
}

function formatTimestamp(timestamp: string): string {
  const date = new Date(timestamp);
  return Number.isNaN(date.getTime()) ? timestamp : date.toLocaleString();
}

function formatCompactTimestamp(timestamp: string): string {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return timestamp;
  return date.toLocaleTimeString([], {
    hour: "numeric",
    minute: "2-digit",
  });
}
