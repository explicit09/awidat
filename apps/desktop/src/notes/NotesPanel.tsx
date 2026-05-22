// NotesPanel — collapsible panel above the composer that surfaces
// editorial findings the agent flagged. Phase 1.7.
//
// Each open note has three actions:
//   - Generate Proposal: pipes suggested_proposal through
//     propose_user_edit, opening the standard ghost-overlay flow.
//   - Resolve: marks the note as acted-upon (no dismissal pattern
//     stamped — the user fixed THIS finding, not "all findings of
//     this kind").
//   - Dismiss: marks the note dismissed AND stamps the coarse
//     pattern in dismissed_patterns.json so the agent's editorial-
//     finding tools won't re-surface that bucket on next scan.
//
// Click anywhere on the note's body seeks the timeline to its
// anchor_at_s.

import { useEffect, useMemo, useState } from "react";
import { useNotesStore, type DismissalBucket, type Note } from "./store";
import { useProjectStore } from "../app/state";
import { useMediaStore } from "../media/store";
import type { EditorialNoteKind } from "../protocol";
import { BrollNoteCard } from "./BrollNoteCard";
import { editorDispatch } from "../editor/tauriDispatch";

/** Cap on visible open notes before the panel collapses tail
 *  entries behind a "show all" affordance. Long podcasts can
 *  surface dozens; we don't let them dominate the chat column. */
const VISIBLE_OPEN_CAP = 10;

export function NotesPanel() {
  const projectReady = useProjectStore((s) => s.current !== null);
  const projectRoot = useProjectStore((s) => s.current);
  const notes = useNotesStore((s) => s.notes);
  const busy = useNotesStore((s) => s.busy);
  const refresh = useNotesStore((s) => s.refresh);
  const setStatus = useNotesStore((s) => s.setStatus);
  const clear = useNotesStore((s) => s.clear);
  const [collapsed, setCollapsed] = useState(false);
  const [showAll, setShowAll] = useState(false);

  // Refresh notes on mount + project change.
  useEffect(() => {
    if (projectReady) {
      refresh();
    } else {
      clear();
    }
  }, [projectReady, projectRoot, refresh, clear]);

  // Sort: open first (by anchor_at_s ascending), then resolved,
  // then dismissed. Within each status, ordered by timeline anchor
  // so the panel reads chronologically.
  const sorted = useMemo(() => {
    const order: Record<string, number> = { open: 0, resolved: 1, dismissed: 2 };
    return [...notes].sort((a, b) => {
      const da = order[a.status] ?? 99;
      const db = order[b.status] ?? 99;
      if (da !== db) return da - db;
      return a.anchorAtS - b.anchorAtS;
    });
  }, [notes]);

  const open = sorted.filter((n) => n.status === "open");
  const resolved = sorted.filter((n) => n.status === "resolved");
  const dismissed = sorted.filter((n) => n.status === "dismissed");
  const visibleOpen = showAll ? open : open.slice(0, VISIBLE_OPEN_CAP);
  const hiddenOpenCount = Math.max(0, open.length - visibleOpen.length);

  if (!projectReady) {
    return null;
  }
  if (notes.length === 0) {
    return null;
  }

  return (
    <section
      className={`notes-panel ${collapsed ? "notes-panel-collapsed" : ""} ${
        busy ? "notes-panel-busy" : ""
      }`}
    >
      <header
        className="notes-header"
        onClick={() => setCollapsed((v) => !v)}
        title={collapsed ? "Click to expand" : "Click to collapse"}
      >
        <span className="notes-label">Editorial notes</span>
        <span className="notes-counts">
          {open.length > 0 && <span className="notes-count-open">{open.length} open</span>}
          {resolved.length > 0 && (
            <span className="notes-count-resolved">{resolved.length} resolved</span>
          )}
          {dismissed.length > 0 && (
            <span className="notes-count-dismissed">{dismissed.length} dismissed</span>
          )}
        </span>
        <span className="notes-chevron">{collapsed ? "▸" : "▾"}</span>
      </header>
      {!collapsed && (
        <div className="notes-body">
          {open.length === 0 && (
            <div className="notes-empty">No open notes — the agent hasn't flagged anything yet.</div>
          )}
          {visibleOpen.map((n) => (
            <NoteCardDispatch key={n.id} note={n} onStatus={setStatus} />
          ))}
          {hiddenOpenCount > 0 && (
            <button
              className="notes-show-all"
              onClick={() => setShowAll(true)}
              disabled={busy}
            >
              Show {hiddenOpenCount} more open note{hiddenOpenCount === 1 ? "" : "s"}
            </button>
          )}
          {resolved.length > 0 || dismissed.length > 0 ? (
            <div className="notes-archive">
              {resolved.slice(0, 5).map((n) => (
                <NoteCardDispatch key={n.id} note={n} onStatus={setStatus} faded />
              ))}
              {dismissed.slice(0, 3).map((n) => (
                <NoteCardDispatch key={n.id} note={n} onStatus={setStatus} faded />
              ))}
            </div>
          ) : null}
        </div>
      )}
    </section>
  );
}

/** Dispatcher that routes b-roll notes to the BrollNoteCard variant
 *  and everything else to the default NoteCard. Keeps the panel's
 *  iteration site simple. */
function NoteCardDispatch(props: {
  note: Note;
  onStatus: (
    id: string,
    status: "open" | "resolved" | "dismissed",
    bucket?: DismissalBucket,
  ) => Promise<void>;
  faded?: boolean;
}) {
  if (props.note.kind === "broll_suggestion") {
    return <BrollNoteCard {...props} />;
  }
  return <NoteCard {...props} />;
}

function NoteCard({
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
  const [busy, setBusy] = useState(false);

  function seek() {
    requestSeek(note.anchorAtS);
  }

  async function generate() {
    if (!note.suggestedProposal) return;
    setBusy(true);
    try {
      await editorDispatch.proposeUserEditText(note.suggestedProposal);
      // The agent loop will fire an EditorialNote re-emit on
      // proposal acceptance to mark it resolved; in the meantime
      // we leave the note open. Worst case the user clicks
      // Resolve manually if the proposal was rejected.
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (note) failed", e);
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
    const bucket = bucketFor(note);
    await onStatus(note.id, "dismissed", bucket ?? undefined);
    setBusy(false);
  }

  // continuity_warning notes carry a verdict and per-rule reasons.
  // The card gets a verdict-colored class so the user sees urgency
  // at a glance; reasons render as a bullet list under the summary.
  const verdictClass = note.continuityVerdict
    ? `note-card-verdict-${note.continuityVerdict}`
    : "";

  return (
    <article
      className={`note-card note-card-${note.status} ${verdictClass} ${
        faded ? "note-card-faded" : ""
      }`}
    >
      <div className="note-card-body" onClick={seek} title="Seek timeline">
        <span className={`note-icon note-icon-${note.kind}`} aria-hidden>
          {iconFor(note.kind)}
        </span>
        <div className="note-text">
          <div className="note-summary">
            {note.continuityVerdict && (
              <span
                className={`note-verdict-pill note-verdict-${note.continuityVerdict}`}
              >
                {note.continuityVerdict}
              </span>
            )}
            {note.summary}
          </div>
          {note.continuityReasons && note.continuityReasons.length > 0 && (
            <ul className="note-reasons">
              {note.continuityReasons.map((r, i) => (
                <li key={i}>{r}</li>
              ))}
            </ul>
          )}
          <div className="note-anchor">
            at {formatTime(note.anchorAtS)}
          </div>
        </div>
      </div>
      {note.status === "open" && (
        <div className="note-actions">
          {note.suggestedProposal && (
            <button
              onClick={generate}
              disabled={busy}
              title="Apply the agent's suggested fix"
            >
              {busy ? "…" : "Fix"}
            </button>
          )}
          <button onClick={resolve} disabled={busy} title="Mark resolved">
            ✓
          </button>
          <button
            onClick={dismiss}
            disabled={busy}
            title="Dismiss this AND stop suggesting this kind"
          >
            ✕
          </button>
        </div>
      )}
    </article>
  );
}

/** Map an editorial-note kind to a single-glyph icon. */
function iconFor(kind: EditorialNoteKind): string {
  switch (kind) {
    case "silence_trim":
      return "𝛁"; // dipping bracket-shaped silence indicator
    case "filler_word":
      return "𝛍"; // mu, looks like an "uh"
    case "false_start":
      return "↻"; // restart loop
    case "continuity_warning":
      return "⚠"; // the only one with a "this might break" feel
    case "broll_suggestion":
      return "▶"; // play/clip
    case "generic":
      return "◇";
  }
}

/** Map a note to the coarse dismissal bucket the matcher uses.
 *  Returns `null` for kinds whose bucket isn't representable from
 *  the note alone (continuity warnings carry their own logic;
 *  b-roll suggestions don't have a global bucket in v1).
 *
 *  For silence: the bucket depends on the duration, which we
 *  don't carry on the wire. Heuristic: parse the summary's
 *  "(2.4s)" suffix that the find_dead_air tool's note-summary
 *  builder writes. If we can't parse it, fall back to under_2s
 *  which is the safest default ("hide more aggressively"). */
function bucketFor(note: Note): DismissalBucket | null {
  switch (note.kind) {
    case "silence_trim": {
      const m = note.summary.match(/\((\d+\.?\d*)\s*s\)/);
      const dur = m ? parseFloat(m[1]) : NaN;
      if (!isFinite(dur)) return { kind: "silence_under_2s" };
      if (dur < 2.0) return { kind: "silence_under_2s" };
      if (dur < 5.0) return { kind: "silence_2s_to_5s" };
      return { kind: "silence_over_5s" };
    }
    case "filler_word":
      // The tool tracks aggressive vs basic on the call side; the
      // note doesn't currently carry that distinction. Default to
      // basic — the conservative dismissal (hides only basic-list
      // matches; aggressive findings still surface).
      return { kind: "filler_basic" };
    case "false_start":
      return { kind: "false_start" };
    case "continuity_warning":
    case "broll_suggestion":
    case "generic":
      return null;
  }
}

function formatTime(s: number): string {
  if (!isFinite(s) || s < 0) return "--:--";
  const m = Math.floor(s / 60);
  const sec = (s - m * 60).toFixed(1);
  return `${m}:${sec.padStart(4, "0")}`;
}
