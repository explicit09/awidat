import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type VeditCommitEntry = {
  commitHash: string;
  timelineHash: string;
  timestamp: string;
  header: string;
  fullMessage: string;
  parents: string[];
};

export function VeditPanel() {
  const [entries, setEntries] = useState<VeditCommitEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  function load() {
    let cancelled = false;
    setLoading(true);
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
        <button type="button" onClick={load}>
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
            </div>
          </details>
        ))}
      </div>
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
