// Confirm modal for permanent project deletion. Shared between the
// inline trash icon in the recents popover and the Manage projects
// dialog — both route here so the destructive copy and the basename
// guard only have one source of truth.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Props = {
  /** Absolute path to the project folder being deleted. */
  path: string;
  /** True when this is the project currently loaded in the app shell. */
  isActive: boolean;
  /** Called after the backend confirms the delete succeeded. */
  onDeleted: () => void;
  /** Called when the user cancels — no backend call is made. */
  onCancel: () => void;
};

export function DeleteProjectConfirm({
  path,
  isActive,
  onDeleted,
  onCancel,
}: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sizeBytes, setSizeBytes] = useState<number | null>(null);
  const name = basename(path);

  // Lazy size lookup so the modal opens instantly; the "freeing X" line
  // appears a beat later. Best-effort: a failed walk leaves sizeBytes
  // null and the modal just omits the freed-space hint.
  useEffect(() => {
    let cancelled = false;
    invoke<number>("project_size_bytes", { path })
      .then((b) => {
        if (!cancelled) setSizeBytes(b);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [path]);

  async function performDelete() {
    setBusy(true);
    setError(null);
    try {
      await invoke("delete_project", {
        path,
        expectedBasename: name,
      });
      onDeleted();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={busy ? undefined : onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <header className="modal-header">
          <h2>Delete &ldquo;{name}&rdquo; permanently?</h2>
          <button
            className="modal-close"
            onClick={onCancel}
            disabled={busy}
            aria-label="Close"
          >
            ×
          </button>
        </header>
        <div className="modal-body">
          <p className="field-hint">
            This removes the entire project folder, including raw media,
            proxies, and edit history. <strong>This cannot be undone.</strong>
          </p>
          <div className="field">
            <span>Path</span>
            <code className="recent-path">{path}</code>
          </div>
          {sizeBytes !== null && (
            <p className="field-hint">
              Frees about <strong>{formatBytes(sizeBytes)}</strong> of disk
              space.
            </p>
          )}
          {isActive && (
            <p className="field-hint">
              This project is currently open. It will be closed before the
              folder is removed.
            </p>
          )}
          {error && <div className="field-error">{error}</div>}
        </div>
        <footer className="modal-footer">
          <button onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button
            className="properties-danger"
            onClick={performDelete}
            disabled={busy}
          >
            {busy ? "Deleting…" : "Delete permanently"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}
