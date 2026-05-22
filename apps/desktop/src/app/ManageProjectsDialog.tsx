// Manage projects dialog — separates "delete from disk" mental mode
// from the recents popover's "switch to a project" mental mode. Lists
// every recent with its on-disk size and a per-row delete button.
// Routes through DeleteProjectConfirm so the destructive logic only
// has one implementation.

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Trash2 } from "lucide-react";
import { useProjectStore } from "./state";
import { DeleteProjectConfirm } from "./DeleteProjectConfirm";

type Props = {
  /** Called when the user closes the dialog. */
  onClose: () => void;
  /** Called when the currently-open project gets deleted from in here.
   *  Lets the app shell clear its `current` and route back to the picker. */
  onDeletedActive: () => void;
};

export function ManageProjectsDialog({ onClose, onDeletedActive }: Props) {
  const recent = useProjectStore((s) => s.recent);
  const current = useProjectStore((s) => s.current);
  const refresh = useProjectStore((s) => s.refresh);
  const [sizes, setSizes] = useState<Record<string, number>>({});
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  // Fan out size lookups in parallel — each is a recursive walk that
  // can take a few hundred ms for a project with thousands of files,
  // so we don't want to chain them. A failed walk leaves the entry
  // out of `sizes` and the row just omits the size cell.
  useEffect(() => {
    let cancelled = false;
    for (const path of recent) {
      invoke<number>("project_size_bytes", { path })
        .then((bytes) => {
          if (cancelled) return;
          setSizes((prev) => ({ ...prev, [path]: bytes }));
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
    };
  }, [recent]);

  return (
    <>
      <div className="modal-backdrop" onClick={onClose}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <header className="modal-header">
            <h2>Manage projects</h2>
            <button className="modal-close" onClick={onClose} aria-label="Close">
              ×
            </button>
          </header>
          <div className="modal-body manage-projects-body">
            {recent.length === 0 ? (
              <p className="field-hint">No recent projects to manage.</p>
            ) : (
              <ul className="manage-projects-list">
                {recent.map((p) => (
                  <li key={p} className="manage-projects-row">
                    <div className="manage-projects-text">
                      <div className="manage-projects-name-row">
                        <strong>{basename(p)}</strong>
                        {p === current && (
                          <span className="manage-projects-active">Open</span>
                        )}
                      </div>
                      <code className="recent-path">{p}</code>
                      <span className="popover-hint">
                        {sizes[p] !== undefined
                          ? formatBytes(sizes[p])
                          : "Measuring…"}
                      </span>
                    </div>
                    <button
                      className="properties-danger"
                      onClick={() => setPendingDelete(p)}
                      title={`Delete "${basename(p)}" permanently`}
                    >
                      <Trash2 size={12} strokeWidth={1.75} />
                      <span>Delete</span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <footer className="modal-footer">
            <button onClick={onClose}>Done</button>
          </footer>
        </div>
      </div>
      {pendingDelete && (
        <DeleteProjectConfirm
          path={pendingDelete}
          isActive={pendingDelete === current}
          onCancel={() => setPendingDelete(null)}
          onDeleted={() => {
            const wasActive = pendingDelete === current;
            setPendingDelete(null);
            refresh().catch(() => {});
            if (wasActive) onDeletedActive();
          }}
        />
      )}
    </>
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
