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

import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useTranscriptStore } from "./store";
import { mediaStreamUrl } from "../media/mediaStreamUrl";
import { useMediaStore } from "../media/store";
import { useTimelineStore, type TimelineSnapshot } from "../timeline/store";
import {
  findActiveSegment,
  nearestTimelineTimeForSource,
  timelineTimeForSource,
  usePlaySegments,
} from "../timeline/usePlaySegments";
import { type EdlOp } from "../timeline/edlBuilder";
import { editorDispatch } from "../editor/tauriDispatch";
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
  // When there's a timeline, render the composed view (every clip's
  // words in timeline order, spliced on clip windows). Falls back to
  // the single-stem view for the pre-timeline case (source-preview of
  // a newly imported asset).
  const playSegments = usePlaySegments();
  if (playSegments.length > 0) {
    return <TimelineComposedTranscript playSegments={playSegments} />;
  }
  return <SingleStemTranscript stem={stem} />;
}

/** One visible row in the composed timeline transcript. Each row is
 *  one whisper segment, scoped to a single timeline clip (`playSegment`).
 *  When the same source asset appears in multiple clips, its sidecar
 *  contributes one row per clip — at each clip's timeline position. */
type ComposedRow = {
  /** The clip this row belongs to. */
  playSegment: import("../timeline/usePlaySegments").PlaySegment;
  /** Whisper segment from the clip's sidecar, sliced to the clip
   *  window. `start_s` and `end_s` are still source-time; mapping to
   *  timeline-time uses the clip's `sourceStart`/`timelineStart`. */
  segment: TranscriptSegment;
  /** Words belonging to this segment, also clipped to the window. */
  words: TranscriptWord[];
  /** Index of the first word within the parent transcript's words[].
   *  Carries the original index so the active-word highlight (which
   *  keys off `data-word-idx`) lines up with the global word table. */
  firstWordIdx: number;
};

type ComposedPlayback = { rowIdx: number; sourceTime: number | null };

function TimelineComposedTranscript({
  playSegments,
}: {
  playSegments: import("../timeline/usePlaySegments").PlaySegment[];
}) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);

  // Ensure every stem on the timeline is loaded into the transcript
  // store. The store already de-dupes in-flight loads and caches
  // results across tab toggles.
  const load = useTranscriptStore((s) => s.load);
  const stems = useMemo(() => {
    const set = new Set<string>();
    for (const seg of playSegments) {
      if (seg.proxyStem) set.add(seg.proxyStem);
    }
    return Array.from(set);
  }, [playSegments]);

  useEffect(() => {
    for (const stem of stems) void load(stem);
  }, [stems, load]);

  // Subscribe to the whole byStem map so any sidecar arriving causes
  // a re-render. The map identity changes on each store update, so the
  // selector returns a new ref per change without needing equality.
  const byStem = useTranscriptStore((s) => s.byStem);

  // Build the row list: walk timeline-ordered playSegments, look up
  // each clip's transcript, slice its segments + words to the clip's
  // [sourceStart, sourceEnd] window. Slicing means a word whose
  // midpoint lands outside the window is dropped, so a segment that
  // straddles a clip cut shows on whichever clip contains the bulk
  // of the speech (or both, partially).
  const rows = useMemo<ComposedRow[]>(() => {
    const out: ComposedRow[] = [];
    for (const ps of playSegments) {
      const stem = ps.proxyStem;
      if (!stem) continue;
      const st = byStem[stem];
      if (!st || st.state !== "loaded") continue;
      const transcript = st.transcript;
      const lo = ps.sourceStart;
      const hi = ps.sourceEnd;
      for (const seg of transcript.segments) {
        // Skip segments that don't overlap the clip window at all.
        if (seg.end_s <= lo || seg.start_s >= hi) continue;
        // Walk the transcript's words list to find both the slice and
        // its global first-index. A linear scan per segment is fine
        // here — segments are sparse (~50–100/min) and the words[]
        // is sorted by start_s.
        const segWords: TranscriptWord[] = [];
        let firstWordIdx = -1;
        for (let i = 0; i < transcript.words.length; i++) {
          const w = transcript.words[i];
          const mid = (w.start_s + w.end_s) / 2;
          if (mid < seg.start_s) continue;
          if (mid >= seg.end_s) break;
          // Drop words whose midpoint lands outside the clip window.
          if (mid < lo || mid >= hi) continue;
          if (firstWordIdx < 0) firstWordIdx = i;
          segWords.push(w);
        }
        out.push({
          playSegment: ps,
          segment: seg,
          words: segWords,
          firstWordIdx: firstWordIdx < 0 ? 0 : firstWordIdx,
        });
      }
    }
    return out;
  }, [playSegments, byStem]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 96,
    overscan: 6,
  });

  // Active-row + active-word follow is a hot playback path. Keep it
  // out of React render so transcript scrubbing doesn't force the
  // virtualized list to reconcile on every timeline-time tick.
  const rowsRef = useRef(rows);
  const virtualizerRef = useRef(virtualizer);
  const lastActiveRef = useRef<{ rowIdx: number; wordKey: string | null }>({
    rowIdx: -1,
    wordKey: null,
  });
  rowsRef.current = rows;
  virtualizerRef.current = virtualizer;

  useEffect(() => {
    const update = (timelineTime: number) => {
      const active = findComposedPlayback(rowsRef.current, timelineTime);
      updateComposedTranscriptActive(
        scrollRef.current,
        virtualizerRef.current,
        rowsRef.current,
        lastActiveRef.current,
        active,
      );
    };

    update(useMediaStore.getState().timelineTime);
    return useMediaStore.subscribe((state, previous) => {
      if (state.timelineTime === previous.timelineTime) return;
      update(state.timelineTime);
    });
  }, [rows]);

  if (stems.length === 0) {
    return (
      <div className="media-empty">
        <p className="media-empty-title">No clips on the timeline.</p>
        <p className="media-empty-hint">
          Drop a clip on the timeline to see its transcript here.
        </p>
      </div>
    );
  }

  // Per-stem load status feedback so the user understands why some
  // ranges are blank (sidecar still loading, or transcript missing
  // entirely for that asset).
  const pendingStems = stems.filter(
    (s) => !byStem[s] || byStem[s]?.state === "loading",
  );
  const missingStems = stems.filter((s) => byStem[s]?.state === "missing");

  return (
    <div className="transcript-pane">
      <header className="transcript-meta">
        <span className="transcript-meta-lang">
          Timeline · {rows.length} segments · {stems.length} sources
        </span>
        <span className="transcript-meta-counts">
          {pendingStems.length > 0
            ? `loading ${pendingStems.length}…`
            : missingStems.length > 0
              ? `${missingStems.length} missing transcript`
              : ""}
        </span>
      </header>
      <div
        ref={scrollRef}
        className="transcript-scroll"
        onClick={(e) => {
          // Click on a word or segment: seek the timeline playhead to
          // the corresponding timeline-time.
          const target = (e.target as HTMLElement).closest(
            "[data-row-idx]",
          ) as HTMLElement | null;
          if (!target) return;
          const rowIdxStr = target.getAttribute("data-row-idx");
          if (rowIdxStr === null) return;
          const rowIdx = Number(rowIdxStr);
          const row = rows[rowIdx];
          if (!row) return;
          const wordTarget = (e.target as HTMLElement).closest(
            "[data-word-start]",
          ) as HTMLElement | null;
          const sourceTime = wordTarget
            ? Number(wordTarget.getAttribute("data-word-start"))
            : row.segment.start_s;
          if (!Number.isFinite(sourceTime)) return;
          const tlTime =
            row.playSegment.timelineStart +
            (sourceTime - row.playSegment.sourceStart);
          void mediaStreamUrl(row.playSegment.proxyPath).catch(() => {});
          requestTimelineSeek(tlTime);
        }}
      >
        <div
          style={{
            height: virtualizer.getTotalSize(),
            position: "relative",
            width: "100%",
          }}
        >
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows[vi.index];
            const tlStart =
              row.playSegment.timelineStart +
              (row.segment.start_s - row.playSegment.sourceStart);
            return (
              <div
                key={vi.key}
                ref={virtualizer.measureElement}
                data-index={vi.index}
                data-row-idx={vi.index}
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
                  <SpeakerLabel
                    stem={row.playSegment.proxyStem}
                    speakerId={row.segment.speaker_id}
                  />
                ) : null}
                <ComposedSegmentBody row={row} rowIdx={vi.index} />
                <div className="transcript-time">{fmt(tlStart)}</div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function ComposedSegmentBody({
  row,
  rowIdx,
}: {
  row: ComposedRow;
  rowIdx: number;
}) {
  if (row.words.length === 0) {
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
          data-composed-word-key={`${rowIdx}:${i}`}
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

function findComposedPlayback(
  rows: ComposedRow[],
  timelineTime: number,
): ComposedPlayback {
  if (rows.length === 0 || !Number.isFinite(timelineTime)) {
    return { rowIdx: -1, sourceTime: null };
  }
  let lo = 0;
  let hi = rows.length - 1;
  let candidate = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    const row = rows[mid];
    const rowEnd =
      row.playSegment.timelineStart +
      (row.segment.end_s - row.playSegment.sourceStart);
    if (timelineTime < rowEnd) {
      candidate = mid;
      hi = mid - 1;
    } else {
      lo = mid + 1;
    }
  }
  if (candidate < 0) return { rowIdx: -1, sourceTime: null };

  const row = rows[candidate];
  if (
    timelineTime < row.playSegment.timelineStart ||
    timelineTime >= row.playSegment.timelineEnd
  ) {
    return { rowIdx: -1, sourceTime: null };
  }

  const sourceTime =
    row.playSegment.sourceStart +
    (timelineTime - row.playSegment.timelineStart);
  if (sourceTime < row.segment.start_s || sourceTime >= row.segment.end_s) {
    return { rowIdx: -1, sourceTime: null };
  }
  return { rowIdx: candidate, sourceTime };
}

function updateComposedTranscriptActive(
  scope: HTMLDivElement | null,
  virtualizer: ReturnType<typeof useVirtualizer<HTMLDivElement, Element>>,
  rows: ComposedRow[],
  last: { rowIdx: number; wordKey: string | null },
  active: ComposedPlayback,
): void {
  const previousRow = last.rowIdx;
  const previousWordKey = last.wordKey;
  const row = active.rowIdx >= 0 ? rows[active.rowIdx] : undefined;
  let nextWordKey: string | null = null;
  if (row && active.sourceTime !== null) {
    const wordIdx = row.words.findIndex(
      (w) => active.sourceTime! >= w.start_s && active.sourceTime! <= w.end_s + 0.05,
    );
    if (wordIdx >= 0) nextWordKey = `${active.rowIdx}:${wordIdx}`;
  }

  if (previousRow !== active.rowIdx) {
    scope
      ?.querySelector(`[data-row-idx="${previousRow}"]`)
      ?.classList.remove("transcript-segment-active");
  }
  if (previousWordKey !== nextWordKey && previousWordKey !== null) {
    scope
      ?.querySelector(`[data-composed-word-key="${previousWordKey}"]`)
      ?.classList.remove("transcript-word-active");
  }

  last.rowIdx = active.rowIdx;
  last.wordKey = nextWordKey;

  const apply = () => {
    scope
      ?.querySelector(`[data-row-idx="${active.rowIdx}"]`)
      ?.classList.add("transcript-segment-active");
    if (nextWordKey !== null) {
      scope
        ?.querySelector(`[data-composed-word-key="${nextWordKey}"]`)
        ?.classList.add("transcript-word-active");
    }
  };

  if (active.rowIdx < 0) return;
  if (previousRow !== active.rowIdx) {
    virtualizer.scrollToIndex(active.rowIdx, { align: "center", behavior: "auto" });
    requestAnimationFrame(() => requestAnimationFrame(apply));
    return;
  }
  apply();
}

function SingleStemTranscript({ stem }: { stem: string | null }) {
  const state = useTranscriptStore((s) =>
    stem ? s.byStem[stem] : undefined,
  );

  if (!stem) {
    return (
      <div className="media-empty">
        <p className="media-empty-title">No transcript context.</p>
        <p className="media-empty-hint">
          Pick an asset (or load a timeline) to see its transcript here.
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

  return <LoadedTranscript transcript={state.transcript} stem={stem} />;
}

function LoadedTranscript({
  transcript,
  stem,
}: {
  transcript: Transcript;
  stem: string;
}) {
  const t = transcript;
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const segments = usePlaySegments();
  const requestTimelineSeek = useMediaStore((s) => s.requestTimelineSeek);
  const requestSeek = useMediaStore((s) => s.requestSeek);
  const selection = useTranscriptStore((s) => s.selection);
  const setSelection = useTranscriptStore((s) => s.setSelection);
  const snapshot = useTimelineStore((s) => s.snapshot);
  // Drag state — refs because we don't want a render per pointermove.
  const dragStartRef = useRef<number | null>(null);
  const didDragRef = useRef<boolean>(false);

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

  // Active-word highlight + auto-scroll. Imperative DOM toggle on
  // [data-word-idx] spans driven by whichever playhead is active —
  // the timeline preview when a clip is on the OTIO timeline that
  // matches this transcript's stem, otherwise the source-preview
  // currentTime. Without the source-preview fallback the highlight
  // never fires for assets that haven't been auto-inserted onto the
  // timeline yet, which is exactly the case immediately after import.
  const timelineTime = useMediaStore((s) => s.timelineTime);
  const sourcePreviewTime = useMediaStore((s) => s.currentTime);
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const tlSegments = segments;
  const activeWordIdxRef = useRef<number>(-1);
  const lastScrolledSegmentRef = useRef<number>(-1);
  const activeSourceTimeRef = useRef<number | null>(null);
  // Pre-build a sorted array of word start-times for binary search.
  const wordStarts = useMemo(
    () => t.words.map((w) => w.start_s),
    [t],
  );

  useEffect(() => {
    // Resolve source-time for this transcript's stem. Prefer the
    // timeline-time path (the timeline preview is the canonical
    // playhead when there's a clip on the OTIO); fall back to the
    // source-preview currentTime when no timeline segment covers the
    // playhead OR the timeline's active segment is for a different
    // asset. The fallback is what makes highlighting work for a clip
    // that's been imported + indexed but not yet placed on the
    // timeline.
    const tlSegIdx = findActiveSegment(tlSegments, timelineTime);
    let sourceTime: number | null = null;
    if (tlSegIdx >= 0 && tlSegments[tlSegIdx].proxyStem === stem) {
      const seg = tlSegments[tlSegIdx];
      sourceTime = seg.sourceStart + (timelineTime - seg.timelineStart);
    } else if (selectedStem === stem) {
      // Source-preview branch — the user is reviewing the source
      // asset directly; `currentTime` is already source-time on the
      // same asset this transcript belongs to.
      sourceTime = sourcePreviewTime;
    }

    if (sourceTime === null) {
      // Neither path matches — the active asset is something else.
      // Clear any lingering highlight so a stale word doesn't stay
      // lit on a transcript that isn't being followed.
      clearActive(scrollRef.current, activeWordIdxRef.current);
      activeWordIdxRef.current = -1;
      return;
    }

    activeSourceTimeRef.current = sourceTime;
    // Find the word whose [start_s, end_s] covers sourceTime via
    // binary search on the pre-built starts array.
    const wordIdx = findWordAt(wordStarts, t.words, sourceTime);
    if (wordIdx === activeWordIdxRef.current && wordIdx >= 0) return;
    clearActive(scrollRef.current, activeWordIdxRef.current);
    setActive(scrollRef.current, wordIdx);
    activeWordIdxRef.current = wordIdx;

    // Auto-scroll: when the active word's segment changes, scroll
    // the row into view. If the playhead lands in pre-roll silence
    // before a word, scroll to the nearest segment instead so the
    // transcript rail opens near the actual timeline context rather
    // than raw-source 0:00.
    if (wordIdx >= 0) {
      const newSegmentIdx = findSegmentForWord(rows, wordIdx);
      if (
        newSegmentIdx >= 0 &&
        newSegmentIdx !== lastScrolledSegmentRef.current
      ) {
        lastScrolledSegmentRef.current = newSegmentIdx;
        scrollTranscriptToIndex(virtualizer, newSegmentIdx);
      }
    } else {
      const nearestSegmentIdx = findSegmentAtOrAfter(rows, sourceTime);
      if (
        nearestSegmentIdx >= 0 &&
        nearestSegmentIdx !== lastScrolledSegmentRef.current
      ) {
        lastScrolledSegmentRef.current = nearestSegmentIdx;
        scrollTranscriptToIndex(virtualizer, nearestSegmentIdx);
      }
    }
  }, [
    timelineTime,
    sourcePreviewTime,
    selectedStem,
    tlSegments,
    stem,
    t.words,
    wordStarts,
    rows,
    virtualizer,
  ]);

  useEffect(() => {
    lastScrolledSegmentRef.current = -1;
    activeWordIdxRef.current = -1;
  }, [stem, t]);

  useEffect(() => {
    const sourceTime = activeSourceTimeRef.current;
    if (sourceTime === null) return;
    const nearestSegmentIdx = findSegmentAtOrAfter(rows, sourceTime);
    if (nearestSegmentIdx >= 0) {
      scrollTranscriptToIndex(virtualizer, nearestSegmentIdx);
      lastScrolledSegmentRef.current = nearestSegmentIdx;
    }
  }, [stem, rows, virtualizer]);

  // Pointer handling — supports both click-to-seek (no drag) and
  // drag-to-select (extends a word range). Event delegation off
  // the scroll container catches all word + segment events without
  // attaching thousands of handlers.
  //
  // Distinguishing click vs drag:
  //   - pointerdown on a word records the start word index
  //   - pointermove on another word marks didDrag and extends
  //     selection
  //   - pointerup commits: drag → selection persists; click → seek
  function onPointerDown(e: React.PointerEvent<HTMLDivElement>) {
    const wordIdx = wordIdxFromTarget(e.target as HTMLElement);
    if (wordIdx < 0) {
      // Maybe a segment-level click without word alignment — let
      // pointerup handle the seek.
      dragStartRef.current = null;
      return;
    }
    dragStartRef.current = wordIdx;
    didDragRef.current = false;
    setSelection({ stem, startWordIdx: wordIdx, endWordIdx: wordIdx });
    // Don't capture pointer here — we want to allow scroll while
    // dragging. The selection extends via the move handler firing
    // for events that pass through other word spans.
  }

  function onPointerMove(e: React.PointerEvent<HTMLDivElement>) {
    if (dragStartRef.current === null) return;
    const wordIdx = wordIdxFromTarget(e.target as HTMLElement);
    if (wordIdx < 0) return;
    if (wordIdx !== dragStartRef.current) didDragRef.current = true;
    const start = Math.min(dragStartRef.current, wordIdx);
    const end = Math.max(dragStartRef.current, wordIdx);
    setSelection({ stem, startWordIdx: start, endWordIdx: end });
  }

  function onPointerUp(e: React.PointerEvent<HTMLDivElement>) {
    const wasDrag = didDragRef.current;
    dragStartRef.current = null;
    didDragRef.current = false;
    if (wasDrag) return; // selection is the result; don't seek
    // Click — seek to the word/segment under the pointer.
    const target = (e.target as HTMLElement).closest(
      "[data-word-start], [data-segment-start]",
    ) as HTMLElement | null;
    if (!target) return;
    const sourceStr =
      target.getAttribute("data-word-start") ??
      target.getAttribute("data-segment-start");
    if (!sourceStr) return;
    const sourceTime = Number(sourceStr);
    if (!Number.isFinite(sourceTime)) return;
    // A click clears any prior selection (Descript behavior).
    setSelection(null);
    const tlTime =
      timelineTimeForSource(segments, stem, sourceTime) ??
      nearestTimelineTimeForSource(segments, stem, sourceTime);
    if (tlTime !== null) {
      requestTimelineSeek(tlTime);
    } else {
      requestSeek(sourceTime);
    }
  }

  // Imperative selection paint: walk word spans in [start, end] and
  // toggle `transcript-word-selected`. Cheap because the visible
  // word set is bounded by virtualization. Re-runs whenever
  // selection changes.
  const selectionStart =
    selection && selection.stem === stem ? selection.startWordIdx : -1;
  const selectionEnd =
    selection && selection.stem === stem ? selection.endWordIdx : -1;
  const selectedTranscriptText = useMemo(() => {
    if (selectionStart < 0 || selectionEnd < 0) return "";
    return t.words
      .slice(selectionStart, selectionEnd + 1)
      .map((word) => word.text)
      .join(" ")
      .replace(/\s+/g, " ")
      .trim();
  }, [selectionStart, selectionEnd, t.words]);
  useEffect(() => {
    const scope = scrollRef.current;
    if (!scope) return;
    // Clear any spans currently flagged.
    scope
      .querySelectorAll(".transcript-word-selected")
      .forEach((el) => el.classList.remove("transcript-word-selected"));
    if (selectionStart < 0 || selectionEnd < 0) return;
    for (let i = selectionStart; i <= selectionEnd; i++) {
      const el = scope.querySelector(`[data-word-idx="${i}"]`);
      if (el) el.classList.add("transcript-word-selected");
    }
  }, [selectionStart, selectionEnd]);

  // Delete-key handler: build an EDL envelope from the selected
  // range and fire propose_user_edit. The Step-5 ghost overlay
  // takes over on the timeline.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.code !== "Delete" && e.code !== "Backspace") return;
      // Don't intercept while typing in the composer.
      const tag = (e.target as HTMLElement | null)?.tagName?.toLowerCase();
      if (tag === "textarea" || tag === "input") return;
      // Only act on a non-empty selection bound to this transcript.
      if (selectionStart < 0 || selectionEnd < 0) return;
      e.preventDefault();
      const startWord = t.words[selectionStart];
      const endWord = t.words[selectionEnd];
      if (!startWord || !endWord) return;
      const ops = buildDeleteRangeOps({
        snapshot,
        stem,
        sourceStart: startWord.start_s,
        sourceEnd: endWord.end_s,
      });
      if (ops.length === 0) {
        // eslint-disable-next-line no-console
        console.warn(
          "transcript delete: selected range doesn't intersect any clip on the timeline; nothing to do",
        );
        return;
      }
      editorDispatch.proposeUserEdit(ops).catch((err: unknown) => {
        // eslint-disable-next-line no-console
        console.warn("propose_user_edit (transcript delete) failed", err);
      });
      setSelection(null);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectionStart, selectionEnd, snapshot, stem, t.words]);

  function requestVisualSupportForSelection() {
    if (!selectedTranscriptText) return;
    const startWord = t.words[selectionStart];
    const anchorTimelineStartS = startWord
      ? sourceTimeToTimeline(snapshot, stem, startWord.start_s)
      : undefined;
    editorDispatch
      .proposeVisualSupport({
        selectionText: selectedTranscriptText,
        request: "request visual support",
        anchorTranscript: selectedTranscriptText,
        anchorTimelineStartS,
      })
      .catch((err: unknown) => {
        // eslint-disable-next-line no-console
        console.warn("propose_visual_support (transcript selection) failed", err);
      });
    setSelection(null);
  }

  return (
    <div className="transcript-pane">
      <header className="transcript-meta">
        <span className="transcript-meta-lang">
          {t.language || "—"}
          {t.diarized ? " · diarized" : ""}
        </span>
        <span className="transcript-meta-right">
          {selectedTranscriptText ? (
            <button
              type="button"
              className="transcript-action-button"
              onClick={requestVisualSupportForSelection}
            >
              Visual support
            </button>
          ) : null}
          <span className="transcript-meta-counts">
            {t.segments.length} segments · {t.words.length} words
          </span>
        </span>
      </header>
      <div
        ref={scrollRef}
        className="transcript-scroll"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
      >
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
                  <SpeakerLabel stem={stem} speakerId={row.segment.speaker_id} />
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

function scrollTranscriptToIndex(
  virtualizer: ReturnType<typeof useVirtualizer<HTMLDivElement, Element>>,
  index: number,
): void {
  requestAnimationFrame(() => {
    virtualizer.scrollToIndex(index, {
      align: "center",
      behavior: "auto",
    });
  });
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

/** Click-to-rename speaker label. Diarization seeds clips with
 *  `SPEAKER_00`, `SPEAKER_01`, etc. The user clicks to swap them for
 *  human names ("Host", "Guest"), which we persist by rewriting the
 *  whisper sidecar via `rename_speaker`. Subsequent transcript reads
 *  see the new label everywhere. */
function SpeakerLabel({ stem, speakerId }: { stem: string; speakerId: string }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(speakerId);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  // Keep draft in sync if the parent's speakerId prop changes (e.g.,
  // a sibling rename just rewrote the sidecar and the transcript
  // refreshed).
  useEffect(() => {
    if (!editing) setDraft(speakerId);
  }, [speakerId, editing]);
  useEffect(() => {
    if (editing) inputRef.current?.focus();
  }, [editing]);

  async function commit() {
    const next = draft.trim();
    if (!next || next === speakerId) {
      setEditing(false);
      setDraft(speakerId);
      return;
    }
    setBusy(true);
    try {
      await invoke<number>("rename_speaker", {
        stem,
        oldId: speakerId,
        newId: next,
      });
      // Re-fetch so every segment picks up the new label. Backend
      // already invalidated its cache during rename; this clears the
      // frontend's byStem entry and forces a fresh read.
      await useTranscriptStore.getState().reload(stem);
    } catch (e) {
      // Revert to last-known-good label.
      // eslint-disable-next-line no-console
      console.warn("rename_speaker failed", e);
      setDraft(speakerId);
    } finally {
      setBusy(false);
      setEditing(false);
    }
  }

  if (!editing) {
    return (
      <button
        type="button"
        onClick={() => setEditing(true)}
        className="transcript-speaker transcript-speaker-button"
        title="Click to rename speaker"
      >
        {speakerId}
      </button>
    );
  }
  return (
    <input
      ref={inputRef}
      type="text"
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") {
          setDraft(speakerId);
          setEditing(false);
        }
      }}
      disabled={busy}
      className="transcript-speaker transcript-speaker-input"
      aria-label="Speaker name"
    />
  );
}

function fmt(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${sec.toString().padStart(2, "0")}`;
}

/** Imperative `classList.remove` on the previously-active span.
 *  Cheap — typical case is a single querySelector + classList op. */
function clearActive(scope: HTMLDivElement | null, idx: number): void {
  if (!scope || idx < 0) return;
  const el = scope.querySelector(`[data-word-idx="${idx}"]`);
  if (el) el.classList.remove("transcript-word-active");
}

/** Imperative `classList.add` on the new active span. */
function setActive(scope: HTMLDivElement | null, idx: number): void {
  if (!scope || idx < 0) return;
  const el = scope.querySelector(`[data-word-idx="${idx}"]`);
  if (el) el.classList.add("transcript-word-active");
}

/** Binary-search words[] for the index whose [start_s, end_s]
 *  covers `t`. Returns -1 when `t` falls in a gap (e.g. between
 *  two whisper-segmented utterances) or before the first word. */
function findWordAt(
  starts: number[],
  words: TranscriptWord[],
  t: number,
): number {
  if (words.length === 0) return -1;
  // Find the rightmost word whose start_s <= t.
  let lo = 0;
  let hi = words.length - 1;
  let best = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    if (starts[mid] <= t) {
      best = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  if (best < 0) return -1;
  // Confirm the candidate's range covers t (some sidecars have
  // gaps between consecutive words during silence).
  return t <= words[best].end_s + 0.05 ? best : -1;
}

/** Find which segment owns a given word index. Linear search is
 *  fine — we only call this on segment-change, not per frame. */
function findSegmentForWord(rows: SegmentRow[], wordIdx: number): number {
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i];
    if (
      wordIdx >= r.firstWordIdx &&
      wordIdx < r.firstWordIdx + r.words.length
    ) {
      return i;
    }
  }
  return -1;
}

function findSegmentAtOrAfter(rows: SegmentRow[], sourceTime: number): number {
  if (rows.length === 0 || !Number.isFinite(sourceTime)) return -1;
  let lo = 0;
  let hi = rows.length - 1;
  let best = rows.length - 1;
  while (lo <= hi) {
    const mid = Math.floor((lo + hi) / 2);
    const segment = rows[mid].segment;
    if (sourceTime <= segment.end_s) {
      best = mid;
      hi = mid - 1;
    } else {
      lo = mid + 1;
    }
  }
  return best;
}

/** Walk up from a pointer event target to the nearest [data-word-idx]
 *  ancestor and return the parsed word index, or -1 when no match. */
function wordIdxFromTarget(target: HTMLElement | null): number {
  if (!target) return -1;
  const span = target.closest("[data-word-idx]") as HTMLElement | null;
  if (!span) return -1;
  const v = span.getAttribute("data-word-idx");
  const idx = v === null ? NaN : Number(v);
  return Number.isFinite(idx) ? idx : -1;
}

/**
 * Build the EDL envelope to cut `[sourceStart, sourceEnd]` of asset
 * `stem` out of the timeline.
 *
 * v1 scope: handle the **range fully inside one clip** case (the
 * ~95% common case for transcript-delete). Multi-clip spans return
 * an empty op list — the caller logs a warning. Future commits
 * extend with the boundary-crossing cases from the Step 6 plan.
 *
 * The resulting envelope is `Split @ start; Split @ end; Delete
 * (middle piece)`. Naming: after Split A, the right piece is
 * `<original>-b`; after Split B, the right piece of *that* is
 * `<original>-b-b`. We delete the middle piece (`<original>-b`)
 * which now spans `[start, end]` in source-time. Step 8.1's
 * uuid-stamping path keeps the anchors unique.
 */
/** Map an asset-local source second to its absolute timeline second by
 *  finding the clip on a video track that references this stem and contains
 *  the source time: `track_start_s + (sourceS - clip source_start_s)`.
 *  Returns undefined when the source time isn't placed on the timeline (e.g.
 *  the selection sits in trimmed-out media), so callers can omit the anchor
 *  rather than guess program-start. */
function sourceTimeToTimeline(
  snapshot: TimelineSnapshot,
  stem: string,
  sourceS: number,
): number | undefined {
  for (const track of snapshot.tracks) {
    if (track.kind !== "video") continue;
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.proxy_path === null) continue;
      if (stemOf(item.proxy_path) !== stem) continue;
      const clipStart = item.source_start_s ?? 0;
      const clipEnd = clipStart + item.duration_s;
      if (sourceS >= clipStart - 0.01 && sourceS <= clipEnd + 0.01) {
        return item.track_start_s + (sourceS - clipStart);
      }
    }
  }
  return undefined;
}

function buildDeleteRangeOps(args: {
  snapshot: TimelineSnapshot;
  stem: string;
  sourceStart: number;
  sourceEnd: number;
}): EdlOp[] {
  const { snapshot, stem, sourceStart, sourceEnd } = args;
  if (!(sourceEnd > sourceStart + 0.05)) return []; // empty / inverted

  // Find every clip on a video track that references this asset and
  // contains the range. The proxy stem maps to a clip via the clip
  // item's proxy_path (which the backend resolves the same way the
  // transcript pane does).
  const candidates: {
    clipUuid: string;
    clipSourceStart: number;
    clipSourceEnd: number;
  }[] = [];
  for (const track of snapshot.tracks) {
    if (track.kind !== "video") continue;
    for (const item of track.items) {
      if (item.kind !== "clip") continue;
      if (item.proxy_path === null) continue;
      const itemStem = stemOf(item.proxy_path);
      if (itemStem !== stem) continue;
      const clipStart = item.source_start_s ?? 0;
      const clipEnd = clipStart + item.duration_s;
      candidates.push({
        clipUuid: item.clip_uuid,
        clipSourceStart: clipStart,
        clipSourceEnd: clipEnd,
      });
    }
  }
  if (candidates.length === 0) return [];

  // v1: pick the first clip that fully contains [sourceStart,
  // sourceEnd]. Multi-clip spans are deferred (return empty so the
  // caller surfaces a warning rather than emitting a broken EDL).
  const fullyInside = candidates.find(
    (c) =>
      c.clipSourceStart <= sourceStart + 0.01 &&
      c.clipSourceEnd >= sourceEnd - 0.01,
  );
  if (!fullyInside) return [];

  return [
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: fullyInside.clipUuid },
      atS: sourceStart,
    },
    // After the first split, the right piece carries source range
    // [sourceStart, clipSourceEnd]. Anchor the second split against
    // its uuid — but the right piece's uuid is freshly stamped by
    // apply_split, so we can't know it ahead of time. The
    // anchor-by-clip-name path the resolver uses ("name = original-
    // b") works since the parent's name is known. The resolver
    // also matches against awidat.clip_uuid, so giving it the
    // synthesized name as a clip_uuid value invokes the name-
    // match fallback branch on resolve_by_uuid. (Step 5's resolver
    // hardening + the clip-name fallback path covers this round-
    // trip.)
    {
      kind: "split_clip",
      anchor: { kind: "clip_uuid", uuid: nameAfterSplit(fullyInside.clipUuid) },
      atS: sourceEnd,
    },
    {
      kind: "delete_clip",
      anchor: { kind: "clip_uuid", uuid: nameAfterSplit(fullyInside.clipUuid) },
    },
  ];
}

/** After Split, the right piece's *name* is `<parent>-b`. The
 *  parent's display name is what came in (Step 8.1 stamps a fresh
 *  awidat.clip_uuid on the right piece, but the name-match
 *  fallback in the anchor resolver matches against this synthesized
 *  string). */
function nameAfterSplit(parent: string): string {
  return `${parent}-b`;
}

function stemOf(path: string): string {
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const file = slash >= 0 ? path.slice(slash + 1) : path;
  const dot = file.lastIndexOf(".");
  return dot > 0 ? file.slice(0, dot) : file;
}
