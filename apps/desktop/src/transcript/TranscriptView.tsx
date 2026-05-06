// Transcript pane — Descript-style. Renders the whisper sidecar's
// segments as a scrollable column; click-to-seek + drag-to-delete
// land in 6.5 / 6.7. This commit (6.3) is the empty-state +
// loading-state + simple-render scaffold.
//
// Virtualization lands in 6.4 (@tanstack/react-virtual). For now we
// render every segment — fine for short test transcripts; a
// 3-hour podcast would lag without virtualization.

import { useTranscriptStore } from "./store";

export function TranscriptView({ stem }: { stem: string | null }) {
  const state = useTranscriptStore((s) =>
    stem ? s.byStem[stem] : undefined,
  );

  if (!stem) {
    return (
      <div className="media-empty">
        <p className="media-empty-title">No transcript context.</p>
        <p className="media-empty-hint">
          Pick an asset (or run an edit on the timeline) to see its
          transcript here.
        </p>
      </div>
    );
  }

  if (!state || state.state === "loading") {
    return (
      <div className="media-empty">
        <p>Loading transcript…</p>
      </div>
    );
  }

  if (state.state === "missing") {
    return (
      <div className="media-empty">
        <p className="media-empty-title">No transcript yet.</p>
        <p className="media-empty-hint">
          Run whisper indexing on <code>{stem}</code> to populate this
          tab. The agent does this automatically when you ask for an
          editorial operation that needs alignment, or you can run{" "}
          <strong>Index project</strong> manually.
        </p>
      </div>
    );
  }

  const t = state.transcript;
  return (
    <div className="transcript-pane">
      <header className="transcript-meta">
        <span className="transcript-meta-lang">
          {t.language || "—"}
          {t.diarized ? " · diarized" : ""}
        </span>
        <span className="transcript-meta-counts">
          {t.segments.length} segments · {t.words.length} words
        </span>
      </header>
      <div className="transcript-scroll">
        {t.segments.map((seg, i) => (
          <div key={i} className="transcript-segment">
            {seg.speaker_id ? (
              <div className="transcript-speaker">{seg.speaker_id}</div>
            ) : null}
            <div className="transcript-text">{seg.text}</div>
            <div className="transcript-time">
              {fmt(seg.start_s)} – {fmt(seg.end_s)}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function fmt(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}
