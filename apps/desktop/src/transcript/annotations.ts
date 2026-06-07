/**
 * useTranscriptAnnotations — per-project "must-keep" markers (Wave 4
 * W4.4). Annotations are LOCAL hints: they don't generate proposals,
 * don't touch the EDL, don't round-trip the backend. The transcript
 * surface paints a yellow underline on marked ranges; the agent
 * picks them up via `contextLine()` on its next turn so it avoids
 * cutting them.
 *
 * Persistence: per-project, in localStorage. Keyed by project path so
 * a switch loads the right marks. Quota errors swallowed — worst case
 * the user re-marks next session.
 *
 * Why a separate store (not co-located with `useTranscriptStore`):
 * transcript content has its own lifecycle (clearCache on indexer
 * complete) that must NOT wipe annotations.
 */

import { create } from "zustand";

/** One must-keep range. `stem` anchors to a source asset; `start_s`
 *  / `end_s` are source-time seconds. */
export interface MustKeepRange {
  stem: string;
  start_s: number;
  end_s: number;
  /** ms epoch — sort key for context rendering. */
  markedAt: number;
}

export const STORAGE_KEY = "montage:transcript-annotations";

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

interface AnnotationsState {
  /** project path → list of marks. */
  byProject: Record<string, MustKeepRange[]>;
  /** Append a new must-keep range to `project`. De-dupes by stem +
   *  near-identical (<50ms tolerance) start/end. */
  add: (project: string | null, range: Omit<MustKeepRange, "markedAt">) => void;
  /** Drop a range matching `stem` + `[start_s, end_s]`. */
  remove: (project: string | null, stem: string, startS: number, endS: number) => void;
  /** Build the agent context line ("user marked … as must-keep: …").
   *  Returns null when there are no marks. */
  contextLine: (project: string | null) => string | null;
}

function loadInitial(storage: StorageAdapter | null): Record<string, MustKeepRange[]> {
  if (!storage) return {};
  const raw = storage.getItem(STORAGE_KEY);
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: Record<string, MustKeepRange[]> = {};
    for (const [project, marks] of Object.entries(parsed as Record<string, unknown>)) {
      if (typeof project !== "string" || !Array.isArray(marks)) continue;
      const valid = marks.filter(isMustKeepRange);
      if (valid.length > 0) out[project] = valid;
    }
    return out;
  } catch {
    return {};
  }
}

function isMustKeepRange(m: unknown): m is MustKeepRange {
  if (!m || typeof m !== "object") return false;
  const r = m as Record<string, unknown>;
  return (
    typeof r.stem === "string" &&
    typeof r.start_s === "number" &&
    typeof r.end_s === "number" &&
    typeof r.markedAt === "number" &&
    r.end_s > r.start_s
  );
}

function persistMap(
  storage: StorageAdapter | null,
  map: Record<string, MustKeepRange[]>,
): void {
  if (!storage) return;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(map));
  } catch {
    /* quota — non-fatal */
  }
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
}

/** Factory exported for tests. */
export function createAnnotationsStore(opts: CreateOpts = {}) {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? (opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null))
    : null;

  return create<AnnotationsState>((set, get) => ({
    byProject: loadInitial(storage),
    add(project, range) {
      if (!project) return;
      set((state) => {
        const existing = state.byProject[project] ?? [];
        const filtered = existing.filter(
          (m) =>
            !(
              m.stem === range.stem &&
              Math.abs(m.start_s - range.start_s) < 0.05 &&
              Math.abs(m.end_s - range.end_s) < 0.05
            ),
        );
        const next: MustKeepRange[] = [
          ...filtered,
          { ...range, markedAt: Date.now() },
        ];
        const nextMap = { ...state.byProject, [project]: next };
        persistMap(storage, nextMap);
        return { byProject: nextMap };
      });
    },
    remove(project, stem, startS, endS) {
      if (!project) return;
      set((state) => {
        const existing = state.byProject[project];
        if (!existing) return state;
        const next = existing.filter(
          (m) =>
            !(
              m.stem === stem &&
              Math.abs(m.start_s - startS) < 0.05 &&
              Math.abs(m.end_s - endS) < 0.05
            ),
        );
        const nextMap =
          next.length > 0
            ? { ...state.byProject, [project]: next }
            : (() => {
                const { [project]: _drop, ...rest } = state.byProject;
                return rest;
              })();
        persistMap(storage, nextMap);
        return { byProject: nextMap };
      });
    },
    contextLine(project) {
      if (!project) return null;
      const all = get().byProject[project];
      if (!all || all.length === 0) return null;
      const sorted = [...all].sort(
        (a, b) => a.stem.localeCompare(b.stem) || a.start_s - b.start_s,
      );
      const fragments = sorted.map(
        (m) => `${m.stem}:${fmtT(m.start_s)}-${fmtT(m.end_s)}`,
      );
      return `user marked these ranges as must-keep: ${fragments.join(", ")}`;
    },
  }));
}

function fmtT(s: number): string {
  if (!Number.isFinite(s) || s < 0) return "0:00";
  const m = Math.floor(s / 60);
  const sec = (s % 60).toFixed(1);
  return `${m}:${sec.padStart(4, "0")}`;
}

export const useTranscriptAnnotations = createAnnotationsStore();
