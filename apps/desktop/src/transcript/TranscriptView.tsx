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

import { useEffect, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useTranscriptStore } from "./store";
import { useMediaStore } from "../media/store";
import { useTimelineStore, type TimelineSnapshot } from "../timeline/store";
import {
  findActiveSegment,
  nearestTimelineTimeForSource,
  timelineTimeForSource,
  usePlaySegments,
} from "../timeline/usePlaySegments";
import { serializeEdl, type EdlOp } from "../timeline/edlBuilder";
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
      const edl = serializeEdl(ops);
      invoke<string>("propose_user_edit", { edlText: edl }).catch(
        (err: unknown) => {
          // eslint-disable-next-line no-console
          console.warn("propose_user_edit (transcript delete) failed", err);
        },
      );
      setSelection(null);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectionStart, selectionEnd, snapshot, stem, t.words]);

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
