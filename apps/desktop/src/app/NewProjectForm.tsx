// Modal form for creating a new project.
//
// Step 1.3 (Phase 1) added a project-type picker — Podcast / Shorts /
// Tutorial / Other. The choice routes through init_project, gets
// stamped into the OTIO timeline's metadata.awidat slot, and drives
// the agent's per-format system prompt + editorial defaults.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { ProjectType } from "../protocol";

type Props = {
  /** Called on cancel or successful create — caller decides what to do. */
  onClose: () => void;
  /** Called with the new project path once init_project succeeds. */
  onCreated: (path: string) => void;
};

/** Discriminated picker value. The "other" branch carries the
 *  free-text description; the typed enums don't. */
type PickerKind = "podcast" | "shorts" | "tutorial" | "other";

export function NewProjectForm({ onClose, onCreated }: Props) {
  const [parent, setParent] = useState("");
  const [name, setName] = useState("");
  const [kind, setKind] = useState<PickerKind>("podcast");
  const [otherDescription, setOtherDescription] = useState("");
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
    if (kind === "other" && otherDescription.trim().length === 0) {
      setError("Other projects need a short description");
      return;
    }
    setError(null);
    setBusy(true);
    try {
      const projectType: ProjectType =
        kind === "other"
          ? { kind: "other", description: otherDescription.trim() }
          : { kind };
      const path = await invoke<string>("init_project", {
        parentDir: parent.trim(),
        name: name.trim(),
        projectType,
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
          <label className="field">
            <span>Project type</span>
            <select
              value={kind}
              onChange={(e) => setKind(e.target.value as PickerKind)}
              disabled={busy}
              className="properties-select"
            >
              <option value="podcast">Podcast (long-form cleanup)</option>
              <option value="shorts">Shorts (vertical, 60s)</option>
              <option value="tutorial">Tutorial / demo</option>
              <option value="other">Other (custom)</option>
            </select>
          </label>
          {kind === "other" && (
            <label className="field">
              <span>Describe your project</span>
              <input
                type="text"
                value={otherDescription}
                onChange={(e) => setOtherDescription(e.target.value)}
                placeholder="e.g. compilation of my best clips from 5 source videos"
                disabled={busy}
              />
            </label>
          )}
          <p className="field-hint">
            Awidat will create <code>{parent || "<parent>"}/{name || "<name>"}</code>{" "}
            with an empty timeline and a starter <code>AGENTS.md</code>. The
            project type drives the agent's editorial defaults — podcast mode
            trims silences and respects breath beats; other modes fall back to
            neutral cleanup until your description tells the agent what to do.
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
