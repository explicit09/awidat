// Modal form for creating a new project.
//
// The picker offers four project shapes — podcast / shorts / tutorial /
// other — and routes the choice through `init_project`, where it gets
// stamped into the OTIO timeline's metadata.awidat slot and drives the
// agent's per-format system prompt + editorial defaults.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { ComponentType, KeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  BookOpen,
  ChevronRight,
  Loader2,
  Mic,
  Sparkles,
  Video,
} from "lucide-react";
import type { LucideProps } from "lucide-react";
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

type TypeOption = {
  kind: PickerKind;
  label: string;
  hint: string;
  Icon: ComponentType<LucideProps>;
};

/** Card metadata for the type picker. Order matters — these render as a
 *  2-column grid, so the most common shape (podcast) lands top-left. */
const TYPE_OPTIONS: readonly TypeOption[] = [
  { kind: "podcast", label: "Podcast", hint: "Long-form cleanup", Icon: Mic },
  { kind: "shorts", label: "Shorts", hint: "Vertical, ~60s", Icon: Video },
  { kind: "tutorial", label: "Tutorial", hint: "Walkthrough / demo", Icon: BookOpen },
  { kind: "other", label: "Other", hint: "Describe your own", Icon: Sparkles },
];

/** Join a parent dir + name without doubling slashes — purely for the
 *  preview row; the real path comes back from the backend. */
function previewPath(parent: string, name: string): string {
  const trimmedParent = parent.trim().replace(/[\\/]+$/, "");
  const trimmedName = name.trim();
  if (!trimmedParent && !trimmedName) return "/Users/you/projects/my-project";
  if (!trimmedParent) return `…/${trimmedName || "my-project"}`;
  if (!trimmedName) return `${trimmedParent}/…`;
  return `${trimmedParent}/${trimmedName}`;
}

export function NewProjectForm({ onClose, onCreated }: Props) {
  const [parent, setParent] = useState("");
  const [name, setName] = useState("");
  const [kind, setKind] = useState<PickerKind>("podcast");
  const [otherDescription, setOtherDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const canSubmit = useMemo(() => {
    if (busy) return false;
    if (!parent.trim() || !name.trim()) return false;
    if (kind === "other" && !otherDescription.trim()) return false;
    return true;
  }, [busy, parent, name, kind, otherDescription]);

  const pickParent = useCallback(async () => {
    setError(null);
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Choose folder to create project in",
      });
      if (typeof picked === "string") setParent(picked);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const create = useCallback(async () => {
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
  }, [parent, name, kind, otherDescription, onCreated]);

  // Esc to cancel, ⌘/Ctrl+Enter to submit. Scoped to the modal so we
  // don't fight global keymaps when the user is typing in inputs.
  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (!busy) onClose();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
        e.preventDefault();
        if (canSubmit) void create();
      }
    },
    [busy, canSubmit, create, onClose],
  );

  // Autofocus the name field on open — parent is usually picked, name
  // is what the user actually types.
  useEffect(() => {
    const el = document.getElementById("np-name-input");
    if (el instanceof HTMLInputElement) el.focus();
  }, []);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal new-project"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <header className="modal-header">
          <div className="np-title">
            <h2>New project</h2>
            <kbd className="np-kbd">⌘N</kbd>
          </div>
          <button className="modal-close" onClick={onClose} aria-label="Close">
            ×
          </button>
        </header>
        <div className="modal-body">
          <label className="field">
            <span>Parent folder</span>
            <div className="field-row np-row">
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
              id="np-name-input"
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="my-podcast-ep1"
              disabled={busy}
              autoComplete="off"
              spellCheck={false}
            />
            <span className="np-input-hint">lowercase, dashes ok</span>
          </label>
          <fieldset className="field np-types" disabled={busy}>
            <span>Project type</span>
            <div className="np-type-grid" role="radiogroup" aria-label="Project type">
              {TYPE_OPTIONS.map((opt) => (
                <TypeCard
                  key={opt.kind}
                  option={opt}
                  selected={kind === opt.kind}
                  onSelect={() => setKind(opt.kind)}
                />
              ))}
            </div>
          </fieldset>
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
          <p className="field-hint np-helper">
            Creates an empty timeline + starter <code>AGENTS.md</code>. The
            project type sets the agent's editorial defaults; you can refine in
            the project's <code>AGENTS.md</code> anytime.
          </p>
          <div className="np-preview" aria-live="polite">
            <ChevronRight size={12} strokeWidth={2} />
            <span>{previewPath(parent, name)}</span>
          </div>
          {error && <div className="field-error">{error}</div>}
        </div>
        <footer className="modal-footer np-footer">
          <button onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="primary np-create"
            onClick={create}
            disabled={!canSubmit}
          >
            {busy ? (
              <>
                <Loader2 size={13} strokeWidth={2.4} className="np-spin" />
                Creating…
              </>
            ) : (
              <>
                Create
                <kbd className="np-kbd np-kbd-primary">⌘↵</kbd>
              </>
            )}
          </button>
        </footer>
      </div>
    </div>
  );
}

/** Single card in the type picker. Rendered as a radio so screen readers
 *  see the picker as a group of options, not buttons. */
function TypeCard({
  option,
  selected,
  onSelect,
}: {
  option: TypeOption;
  selected: boolean;
  onSelect: () => void;
}) {
  const { label, hint, Icon } = option;
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={`np-type-card${selected ? " np-type-card-selected" : ""}`}
    >
      <Icon size={16} strokeWidth={1.8} className="np-type-icon" />
      <div className="np-type-text">
        <div className="np-type-label">{label}</div>
        <div className="np-type-hint">{hint}</div>
      </div>
    </button>
  );
}
