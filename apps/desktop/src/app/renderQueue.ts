// Render queue store for the Deliver tab.
//
// Captures the user-side intent — "I want these N targets exported" —
// and tracks each entry's lifecycle (pending → running → done/failed).
// Sequenced; one render at a time so ffmpeg doesn't fight itself.
//
// Persistence: queued + running + done entries are written to
// localStorage so a reload keeps history. The runtime worker (in
// useRenderQueueWorker) reconciles state on app boot — anything
// stuck in `running` from a prior session gets demoted to `failed`
// because we have no way to recover progress mid-render.
//
// This store does NOT invoke any Tauri commands itself. The worker
// hook drives the actual `start_timeline_render` / `start_reframe_render`
// / `export_caption_sidecars` / `export_still` calls and reports back
// via the store's `markRunning` / `markProgress` / `markDone` /
// `markFailed` actions.

import { create } from "zustand";

/** A queue entry — one selected target for one Deliver action. */
export type RenderTargetKind =
  | "video_master"
  | "video_reframe"
  | "captions"
  | "still";

/**
 * Per-target upload lifecycle. Mirrors the Rust `UploadState` enum
 * shape returned by `poll_upload_states` — `state` is the tag, the
 * other fields appear when the variant carries them.
 *
 *   - `pending`    — render done, upload not started yet
 *   - `uploading`  — in flight; `progress` is 0..1 (or NaN unknown)
 *   - `published`  — provider accepted; `remote_url` is shareable
 *   - `failed`     — terminal failure; `reason` is shown verbatim
 */
export type RenderUploadState =
  | { state: "pending" }
  | { state: "uploading"; progress: number }
  | { state: "published"; remote_url: string; remote_id: string }
  | { state: "failed"; reason: string };

export type RenderQueueEntry = {
  /** Stable id; survives reload. */
  id: string;
  /** Which Deliver target produced this entry. */
  targetId: string;
  /** Display name for the queue row ("YouTube 1080p", "TikTok 9:16"). */
  label: string;
  /** What kind of artifact this entry produces. */
  kind: RenderTargetKind;
  /** Lifecycle. */
  status: "pending" | "running" | "done" | "failed" | "cancelled";
  /** 0–100 while running; undefined otherwise. */
  progress?: number;
  /** Absolute output path once known. */
  outputPath?: string;
  /** Error message when status === "failed". */
  error?: string;
  /** Human review state after an artifact finishes exporting. */
  reviewStatus?: "pending" | "approved" | "changes_requested";
  /** When this entry was enqueued (epoch ms). */
  enqueuedAt: number;
  /** When this entry transitioned to a terminal state (epoch ms). */
  completedAt?: number;
  /**
   * Tauri job id, set when the worker has called the backend. Used
   * by the cancel button to address the right render. Empty for kinds
   * that don't run as background jobs (still / captions write
   * synchronously).
   */
  jobId?: string;
  /**
   * For video_reframe entries: the master path the reframe consumes.
   * Populated by the worker when the master_video entry completes.
   */
  reframeMasterPath?: string;
  /**
   * For video_reframe entries: target width/height/bitrate. The
   * worker passes these straight to `start_reframe_render`.
   */
  reframeWidth?: number;
  reframeHeight?: number;
  reframeBitrateKbps?: number;
  /**
   * For still entries: asset path + timecode.
   */
  stillAssetPath?: string;
  stillTimecodeS?: number;
  stillKind?: "cover" | "custom";
  /**
   * Provider keys (`"youtube"`, `"tiktok"`, `"instagram"`) the user
   * has opted into auto-publishing to once this render lands at
   * `done`. Empty (or undefined) → no auto-upload.
   */
  uploadTargets?: string[];
  /**
   * Per-target lifecycle, keyed by provider key. Populated when the
   * worker registers targets with the backend and updated by the
   * upload poller. Mirrors the backend `UploadJobEntry.upload_states`.
   */
  uploadStates?: Record<string, RenderUploadState>;
  /**
   * Convenience mirror of `uploadStates[*].remote_url` so the row can
   * cheap-fetch link hrefs without pattern-matching. Empty until at
   * least one target publishes.
   */
  publishedUrls?: Record<string, string>;
};

type State = {
  entries: RenderQueueEntry[];
  enqueue: (entries: RenderQueueEntry[]) => void;
  markRunning: (id: string, jobId?: string) => void;
  markProgress: (id: string, percent: number) => void;
  markDone: (id: string, outputPath?: string) => void;
  markReviewed: (
    id: string,
    reviewStatus: NonNullable<RenderQueueEntry["reviewStatus"]>,
  ) => void;
  markFailed: (id: string, error: string) => void;
  markCancelled: (id: string) => void;
  /** Remove a terminal entry from the visible queue. */
  dismiss: (id: string) => void;
  /** Clear every terminal entry. */
  clearTerminal: () => void;
  /** Set this entry's upload targets (provider keys). */
  setUploadTargets: (id: string, providers: string[]) => void;
  /**
   * Replace this entry's per-target upload state map. Called by the
   * upload poller after each backend `poll_upload_states` round trip;
   * mirrors the wire shape verbatim so the row can render directly.
   */
  setUploadStates: (
    id: string,
    states: Record<string, RenderUploadState>,
    publishedUrls: Record<string, string>,
  ) => void;
};

const PERSIST_KEY = "awidat.deliver.renderQueue.v1";

function loadPersisted(): RenderQueueEntry[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(PERSIST_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as RenderQueueEntry[];
    if (!Array.isArray(parsed)) return [];
    // Anything that was 'running' at last write is now stale — we
    // can't reattach to a backend job after a reload. Demote to
    // failed so the UI shows it terminated rather than hung.
    return parsed.map((e) =>
      e.status === "running"
        ? {
            ...e,
            status: "failed" as const,
            error: "interrupted by app reload",
            completedAt: Date.now(),
          }
        : e,
    );
  } catch {
    return [];
  }
}

function persist(entries: RenderQueueEntry[]): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(PERSIST_KEY, JSON.stringify(entries));
  } catch {
    // localStorage may be disabled (private window). Ignore.
  }
}

export const useRenderQueueStore = create<State>((set) => ({
  entries: loadPersisted(),
  enqueue: (newEntries) => {
    set((state) => {
      const next = [...state.entries, ...newEntries];
      persist(next);
      return { entries: next };
    });
  },
  markRunning: (id, jobId) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id ? { ...e, status: "running" as const, jobId } : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  markProgress: (id, percent) => {
    // Skip persistence for progress ticks — too chatty for localStorage.
    set((state) => ({
      entries: state.entries.map((e) =>
        e.id === id ? { ...e, progress: percent } : e,
      ),
    }));
  },
  markDone: (id, outputPath) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id
          ? {
              ...e,
              status: "done" as const,
              progress: 100,
              outputPath: outputPath ?? e.outputPath,
              reviewStatus: "pending" as const,
              completedAt: Date.now(),
            }
          : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  markReviewed: (id, reviewStatus) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id ? { ...e, reviewStatus } : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  markFailed: (id, error) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id
          ? {
              ...e,
              status: "failed" as const,
              error,
              completedAt: Date.now(),
            }
          : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  markCancelled: (id) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id
          ? {
              ...e,
              status: "cancelled" as const,
              completedAt: Date.now(),
            }
          : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  dismiss: (id) => {
    set((state) => {
      const next = state.entries.filter((e) => e.id !== id);
      persist(next);
      return { entries: next };
    });
  },
  clearTerminal: () => {
    const terminal = new Set<RenderQueueEntry["status"]>([
      "done",
      "failed",
      "cancelled",
    ]);
    set((state) => {
      const next = state.entries.filter((e) => !terminal.has(e.status));
      persist(next);
      return { entries: next };
    });
  },
  setUploadTargets: (id, providers) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id
          ? {
              ...e,
              uploadTargets: providers,
              uploadStates: Object.fromEntries(
                providers.map((p) => [
                  p,
                  { state: "pending" as const },
                ]),
              ),
              publishedUrls: {},
            }
          : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  setUploadStates: (id, states, publishedUrls) => {
    // Per-progress ticks would be chatty in localStorage — we still
    // persist because terminal transitions matter for reload reconcile.
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id
          ? { ...e, uploadStates: states, publishedUrls }
          : e,
      );
      persist(next);
      return { entries: next };
    });
  },
}));

/** Selector helpers. */
export const renderQueueSelectors = {
  pending: (s: State) => s.entries.filter((e) => e.status === "pending"),
  running: (s: State) => s.entries.filter((e) => e.status === "running"),
  done: (s: State) => s.entries.filter((e) => e.status === "done"),
  failed: (s: State) =>
    s.entries.filter((e) => e.status === "failed" || e.status === "cancelled"),
  active: (s: State) =>
    s.entries.filter((e) => e.status === "pending" || e.status === "running"),
};

/** Make a new queue id. Deterministic prefix so log entries are
 *  trivially recognizable. */
export function newQueueId(prefix: string): string {
  const ts = Date.now().toString(36);
  const rnd = Math.random().toString(36).slice(2, 8);
  return `${prefix}-${ts}-${rnd}`;
}
