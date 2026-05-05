// Top-of-window project bar. Shows the active project path and a
// "Change" affordance that opens a popover with: native folder picker,
// "New project…" button, and a recents list.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useProjectStore } from "./state";
import { NewProjectForm } from "./NewProjectForm";

type Props = {
  /** Called whenever the active project changes (open/new/clear). */
  onChange: (path: string | null) => void;
};

export function ProjectBanner({ onChange }: Props) {
  const current = useProjectStore((s) => s.current);
  const recent = useProjectStore((s) => s.recent);
  const refresh = useProjectStore((s) => s.refresh);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showNew, setShowNew] = useState(false);
  const [showUrl, setShowUrl] = useState(false);
  const [urlInput, setUrlInput] = useState("");
  const popRef = useRef<HTMLDivElement | null>(null);

  // Initial load + close popover on outside click.
  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
    function onClick(e: MouseEvent) {
      if (popRef.current && !popRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [refresh]);

  async function pickAndOpen() {
    setError(null);
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Open Awidat project",
      });
      if (typeof picked === "string") {
        await invoke("set_project_root", { path: picked });
        await refresh();
        onChange(picked);
        setOpen(false);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function openRecent(path: string) {
    setError(null);
    try {
      await invoke("set_project_root", { path });
      await refresh();
      onChange(path);
      setOpen(false);
    } catch (e) {
      setError(String(e));
    }
  }

  async function importLocal() {
    setError(null);
    try {
      const picked = await openDialog({
        directory: false,
        multiple: false,
        title: "Choose media file to import",
      });
      if (typeof picked === "string") {
        await invoke("import_local", { srcPath: picked, link: false });
        setOpen(false);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function importUrl() {
    const u = urlInput.trim();
    if (!u) return;
    setError(null);
    try {
      setUrlInput("");
      setShowUrl(false);
      await invoke("import_url", { url: u });
    } catch (e) {
      setError(String(e));
    }
  }

  async function runIndex() {
    setError(null);
    try {
      await invoke("index_project");
      setOpen(false);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="project-banner" ref={popRef}>
      <span className="project-label">project:</span>
      <code className="project-path">
        {current ?? "(none — open or create one to start)"}
      </code>
      <button onClick={() => setOpen((v) => !v)}>
        {current ? "Change" : "Open…"}
      </button>
      {open && (
        <div className="project-popover" role="menu">
          <button className="popover-action" onClick={pickAndOpen}>
            <strong>Open existing project…</strong>
            <span className="popover-hint">pick a folder</span>
          </button>
          <button
            className="popover-action"
            onClick={() => {
              setShowNew(true);
              setOpen(false);
            }}
          >
            <strong>New project…</strong>
            <span className="popover-hint">init in a chosen folder</span>
          </button>
          {current && (
            <>
              <div className="popover-divider">Current project</div>
              <button className="popover-action" onClick={importLocal}>
                <strong>Import media file…</strong>
                <span className="popover-hint">copy/symlink into raw/</span>
              </button>
              <button
                className="popover-action"
                onClick={() => {
                  setShowUrl(true);
                  setOpen(false);
                }}
              >
                <strong>Import from URL…</strong>
                <span className="popover-hint">yt-dlp into raw/</span>
              </button>
              <button className="popover-action" onClick={runIndex}>
                <strong>Run indexers</strong>
                <span className="popover-hint">whisper, scenes, audio…</span>
              </button>
            </>
          )}
          {recent.length > 0 && (
            <>
              <div className="popover-divider">Recent</div>
              {recent.map((p) => (
                <button
                  key={p}
                  className="popover-recent"
                  onClick={() => openRecent(p)}
                  title={p}
                >
                  <span className="recent-name">{basename(p)}</span>
                  <span className="recent-path">{p}</span>
                </button>
              ))}
            </>
          )}
          {error && <div className="popover-error">{error}</div>}
        </div>
      )}
      {showNew && (
        <NewProjectForm
          onClose={() => setShowNew(false)}
          onCreated={(path) => {
            setShowNew(false);
            onChange(path);
            refresh().catch(() => {});
          }}
        />
      )}
      {showUrl && (
        <div className="modal-backdrop" onClick={() => setShowUrl(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <header className="modal-header">
              <h2>Import from URL</h2>
              <button
                className="modal-close"
                onClick={() => setShowUrl(false)}
                aria-label="Close"
              >
                ×
              </button>
            </header>
            <div className="modal-body">
              <label className="field">
                <span>URL</span>
                <input
                  type="text"
                  value={urlInput}
                  onChange={(e) => setUrlInput(e.target.value)}
                  placeholder="https://www.youtube.com/watch?v=…"
                  autoFocus
                />
              </label>
              <p className="field-hint">
                Awidat downloads via yt-dlp at up to 1080p mp4. The download
                runs in the background; cancel from the chat any time.
              </p>
            </div>
            <footer className="modal-footer">
              <button onClick={() => setShowUrl(false)}>Cancel</button>
              <button
                className="primary"
                onClick={importUrl}
                disabled={!urlInput.trim()}
              >
                Download
              </button>
            </footer>
          </div>
        </div>
      )}
    </div>
  );
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}
