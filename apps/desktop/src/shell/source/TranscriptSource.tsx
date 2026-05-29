/**
 * TranscriptSource — Wave 4 W4.4. Transcript-first Source surface
 * for podcast / tutorial projects. The existing `<TranscriptView>`
 * does the heavy lifting (virtualized rows, word spans, click-to-seek,
 * drag-select, active-word highlight). This wrapper adds:
 *
 *   - A `Transcript | Video` sub-tab strip on top.
 *   - ⌘F inline search across visible word spans.
 *   - A floating "Propose delete" / "Mark must-keep" action bar that
 *     appears on a non-empty word selection.
 *   - A dim strikethrough on transcript ranges that the agent's
 *     accepted cuts have already removed from the timeline.
 *   - A yellow underline on user must-keep marks (per-project, local).
 *
 * Hot paths (search highlight, overlay paints) are imperative DOM
 * mutations: walk the rendered `[data-word-start]` spans and toggle
 * classes. This stays cheap because the virtualizer caps the live
 * span count to a few hundred even on multi-thousand-word transcripts.
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { cn } from "../../ui";
import { TranscriptView } from "../../transcript/TranscriptView";
import { useTranscriptStore } from "../../transcript/store";
import { useTranscriptAnnotations } from "../../transcript/annotations";
import { useProjectStore } from "../../app/state";
import { useTimelineStore } from "../../timeline/store";
import { useMediaStore } from "../../media/store";
import { usePlaySegments } from "../../timeline/usePlaySegments";
import { editorDispatch } from "../../editor/tauriDispatch";
import {
  buildDeleteRangeOpsForStem,
  isRangeCutFromTimeline,
} from "./transcriptSourceLogic";
import {
  useSourceFocus,
  useTranscriptFlashes,
} from "../../state/focusController";

type SubTab = "transcript" | "video";

export interface TranscriptSourceProps {
  /** Legacy video preview, reachable through the `Video` sub-tab. */
  videoSlot: ReactNode;
}

export function TranscriptSource({ videoSlot }: TranscriptSourceProps) {
  const [subTab, setSubTab] = useState<SubTab>("transcript");
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);

  // Wave 4 W4.6 — drive the sub-tab from the focus controller. The
  // controller bumps `subTabRequestId` on every `setSubTab` (even when
  // the value didn't change) so a "stay-on-video" click still re-flashes.
  const focusSubTab = useSourceFocus((s) => s.subTab);
  const focusSubTabRequestId = useSourceFocus((s) => s.subTabRequestId);
  const focusToast = useSourceFocus((s) => s.toast);
  useEffect(() => {
    if (focusSubTabRequestId === 0) return;
    setSubTab(focusSubTab);
    if (focusSubTab === "video") setSearchOpen(false);
  }, [focusSubTab, focusSubTabRequestId]);

  // ⌘F / Ctrl+F opens search while the transcript sub-tab is active.
  useEffect(() => {
    if (subTab !== "transcript") return;
    function onKey(e: KeyboardEvent) {
      const tag = (e.target as HTMLElement | null)?.tagName?.toLowerCase();
      if (tag === "input" || tag === "textarea") return;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
        e.preventDefault();
        setSearchOpen(true);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [subTab]);

  return (
    <div className="tx-source flex h-full w-full min-h-0 flex-col overflow-hidden bg-[var(--color-surface-panel)]">
      <SubTabStrip
        active={subTab}
        onChange={(t) => {
          setSubTab(t);
          if (t === "video") setSearchOpen(false);
        }}
        onOpenSearch={() => setSearchOpen(true)}
      />
      {subTab === "transcript" ? (
        <>
          {searchOpen && (
            <SearchBar
              query={query}
              onQueryChange={(q) => {
                setQuery(q);
                setCursor(0);
              }}
              cursor={cursor}
              onCursorChange={setCursor}
              onClose={() => {
                setSearchOpen(false);
                setQuery("");
              }}
            />
          )}
          <TranscriptSurface />
        </>
      ) : (
        <div className="relative flex-1 min-h-0 overflow-hidden">
          {videoSlot}
          {focusToast && <FocusToast kind={focusToast} />}
        </div>
      )}
    </div>
  );
}

/**
 * Wave 4 W4.6 — small floating tag rendered over the video preview when
 * the focus controller asks for a color or B-roll review surface that
 * isn't fully built out yet. We render the tag instead of the missing
 * UI so the user gets a clear "the controller heard you, the preview
 * piece is on its way" signal — silent no-op was the original failure
 * mode for these mediums.
 */
function FocusToast({ kind }: { kind: "before-after" | "insert-preview" }) {
  const label =
    kind === "before-after"
      ? "Before / After · coming soon"
      : "Insert preview · coming soon";
  return (
    <div
      role="status"
      aria-live="polite"
      className={cn(
        "absolute right-3 top-3 z-20 pointer-events-none select-none",
        "rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-card)] px-2 py-1",
        "text-[10px] font-semibold uppercase tracking-[0.08em]",
        "text-[var(--color-text-secondary)] shadow-md",
      )}
    >
      {label}
    </div>
  );
}

function SubTabStrip({
  active,
  onChange,
  onOpenSearch,
}: {
  active: SubTab;
  onChange: (t: SubTab) => void;
  onOpenSearch: () => void;
}) {
  return (
    <div
      className="tx-tabs flex h-7 shrink-0 items-center gap-4 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-3"
      role="tablist"
      aria-label="Source view mode"
    >
      {(["transcript", "video"] as const).map((tab) => {
        const isActive = tab === active;
        return (
          <button
            key={tab}
            type="button"
            role="tab"
            aria-selected={isActive}
            onClick={() => onChange(tab)}
            className={cn(
              "relative inline-flex h-7 items-center text-[10px] font-semibold uppercase tracking-[0.1em] transition-[color] duration-[120ms]",
              isActive
                ? "text-[var(--color-text-primary)]"
                : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]",
            )}
          >
            {tab === "transcript" ? "Transcript" : "Video"}
            {isActive && (
              <span
                aria-hidden
                className="absolute -bottom-px left-0 right-0 h-[2px] bg-[var(--color-brand)]"
              />
            )}
          </button>
        );
      })}
      {active === "transcript" && (
        <button
          type="button"
          onClick={onOpenSearch}
          title="Find in transcript (⌘F)"
          className="ml-auto text-[10px] uppercase tracking-[0.08em] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
        >
          Find
        </button>
      )}
    </div>
  );
}

function SearchBar({
  query,
  onQueryChange,
  cursor,
  onCursorChange,
  onClose,
}: {
  query: string;
  onQueryChange: (q: string) => void;
  cursor: number;
  onCursorChange: (idx: number) => void;
  onClose: () => void;
}) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  const [matchCount, setMatchCount] = useState(0);
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // Walk the visible word spans, toggle search-match classes, scroll
  // the cursor span into view. Re-runs whenever query or cursor change.
  useEffect(() => {
    const root = document.querySelector(".tx-surface");
    if (!root) return;
    root
      .querySelectorAll(".tx-search-match, .tx-search-match-cursor")
      .forEach((el) => {
        el.classList.remove("tx-search-match");
        el.classList.remove("tx-search-match-cursor");
      });
    if (query.trim().length === 0) {
      setMatchCount(0);
      return;
    }
    const lc = query.trim().toLowerCase();
    const hits = Array.from(
      root.querySelectorAll<HTMLElement>("[data-word-start]"),
    ).filter((el) => (el.textContent ?? "").toLowerCase().includes(lc));
    hits.forEach((el) => el.classList.add("tx-search-match"));
    setMatchCount(hits.length);
    if (hits.length > 0) {
      const safe = ((cursor % hits.length) + hits.length) % hits.length;
      hits[safe].classList.add("tx-search-match-cursor");
      hits[safe].scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }, [query, cursor]);

  return (
    <div className="tx-search flex h-8 shrink-0 items-center gap-2 border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] px-3">
      <input
        ref={inputRef}
        type="text"
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            onClose();
          } else if (e.key === "Enter" || e.key === "ArrowDown") {
            e.preventDefault();
            onCursorChange(cursor + 1);
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            onCursorChange(cursor - 1);
          }
        }}
        placeholder="Find in transcript…"
        className="flex-1 bg-transparent text-[12px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)]"
        aria-label="Find in transcript"
      />
      <span className="text-[10px] font-mono uppercase tracking-[0.08em] text-[var(--color-text-muted)]">
        {matchCount === 0
          ? query.trim().length > 0 ? "no matches" : ""
          : `${(((cursor % matchCount) + matchCount) % matchCount) + 1} / ${matchCount}`}
      </span>
      <button
        type="button"
        onClick={onClose}
        className="text-[10px] uppercase tracking-[0.08em] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
        aria-label="Close search"
      >
        Close
      </button>
    </div>
  );
}

function TranscriptSurface() {
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const proxies = useMediaStore((s) => s.proxies);
  const playSegments = usePlaySegments();
  const stem = useMemo<string | null>(() => {
    if (selectedStem) return selectedStem;
    if (playSegments.length > 0 && playSegments[0].proxyStem) {
      return playSegments[0].proxyStem;
    }
    return proxies[0]?.stem ?? null;
  }, [selectedStem, proxies, playSegments]);

  const surfaceRef = useRef<HTMLDivElement | null>(null);
  const selection = useTranscriptStore((s) => s.selection);
  const project = useProjectStore((s) => s.current);
  const addAnnotation = useTranscriptAnnotations((s) => s.add);
  const transcript = useTranscriptStore((s) =>
    stem ? s.byStem[stem] : undefined,
  );

  // Right-click on the selection commits a must-keep mark. We capture
  // at the surface level so virtualizer remounts don't lose the
  // handler.
  const onContextMenu = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const sel = useTranscriptStore.getState().selection;
      if (
        !project ||
        !stem ||
        !sel ||
        sel.stem !== stem ||
        transcript?.state !== "loaded"
      ) {
        return;
      }
      const startWord = transcript.transcript.words[sel.startWordIdx];
      const endWord = transcript.transcript.words[sel.endWordIdx];
      if (!startWord || !endWord) return;
      e.preventDefault();
      addAnnotation(project, {
        stem,
        start_s: startWord.start_s,
        end_s: endWord.end_s,
      });
      useTranscriptStore.getState().setSelection(null);
    },
    [project, stem, transcript, addAnnotation],
  );

  return (
    <div
      ref={surfaceRef}
      className="tx-surface relative flex-1 min-h-0 overflow-hidden"
      onContextMenu={onContextMenu}
    >
      <TranscriptView stem={stem} />
      <WordOverlays stem={stem} surfaceRef={surfaceRef} />
      <FocusFlashOverlay surfaceRef={surfaceRef} />
      {stem && selection && selection.stem === stem && (
        <SelectionActions stem={stem} surfaceRef={surfaceRef} />
      )}
    </div>
  );
}

/**
 * Wave 4 W4.6 — listens to the focus controller's transcript-flash
 * store and scrolls the first matching `[data-word-start]` span into
 * view + flashes every span inside the range. The flash is purely
 * cosmetic; the underlying selection / annotation state isn't touched.
 */
function FocusFlashOverlay({
  surfaceRef,
}: {
  surfaceRef: React.RefObject<HTMLDivElement | null>;
}) {
  const flashes = useTranscriptFlashes((s) => s.flashes);
  useEffect(() => {
    const root = surfaceRef.current;
    if (!root) return;
    // Clear stale flash classes before re-applying so a removed range
    // doesn't leave a permanent glow on the spans it touched.
    root
      .querySelectorAll(".tx-focus-flash")
      .forEach((el) => el.classList.remove("tx-focus-flash"));
    if (flashes.length === 0) return;
    // Wait for the next frame so the virtualizer has had a chance to
    // render the words at the target time after a recent stem swap.
    const id = requestAnimationFrame(() => {
      const spans = Array.from(
        root.querySelectorAll<HTMLElement>("[data-word-start]"),
      );
      if (spans.length === 0) return;
      let firstHit: HTMLElement | null = null;
      for (const flash of flashes) {
        for (const span of spans) {
          const startS = Number(span.getAttribute("data-word-start"));
          const endS = Number(span.getAttribute("data-word-end") ?? startS);
          if (!Number.isFinite(startS) || !Number.isFinite(endS)) continue;
          // Overlap test (inclusive on the start side, exclusive on the
          // end side — matches the audio-cue convention used elsewhere).
          if (endS >= flash.startS && startS <= flash.endS) {
            span.classList.add("tx-focus-flash");
            if (!firstHit) firstHit = span;
          }
        }
      }
      firstHit?.scrollIntoView({ block: "center", behavior: "smooth" });
    });
    return () => cancelAnimationFrame(id);
  }, [flashes, surfaceRef]);
  return null;
}

/** Paints `tx-applied-cut` (agent's accepted cuts) + `tx-must-keep`
 *  (user's local annotations) on the rendered word spans. One pass
 *  per dependency change; the virtualizer keeps the mounted span
 *  count bounded so this stays cheap. */
function WordOverlays({
  stem,
  surfaceRef,
}: {
  stem: string | null;
  surfaceRef: React.RefObject<HTMLDivElement | null>;
}) {
  const snapshot = useTimelineStore((s) => s.snapshot);
  const transcript = useTranscriptStore((s) =>
    stem ? s.byStem[stem] : undefined,
  );
  const project = useProjectStore((s) => s.current);
  const marks = useTranscriptAnnotations((s) =>
    stem && project
      ? (s.byProject[project]?.filter((m) => m.stem === stem) ?? [])
      : [],
  );

  useEffect(() => {
    const root = surfaceRef.current;
    if (!root) return;
    root
      .querySelectorAll(".tx-must-keep, .tx-applied-cut")
      .forEach((el) => {
        el.classList.remove("tx-must-keep");
        el.classList.remove("tx-applied-cut");
      });
    if (!stem || transcript?.state !== "loaded") return;
    const id = requestAnimationFrame(() => {
      root.querySelectorAll<HTMLElement>("[data-word-start]").forEach((el) => {
        const startS = Number(el.getAttribute("data-word-start"));
        const endS = Number(el.getAttribute("data-word-end") ?? startS);
        if (!Number.isFinite(startS) || !Number.isFinite(endS)) return;
        if (isRangeCutFromTimeline(snapshot, stem, startS, endS)) {
          el.classList.add("tx-applied-cut");
        }
        if (
          marks.length > 0 &&
          marks.some(
            (m) => startS >= m.start_s - 0.01 && startS <= m.end_s + 0.01,
          )
        ) {
          el.classList.add("tx-must-keep");
        }
      });
    });
    return () => cancelAnimationFrame(id);
  }, [snapshot, stem, transcript, marks, surfaceRef]);

  return null;
}

function SelectionActions({
  stem,
  surfaceRef,
}: {
  stem: string;
  surfaceRef: React.RefObject<HTMLDivElement | null>;
}) {
  const selection = useTranscriptStore((s) => s.selection);
  const setSelection = useTranscriptStore((s) => s.setSelection);
  const transcript = useTranscriptStore((s) =>
    s.byStem[stem]?.state === "loaded" ? s.byStem[stem] : undefined,
  );
  const snapshot = useTimelineStore((s) => s.snapshot);
  const project = useProjectStore((s) => s.current);
  const addAnnotation = useTranscriptAnnotations((s) => s.add);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(
    null,
  );

  // Word range → source-time. Word-level fallback: when the transcript
  // has segments but no aligned words[], `words[idx]` is undefined and
  // we render nothing (selection couldn't have happened either).
  const range = useMemo(() => {
    if (!selection || selection.stem !== stem || transcript?.state !== "loaded") {
      return null;
    }
    const t = transcript.transcript;
    const startWord = t.words[selection.startWordIdx];
    const endWord = t.words[selection.endWordIdx];
    if (!startWord || !endWord) return null;
    return { start_s: startWord.start_s, end_s: endWord.end_s };
  }, [selection, stem, transcript]);

  // Paint the dotted-red strike-through on every word in the
  // selection, and anchor the floating action bar to the last word.
  useEffect(() => {
    const root = surfaceRef.current;
    if (!root) return;
    root
      .querySelectorAll(".tx-strike")
      .forEach((el) => el.classList.remove("tx-strike"));
    if (!selection || selection.stem !== stem) {
      setPosition(null);
      return;
    }
    const id = requestAnimationFrame(() => {
      for (let i = selection.startWordIdx; i <= selection.endWordIdx; i++) {
        root
          .querySelector<HTMLElement>(`[data-word-idx="${i}"]`)
          ?.classList.add("tx-strike");
      }
      const target = root.querySelector<HTMLElement>(
        `[data-word-idx="${selection.endWordIdx}"]`,
      );
      if (!target) {
        setPosition(null);
        return;
      }
      const rootRect = root.getBoundingClientRect();
      const wordRect = target.getBoundingClientRect();
      setPosition({
        left: Math.max(8, wordRect.left - rootRect.left),
        top: wordRect.bottom - rootRect.top + 6,
      });
    });
    return () => cancelAnimationFrame(id);
  }, [selection, stem, surfaceRef]);

  const onProposeDelete = useCallback(async () => {
    if (!range) return;
    setBusy(true);
    setError(null);
    try {
      const ops = buildDeleteRangeOpsForStem({
        snapshot,
        stem,
        sourceStart: range.start_s,
        sourceEnd: range.end_s,
      });
      if (ops.length === 0) {
        setError("Range isn't on the timeline yet.");
        return;
      }
      await editorDispatch.proposeUserEdit(ops);
      setSelection(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [range, snapshot, stem, setSelection]);

  const onMarkMustKeep = useCallback(() => {
    if (!range || !project) return;
    addAnnotation(project, { stem, start_s: range.start_s, end_s: range.end_s });
    setSelection(null);
  }, [range, project, addAnnotation, stem, setSelection]);

  if (!range || !position) return null;
  return (
    <div
      className="tx-action-bar"
      style={{ left: position.left, top: position.top }}
      role="toolbar"
      aria-label="Selection actions"
    >
      <button
        type="button"
        onClick={onProposeDelete}
        disabled={busy}
        className="tx-action-button tx-action-delete"
      >
        Propose delete
      </button>
      <button
        type="button"
        onClick={onMarkMustKeep}
        className="tx-action-button tx-action-keep"
        title="Mark this range as must-keep so the agent avoids cutting it"
      >
        Mark must-keep
      </button>
      {error && <span className="tx-action-error">{error}</span>}
    </div>
  );
}
