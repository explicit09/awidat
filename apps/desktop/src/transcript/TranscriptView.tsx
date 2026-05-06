// Transcript pane — Descript-style. Renders the whisper sidecar's
// segments as a virtualized scrolling list; each segment hosts its
// words as inline `<span data-word-idx={i}>` nodes so the active-
// word highlight (6.6) can flip a class imperatively without a
// React re-render per `timeupdate`.
//
// Virtualization strategy: by segment. Whisper produces ~50–100
// segments per minute of speech; a 60-minute podcast is ~3000-6000
// segment rows. With 50 visible at a time and ~30 words each, we
// have ~1500 word `<span>`s mounted — well within React's
// reconciliation budget. Virtualizing by word would mean every
// drag-select event triggers thousands of mount/unmount cycles
// during scroll. Don't.
//
// Click-to-seek lands in 6.5 (delegated event listener on the
// scroll container). Active-word highlight lands in 6.6 (imperative
// classList toggle keyed off useMediaStore.timelineTime). Drag-
// select + delete-range lands in 6.7.

import { useMemo, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useTranscriptStore } from "./store";
import type {
  Transcript,
  TranscriptSegment,
  TranscriptWord,
} from "../protocol";

/** Pre-computed row data: a segment + the slice of words[] that
 *  fall within its time range. Computed once per transcript load
 *  so the virtualizer's measurement function doesn't recompute it. */
type SegmentRow = {
  segment: TranscriptSegment;
  /** Index of the first word within the transcript's words[] that
   *  belongs to this segment. */
  firstWordIdx: number;
  /** Slice of words[] for this segment. Empty if the indexer
   *  didn't produce word-level alignment. */
  words: TranscriptWord[];
};

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
          tab.
        </p>
      </div>
    );
  }

  return <LoadedTranscript transcript={state.transcript} />;
}

function LoadedTranscript({ transcript }: { transcript: Transcript }) {
  const t = transcript;
  const scrollRef = useRef<HTMLDivElement | null>(null);

  // Pre-compute segment rows once per transcript. Words are sorted
  // by start_s (the backend already sorts on parse) so we can walk
  // them once with a moving cursor across segments.
  const rows = useMemo<SegmentRow[]>(() => {
    const out: SegmentRow[] = [];
    let wordCursor = 0;
    for (const segment of t.segments) {
      const firstWordIdx = wordCursor;
      const segWords: TranscriptWord[] = [];
      while (wordCursor < t.words.length) {
        const w = t.words[wordCursor];
        // Word belongs to segment if its midpoint is inside [start, end].
        // Half-open at the right side so a word that exactly aligns
        // with the next segment's start goes there, not here.
        const mid = (w.start_s + w.end_s) / 2;
        if (mid >= segment.end_s) break;
        if (mid >= segment.start_s) segWords.push(w);
        wordCursor += 1;
      }
      out.push({ segment, firstWordIdx, words: segWords });
    }
    return out;
  }, [t]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    // Estimate: speaker line + ~3 lines of text + timestamp + margin.
    // Real heights vary; the virtualizer will measure rendered rows
    // and update offsets automatically.
    estimateSize: () => 96,
    overscan: 6,
  });

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
      <div ref={scrollRef} className="transcript-scroll">
        <div
          style={{
            height: virtualizer.getTotalSize(),
            position: "relative",
            width: "100%",
          }}
        >
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            return (
              <div
                key={vi.key}
                ref={virtualizer.measureElement}
                data-index={vi.index}
                className="transcript-segment"
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${vi.start}px)`,
                }}
              >
                {row.segment.speaker_id ? (
                  <div className="transcript-speaker">
                    {row.segment.speaker_id}
                  </div>
                ) : null}
                <SegmentBody row={row} />
                <div className="transcript-time">
                  {fmt(row.segment.start_s)} – {fmt(row.segment.end_s)}
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

/** Render a segment's words as inline spans when alignment exists,
 *  or fall back to the segment's concatenated text. The data-word-
 *  idx attribute is what 6.6's active-word highlight keys off (and
 *  6.5's click-to-seek event delegation). */
function SegmentBody({ row }: { row: SegmentRow }) {
  if (row.words.length === 0) {
    // No word-level alignment → render the segment's plain text.
    // Click-to-seek (6.5) falls back to seeking to segment.start_s
    // when the click target lacks a word index.
    return (
      <div
        className="transcript-text"
        data-segment-start={row.segment.start_s}
      >
        {row.segment.text}
      </div>
    );
  }
  return (
    <div className="transcript-text">
      {row.words.map((w, i) => (
        <span
          key={i}
          data-word-idx={row.firstWordIdx + i}
          data-word-start={w.start_s}
          data-word-end={w.end_s}
          className="transcript-word"
        >
          {w.text}
          {i < row.words.length - 1 ? " " : ""}
        </span>
      ))}
    </div>
  );
}

function fmt(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}
