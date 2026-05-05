// Shown in the chat pane when the project has no Items yet AND no
// media in raw/. The point of life: tell the user what to do first
// (import media → index → start chatting). Mirrors the language of
// `awidat new`'s "Next:" output.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

export function EmptyState() {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function importLocal() {
    setError(null);
    try {
      const picked = await openDialog({
        directory: false,
        multiple: false,
        title: "Choose media file to import",
      });
      if (typeof picked === "string") {
        setBusy(true);
        await invoke("import_local", { srcPath: picked, link: false });
        setBusy(false);
      }
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  async function importUrlSubmit() {
    const u = url.trim();
    if (!u) return;
    setError(null);
    setBusy(true);
    try {
      setUrl("");
      await invoke("import_url", { url: u });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function index() {
    setError(null);
    setBusy(true);
    try {
      await invoke("index_project");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="empty-state">
      <h2>Drop media in to get started</h2>
      <p className="empty-hint">
        Awidat needs source media in <code>raw/</code> before the agent has
        anything to edit. After import, run the indexers — they produce the
        transcript and editorial signals the agent reads.
      </p>

      <section className="empty-action">
        <header>
          <strong>Import a local file</strong>
          <span>copy or symlink an mp4/mov/etc. into the project</span>
        </header>
        <button onClick={importLocal} disabled={busy}>
          Choose file…
        </button>
      </section>

      <section className="empty-action">
        <header>
          <strong>Download from URL</strong>
          <span>YouTube, Vimeo, podcasts — anything yt-dlp can reach</span>
        </header>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            importUrlSubmit();
          }}
        >
          <input
            type="text"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://www.youtube.com/watch?v=…"
            disabled={busy}
          />
          <button type="submit" disabled={busy || !url.trim()}>
            Download
          </button>
        </form>
      </section>

      <section className="empty-action">
        <header>
          <strong>Run indexers on existing media</strong>
          <span>
            Already dropped files into <code>raw/</code> manually? Kick off
            the indexer pipeline.
          </span>
        </header>
        <button onClick={index} disabled={busy}>
          Index assets
        </button>
      </section>

      {error && <div className="empty-error">{error}</div>}
    </div>
  );
}
