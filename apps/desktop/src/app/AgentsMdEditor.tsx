// AgentsMdEditor — modal mono-text editor for the project's
// AGENTS.md (Wave 3 T2).
//
// AGENTS.md is the editorial brief the agent reads every turn. Round-
// tripping changes through an external editor is slow; this modal
// closes the loop — edit, save, agent picks up changes on the next
// turn.
//
// Reachable from:
//   - Settings → Project section ("Edit AGENTS.md" button).
//   - Future: the Brief surface (B1) will link here via the same
//     `useAgentsMdEditor.open()` entrypoint. The modal does not
//     care which caller opened it.
//
// Scope discipline:
//   - No markdown rendering. Plain `<textarea>` with mono font is
//     deliberate — a real markdown editor is a follow-up.
//   - No new npm deps.
//   - Tokens-only styling. Modal chrome reuses `.modal-*`; the
//     editor-specific bits are scoped under `.agents-md-editor` so
//     they don't leak into other modal callers.
//
// Save flow: async. Save handler calls `write_agents_md` Tauri
// command, on success updates `lastSavedAt` and flips the dirty
// flag clean. On failure surfaces an inline error chip. ⌘S /
// ⌘Enter trigger the same handler as the Save button.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useProjectStore } from "./state";
import { useAgentsMdEditor } from "../state/agentsMdEditor";
import { Button, Inline } from "../ui";

/** Starter template shown when the project has no AGENTS.md yet.
 *  Saving writes this (or whatever the user edits it into) to disk.
 *  Kept minimal so the user isn't overwhelmed — the existing project
 *  starter template lives in `commands/project.rs` and is what
 *  `init_project` writes; this in-product template is for the
 *  AGENTS.md-deleted case. */
const STARTER_TEMPLATE = `# AGENTS.md

## Editorial intent
<!-- What kind of edit are you making? Tone, length, must-include themes. -->

## Constraints
<!-- Hard rules. -->

## Voice
<!-- How should the agent talk to you in proposals? -->
`;

type SaveStatus =
  | { kind: "idle" }
  | { kind: "saving" }
  | { kind: "saved"; at: number }
  | { kind: "error"; message: string };

export function AgentsMdEditor() {
  const isOpen = useAgentsMdEditor((s) => s.isOpen);
  const isDirty = useAgentsMdEditor((s) => s.isDirty);
  const close = useAgentsMdEditor((s) => s.close);
  const markDirty = useAgentsMdEditor((s) => s.markDirty);
  const markClean = useAgentsMdEditor((s) => s.markClean);
  const projectPath = useProjectStore((s) => s.current);

  const [content, setContent] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>({ kind: "idle" });
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // Load the file when the modal opens. If the backend returns null
  // (file missing), seed the textarea with the starter template — the
  // first Save then creates the file.
  useEffect(() => {
    if (!isOpen || !projectPath) {
      return;
    }
    let cancelled = false;
    setLoaded(false);
    setSaveStatus({ kind: "idle" });

    if (!isTauri()) {
      // Storybook / vite-preview mode — start with the template so
      // the editor remains usable for design review.
      setContent(STARTER_TEMPLATE);
      setLoaded(true);
      return;
    }

    invoke<string | null>("read_agents_md", { projectPath })
      .then((existing) => {
        if (cancelled) return;
        setContent(existing ?? STARTER_TEMPLATE);
        setLoaded(true);
      })
      .catch((e) => {
        if (cancelled) return;
        console.warn("read_agents_md failed", e);
        setContent(STARTER_TEMPLATE);
        setLoaded(true);
        setSaveStatus({ kind: "error", message: String(e) });
      });

    return () => {
      cancelled = true;
    };
  }, [isOpen, projectPath]);

  // Cancel handler — confirms before discarding unsaved edits.
  const cancel = useCallback(() => {
    if (
      isDirty &&
      !window.confirm("Discard unsaved changes to AGENTS.md?")
    ) {
      return;
    }
    close();
  }, [isDirty, close]);

  // Save handler — shared between the button, ⌘S, and ⌘Enter.
  const save = useCallback(async () => {
    if (!projectPath) return;
    if (!isDirty) return;
    if (!isTauri()) {
      // No backend in design review — pretend it succeeded so the
      // UX still demos.
      markClean();
      setSaveStatus({ kind: "saved", at: Date.now() });
      return;
    }
    setSaveStatus({ kind: "saving" });
    try {
      await invoke<void>("write_agents_md", {
        projectPath,
        content,
      });
      markClean();
      setSaveStatus({ kind: "saved", at: Date.now() });
    } catch (e) {
      setSaveStatus({ kind: "error", message: String(e) });
    }
  }, [projectPath, isDirty, content, markClean]);

  // Esc / ⌘S / ⌘Enter handlers — registered at document level so
  // they win regardless of which child has focus.
  useEffect(() => {
    if (!isOpen) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        cancel();
        return;
      }
      const meta = event.metaKey || event.ctrlKey;
      if (!meta) return;
      // ⌘S — save
      if (event.key === "s" || event.key === "S") {
        event.preventDefault();
        void save();
        return;
      }
      // ⌘Enter — save (parity with composer convention)
      if (event.key === "Enter") {
        event.preventDefault();
        void save();
        return;
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [isOpen, cancel, save]);

  // Focus the textarea on first open so typing is immediate.
  useEffect(() => {
    if (isOpen && loaded) {
      textareaRef.current?.focus();
    }
  }, [isOpen, loaded]);

  const wordCount = useMemo(() => countWords(content), [content]);
  const savedLabel = useMemo(() => formatSavedLabel(saveStatus), [saveStatus]);

  if (!isOpen) return null;

  return (
    <div
      className="modal-backdrop"
      onClick={cancel}
      role="presentation"
    >
      <div
        className="modal agents-md-editor"
        onClick={(event) => event.stopPropagation()}
        style={{
          width: "min(720px, calc(100vw - 48px))",
          height: "min(520px, calc(100vh - 48px))",
        }}
        role="dialog"
        aria-modal="true"
        aria-label="Edit AGENTS.md"
      >
        <header className="modal-header">
          <Inline gap="2" align="center">
            <h2>AGENTS.md</h2>
            {projectPath ? (
              <span className="ame-path" title={projectPath}>
                {projectPath}
              </span>
            ) : null}
          </Inline>
          <button
            type="button"
            className="modal-close"
            onClick={cancel}
            aria-label="Close AGENTS.md editor"
          >
            ×
          </button>
        </header>
        <div className="modal-body ame-body">
          <textarea
            ref={textareaRef}
            className="ame-textarea"
            value={content}
            onChange={(event) => {
              setContent(event.target.value);
              markDirty();
              // A fresh edit invalidates the "Saved" chip — without
              // this the user sees "Saved 2s ago" while typing into
              // a now-dirty buffer.
              if (saveStatus.kind === "saved" || saveStatus.kind === "error") {
                setSaveStatus({ kind: "idle" });
              }
            }}
            spellCheck={false}
            aria-label="AGENTS.md content"
            placeholder={loaded ? "" : "Loading…"}
            disabled={!loaded || !projectPath}
          />
        </div>
        <footer className="modal-footer ame-footer">
          <Inline gap="3" align="center" className="ame-meta">
            <span className="ame-wordcount">
              {wordCount} word{wordCount === 1 ? "" : "s"}
            </span>
            <SaveChip status={saveStatus} savedLabel={savedLabel} />
          </Inline>
          <Inline gap="2" align="center">
            <Button variant="ghost" size="sm" onClick={cancel}>
              Cancel
            </Button>
            <Button
              variant="primary"
              size="sm"
              onClick={() => void save()}
              disabled={
                !isDirty ||
                !projectPath ||
                saveStatus.kind === "saving"
              }
              title={
                !projectPath
                  ? "No project loaded"
                  : !isDirty
                    ? "No changes to save"
                    : "Save (⌘S)"
              }
            >
              {saveStatus.kind === "saving" ? "Saving…" : "Save"}
            </Button>
          </Inline>
        </footer>
      </div>
    </div>
  );
}

function SaveChip({
  status,
  savedLabel,
}: {
  status: SaveStatus;
  savedLabel: string;
}) {
  if (status.kind === "idle") return null;
  if (status.kind === "saving") {
    return <span className="ame-chip ame-chip--info">Saving…</span>;
  }
  if (status.kind === "saved") {
    return <span className="ame-chip ame-chip--ok">{savedLabel}</span>;
  }
  return (
    <span className="ame-chip ame-chip--err" title={status.message}>
      Save failed
    </span>
  );
}

function countWords(text: string): number {
  const trimmed = text.trim();
  if (trimmed === "") return 0;
  return trimmed.split(/\s+/).length;
}

function formatSavedLabel(status: SaveStatus): string {
  if (status.kind !== "saved") return "";
  const seconds = Math.max(0, Math.floor((Date.now() - status.at) / 1000));
  if (seconds < 5) return "Saved just now";
  if (seconds < 60) return `Saved ${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  return `Saved ${minutes}m ago`;
}
