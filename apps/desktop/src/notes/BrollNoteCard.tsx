// BrollNoteCard — UI variant for `broll_suggestion` Notes (Phase 3.4).
//
// Two states:
//   1. With pre-fetched previews: render a thumbnail row. Each
//      thumbnail has a "Use this" button that asks the agent to
//      call use_broll(pexels_id, anchor=transcript_snippet, ...)
//      via a chat directive; the agent downloads + emits the EDL.
//   2. Without previews (the more common case at first surface):
//      render the agent's pexels_query and a "Search Pexels"
//      button that asks the agent to call search_broll(query) so
//      it comes back with previews on the next turn.
//
// In both cases, click-on-body still seeks the timeline to the
// note's anchor — same affordance as every other note kind.

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useAgentStore } from "../agent/store";
import { isAuthReadyForAgent } from "../agent/composerAuthGate";
import { useMediaStore } from "../media/store";
import type { Note, DismissalBucket } from "./store";
import type { BrollAnchor } from "../protocol";
import { useAuth } from "../state/auth";

export function BrollNoteCard({
  note,
  onStatus,
  faded,
}: {
  note: Note;
  onStatus: (
    id: string,
    status: "open" | "resolved" | "dismissed",
    bucket?: DismissalBucket,
  ) => Promise<void>;
  faded?: boolean;
}) {
  const requestSeek = useMediaStore((s) => s.requestTimelineSeek);
  const setActiveTurnId = useAgentStore((s) => s.setActiveTurnId);
  const authStatus = useAuth((s) => s.status);
  const openAuth = useAuth((s) => s.open);
  const [busy, setBusy] = useState(false);
  const authReady = isAuthReadyForAgent(authStatus);

  function seek() {
    requestSeek(note.anchorAtS);
  }

  async function searchPexels() {
    if (!note.brollQuery) return;
    if (!authReady) {
      openAuth();
      return;
    }
    setBusy(true);
    try {
      // Ask the agent to search and re-emit the note with previews
      // populated. The agent's prompt + this message wording are the
      // contract: phrase it as a directive, not a question.
      const directive =
        `For the b-roll note at ${note.anchorAtS.toFixed(2)}s ("${note.summary}"), ` +
        `call search_broll(query="${note.brollQuery.replace(/"/g, '\\"')}", per_page=5). ` +
        `Then re-emit the note with broll_previews populated so I can pick one.`;
      const turnId = await invoke<string>("start_turn", { input: directive });
      setActiveTurnId(turnId);
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("start_turn (search_broll) failed", e);
    } finally {
      setBusy(false);
    }
  }

  async function useThis(pexelsId: bigint, durationS: number) {
    const anchorArg = formatUseBrollAnchor(note.brollAnchor);
    if (!anchorArg) return;
    if (!authReady) {
      openAuth();
      return;
    }
    setBusy(true);
    try {
      // The cutaway duration: prefer the note's source-range length
      // if it carries one (we don't expose that today), else clamp
      // the Pexels clip length to a sensible default.
      const dur = Math.min(Math.max(durationS, 1.5), 4.0);
      const directive =
        `For the b-roll note at ${note.anchorAtS.toFixed(2)}s ("${note.summary}"), ` +
        `call use_broll(pexels_id=${pexelsId.toString()}, ` +
        `anchor=${anchorArg}, ` +
        `duration_s=${dur.toFixed(1)}, position="overlay"). ` +
        `Then hand the returned edl_fragment to apply_edl to place the cutaway.`;
      const turnId = await invoke<string>("start_turn", { input: directive });
      setActiveTurnId(turnId);
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("start_turn (use_broll) failed", e);
    } finally {
      setBusy(false);
    }
  }

  async function resolve() {
    setBusy(true);
    await onStatus(note.id, "resolved");
    setBusy(false);
  }

  async function dismiss() {
    setBusy(true);
    await onStatus(note.id, "dismissed", { kind: "broll_opportunity" });
    setBusy(false);
  }

  return (
    <article
      className={`note-card note-card-${note.status} note-card-broll ${
        faded ? "note-card-faded" : ""
      }`}
    >
      <div className="note-card-body" onClick={seek} title="Seek timeline">
        <span className="note-icon note-icon-broll_suggestion" aria-hidden>
          ▶
        </span>
        <div className="note-text">
          <div className="note-summary">{note.summary}</div>
          {note.brollQuery && (
            <div className="note-broll-query">
              Pexels query: <code>{note.brollQuery}</code>
            </div>
          )}
          <div className="note-anchor">at {formatTime(note.anchorAtS)}</div>
        </div>
      </div>

      {note.brollPreviews && note.brollPreviews.length > 0 && (
        <div className="note-broll-previews" onClick={(e) => e.stopPropagation()}>
          {note.brollPreviews.map((p) => (
            <div key={p.pexels_id.toString()} className="note-broll-thumb">
              <img
                src={p.thumbnail_url}
                alt={`Pexels ${p.pexels_id}`}
                loading="lazy"
                onClick={(e) => e.stopPropagation()}
              />
              <div className="note-broll-meta">
                <span className="note-broll-duration">{p.duration_s}s</span>
                <a
                  href={p.pexels_page}
                  target="_blank"
                  rel="noopener noreferrer"
                  onClick={(e) => e.stopPropagation()}
                >
                  {p.attribution}
                </a>
              </div>
              {note.status === "open" && (
                <button
                  className="note-broll-use"
                  disabled={busy || !note.brollAnchor}
                  onClick={(e) => {
                    e.stopPropagation();
                    useThis(p.pexels_id, p.duration_s);
                  }}
                  title={
                    note.brollAnchor
                      ? "Download this clip and place it on the timeline"
                      : "This note is missing a concrete b-roll anchor"
                  }
                >
                  {busy ? "…" : "Use this"}
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {note.status === "open" && (
        <div className="note-actions">
          {!note.brollPreviews && note.brollQuery && (
            <button
              onClick={searchPexels}
              disabled={busy}
              title={authReady ? "Ask the agent to fetch Pexels previews for this query" : "Sign in to get started"}
            >
              {busy ? "…" : authReady ? "Search Pexels" : "Sign in to get started"}
            </button>
          )}
          <button onClick={resolve} disabled={busy} title="Mark resolved">
            ✓
          </button>
          <button
            onClick={dismiss}
            disabled={busy}
            title="Dismiss b-roll suggestions for this project"
          >
            ✕
          </button>
        </div>
      )}
    </article>
  );
}

export function formatUseBrollAnchor(anchor: BrollAnchor | undefined): string | null {
  if (!anchor) return null;
  switch (anchor.kind) {
    case "clip_uuid":
      return JSON.stringify({ clip_uuid: anchor.uuid });
    case "transcript_snippet":
      return JSON.stringify({ transcript_snippet: anchor.text });
  }
}

function formatTime(s: number): string {
  if (!isFinite(s)) return "—";
  const mins = Math.floor(s / 60);
  const secs = (s - mins * 60).toFixed(1);
  return `${mins}:${secs.padStart(4, "0")}`;
}
