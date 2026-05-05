// Modal form for creating a new project. Step 2 only does
// Project::init + starter AWIDAT.md — asset import + indexing land
// in the next commit and will be reachable from the empty-state
// chat ("Drop media to get started…") rather than from this form.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

type Props = {
  /** Called on cancel or successful create — caller decides what to do. */
  onClose: () => void;
  /** Called with the new project path once init_project succeeds. */
  onCreated: (path: string) => void;
};

export function NewProjectForm({ onClose, onCreated }: Props) {
  const [parent, setParent] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function pickParent() {
    setError(null);
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Choose folder to create project in",
      });
      if (typeof picked === "string") {
        setParent(picked);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function create() {
    if (!parent.trim() || !name.trim()) return;
    setError(null);
    setBusy(true);
    try {
      const path = await invoke<string>("init_project", {
        parentDir: parent.trim(),
        name: name.trim(),
      });
      onCreated(path);
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <header className="modal-header">
          <h2>New project</h2>
          <button className="modal-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </header>
        <div className="modal-body">
          <label className="field">
            <span>Parent folder</span>
            <div className="field-row">
              <input
                type="text"
                value={parent}
                onChange={(e) => setParent(e.target.value)}
                placeholder="/Users/you/projects"
                disabled={busy}
              />
              <button onClick={pickParent} disabled={busy}>
                Choose…
              </button>
            </div>
          </label>
          <label className="field">
            <span>Project name</span>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-podcast-ep1"
              disabled={busy}
            />
          </label>
          <p className="field-hint">
            Awidat will create <code>{parent || "<parent>"}/{name || "<name>"}</code>{" "}
            with an empty timeline and a starter <code>AWIDAT.md</code>. You can
            drop source media into <code>raw/</code> after creation, or use the
            chat to import + index from a URL.
          </p>
          {error && <div className="field-error">{error}</div>}
        </div>
        <footer className="modal-footer">
          <button onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="primary"
            onClick={create}
            disabled={busy || !parent.trim() || !name.trim()}
          >
            {busy ? "Creating…" : "Create"}
          </button>
        </footer>
      </div>
    </div>
  );
}
