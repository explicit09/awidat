// Sticky toolbar between the header and the chat items. Always
// visible when a project is loaded — replaces the implicit-popover
// pattern from the v0 ProjectBanner where actions hid behind "Change."
//
// Three primary actions: Import file, Import URL, Run indexers.
// Each is project-scoped: disabled when no project is loaded, and
// busy-disabled while a same-kind job is running (we don't want
// double-fires of long jobs).

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useAgentStore } from "../agent/store";
import { useTimelineStore } from "../timeline/store";
import { useExportJob } from "./useExportJob";

export function ActionBar() {
  const items = useAgentStore((s) => s.items);
  const timelineDuration = useTimelineStore((s) => s.snapshot.duration_s);
  const { start: startExport } = useExportJob();
  const [showUrl, setShowUrl] = useState(false);
  const [urlInput, setUrlInput] = useState("");
  const [error, setError] = useState<string | null>(null);

  // Disable per-kind buttons while a same-kind job is in flight.
  // The auto-chain produces sequential jobs, not concurrent — so
  // detecting a running transcode also disables Run indexers (the
  // chain will fire indexing itself once transcode is done).
  const runningKinds = items
    .filter(
      (it): it is Extract<typeof items[number], { kind: "job" }> =>
        it.kind === "job" && it.phase !== "completed",
    )
    .map((it) => it.job_kind);
  const importBusy =
    runningKinds.includes("local_import") ||
    runningKinds.includes("url_import");
  const transcodeBusy = runningKinds.includes("transcode");
  const indexBusy = runningKinds.includes("indexing");
  const exportBusy = runningKinds.includes("render");
  const exportDisabled = exportBusy || timelineDuration === 0;

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
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function submitUrl() {
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
    } catch (e) {
      setError(String(e));
    }
  }

  async function runExport() {
    setError(null);
    if (exportDisabled) return;
    try {
      await startExport();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <div className="action-bar">
        <button onClick={importLocal} disabled={importBusy}>
          Import file…
        </button>
        <button onClick={() => setShowUrl(true)} disabled={importBusy}>
          Import URL…
        </button>
        <span className="action-bar-divider" aria-hidden="true" />
        <button onClick={runIndex} disabled={indexBusy || transcodeBusy}>
          Run indexers
        </button>
        <span className="action-bar-divider" aria-hidden="true" />
        <button
          onClick={runExport}
          disabled={exportDisabled}
          title={
            timelineDuration === 0
              ? "Add clips to the timeline before exporting"
              : exportBusy
              ? "Export in progress"
              : "Render the timeline to mp4"
          }
        >
          Export…
        </button>
        {error && <span className="action-bar-error">{error}</span>}
      </div>
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
                Awidat downloads via yt-dlp at up to 1080p mp4. After
                download, the proxy is generated and indexers run
                automatically — three job cards stream in sequence.
              </p>
            </div>
            <footer className="modal-footer">
              <button onClick={() => setShowUrl(false)}>Cancel</button>
              <button
                className="primary"
                onClick={submitUrl}
                disabled={!urlInput.trim()}
              >
                Download
              </button>
            </footer>
          </div>
        </div>
      )}
    </>
  );
}
