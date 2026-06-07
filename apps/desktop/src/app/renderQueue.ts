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
import type { AiDisclosure } from "../state/aiDisclosure";
import type { UploadMetadata } from "../state/uploadMetadata";

/** A queue entry — one selected target for one Deliver action. */
export type RenderTargetKind =
  | "video_master"
  | "video_reframe"
  | "captions"
  | "still";

export type RenderProgressPhase =
  | "rendering_source"
  | "rendering_target";

export type RenderProgressDetails = {
  phase?: RenderProgressPhase;
  etaS?: number | null;
  timeDoneS?: number | null;
  logExcerpt?: string | null;
};

/**
 * Per-target upload lifecycle for the server-backed social publish
 * job created after a render lands. `state` is the tag; the other
 * fields appear when the variant carries them.
 *
 *   - `pending`    — render done, upload not started yet
 *   - `uploading`  — in flight; `progress` is 0..1 (or NaN unknown)
 *   - `published`  — provider accepted; `remote_url` appears when shareable
 *   - `failed`     — terminal failure; `reason` is shown verbatim
 */
export type RenderUploadState =
  | { state: "pending" }
  | { state: "uploading"; progress: number }
  | { state: "scheduled"; job_id: string; scheduled_for?: number }
  | { state: "processing"; job_id: string }
  | { state: "published"; remote_url?: string; remote_id: string }
  | { state: "failed"; reason: string; job_id?: string };

export type UploadTargetActions = {
  canRefresh: boolean;
  canRetry: boolean;
  canCancel: boolean;
  canReschedule: boolean;
  canOpenProviderUrl: boolean;
};

export type UploadTargetRetryMode = "server_job" | "republish";

export function deriveUploadTargetActions(
  state: RenderUploadState,
): UploadTargetActions {
  switch (state.state) {
    case "scheduled":
      return {
        canRefresh: true,
        canRetry: false,
        canCancel: true,
        canReschedule: true,
        canOpenProviderUrl: false,
      };
    case "processing":
      return {
        canRefresh: true,
        canRetry: false,
        canCancel: true,
        canReschedule: false,
        canOpenProviderUrl: false,
      };
    case "published":
      return {
        canRefresh: false,
        canRetry: false,
        canCancel: false,
        canReschedule: false,
        canOpenProviderUrl: Boolean(state.remote_url),
      };
    case "failed":
      return {
        canRefresh: Boolean(state.job_id),
        canRetry: true,
        canCancel: false,
        canReschedule: false,
        canOpenProviderUrl: false,
      };
    case "pending":
    case "uploading":
      return {
        canRefresh: false,
        canRetry: false,
        canCancel: false,
        canReschedule: false,
        canOpenProviderUrl: false,
      };
  }
}

export function deriveUploadTargetRetryMode(
  state: RenderUploadState,
  hasRenderOutput: boolean,
): UploadTargetRetryMode | null {
  if (state.state !== "failed") return null;
  if (state.job_id) return "server_job";
  return hasRenderOutput ? "republish" : null;
}

function hasActiveUpload(states?: Record<string, RenderUploadState>): boolean {
  return Object.values(states ?? {}).some((state) =>
    ["pending", "uploading", "scheduled", "processing"].includes(state.state),
  );
}

function hasFailedUpload(states?: Record<string, RenderUploadState>): boolean {
  return Object.values(states ?? {}).some((state) => state.state === "failed");
}

export type RenderQueueEntry = {
  /** Stable id; survives reload. */
  id: string;
  /** Which Deliver target produced this entry. */
  targetId: string;
  /** Display name for the queue row ("YouTube 1080p", "TikTok 9:16"). */
  label: string;
  /** What kind of artifact this entry produces. */
  kind: RenderTargetKind;
  /** Internal dependency render, not a user-selected delivery target. */
  internal?: boolean;
  /** Hidden source render that must complete before this visible entry can run. */
  sourceEntryId?: string;
  /** Lifecycle. */
  status: "pending" | "running" | "done" | "failed" | "cancelled";
  /** 0–100 while running; undefined otherwise. */
  progress?: number;
  /** More detail from live render polling; not persisted per tick. */
  progressPhase?: RenderProgressPhase;
  progressEtaS?: number | null;
  progressTimeDoneS?: number | null;
  progressLogExcerpt?: string | null;
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
   * Concrete connected account choices for upload targets. Keyed by
   * provider key; absent key means the server publish helper may use
   * its provider-level fallback account.
   */
  uploadAccountIds?: Record<string, string>;
  /**
   * Per-target lifecycle, keyed by provider key. Populated when the
   * worker registers targets with the social backend.
   */
  uploadStates?: Record<string, RenderUploadState>;
  /**
   * Convenience mirror of `uploadStates[*].remote_url` so the row can
   * cheap-fetch link hrefs without pattern-matching. Empty until at
   * least one target publishes.
   */
  publishedUrls?: Record<string, string>;
  /**
   * Per-target metadata (title / description / tags / visibility /
   * schedule / thumbnail) the user configured before kicking the
   * render. Keyed by provider key — same set as `uploadTargets`.
   *
   * The render-queue worker forwards each entry through the social
   * server-backed publish commands. Missing keys (or `undefined`) →
   * publishing falls back to the
   * default `(label, no description, private)` payload.
   */
  uploadMetadata?: Record<string, UploadMetadata>;
  /**
   * AI disclosure (W5.A4) — computed at register time by the backend
   * walking the project timeline against the generated-media
   * registry. When `has_synthetic_content` is true the upload
   * dispatcher folds the disclosure intent into each platform's flag
   * (YouTube `alteredContent`, TikTok `aigc_label`, IG `ai_label`).
   *
   * `undefined` means "not computed yet" — clean cuts where the
   * dispatcher never asked. The UI treats undefined the same as "no
   * synthetic content" for display so a missing disclosure doesn't
   * surface a banner on cuts that never contained generated media.
   */
  aiDisclosure?: AiDisclosure;
};

export function renderQueueVisibleEntries(
  entries: RenderQueueEntry[],
): RenderQueueEntry[] {
  const activeTargets = new Set<string>();
  const visible: RenderQueueEntry[] = [];
  for (const entry of entries) {
    if (entry.internal) continue;
    if (isNonterminal(entry)) {
      if (activeTargets.has(entry.targetId)) continue;
      activeTargets.add(entry.targetId);
    }
    visible.push(entry);
  }
  return visible;
}

function sourceEntryFor(
  entry: RenderQueueEntry,
  entries?: RenderQueueEntry[],
): RenderQueueEntry | null {
  if (!entry.sourceEntryId || !entries) return null;
  return entries.find((candidate) => candidate.id === entry.sourceEntryId) ?? null;
}

export function renderQueueStatusLabel(
  entry: RenderQueueEntry,
  entries?: RenderQueueEntry[],
): string {
  const source = sourceEntryFor(entry, entries);
  if (
    entry.status === "pending" &&
    source &&
    (source.status === "pending" || source.status === "running")
  ) {
    return "Preparing source";
  }
  if (entry.status === "done" && hasActiveUpload(entry.uploadStates)) {
    return "Publishing";
  }
  if (entry.status === "done" && hasFailedUpload(entry.uploadStates)) {
    return "Needs action";
  }
  if (entry.status === "done") {
    return entry.reviewStatus === "pending" ? "Review" : "Done";
  }
  if (entry.status === "cancelled") return "Cancelled";
  if (entry.status === "failed") return "Failed";
  if (entry.status === "pending") return "Queued";
  return "Running";
}

function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0s";
  const rounded = Math.round(seconds);
  if (rounded < 60) return `${rounded}s`;
  const minutes = Math.floor(rounded / 60);
  const remainingSeconds = rounded % 60;
  if (remainingSeconds === 0) return `${minutes}m`;
  return `${minutes}m ${remainingSeconds}s`;
}

function progressPercent(entry: RenderQueueEntry): number | null {
  if (typeof entry.progress !== "number" || !Number.isFinite(entry.progress)) {
    return null;
  }
  return Math.max(0, Math.min(100, Math.round(entry.progress)));
}

function progressPhaseLabel(entry: RenderQueueEntry): string {
  if (entry.progressPhase === "rendering_source") return "Rendering source";
  if (entry.progressPhase === "rendering_target") {
    return entry.targetId === "tiktok"
      ? "Rendering TikTok format"
      : `Rendering ${entry.label}`;
  }
  if (entry.kind === "video_reframe") {
    return entry.targetId === "tiktok"
      ? "Rendering TikTok format"
      : `Rendering ${entry.label}`;
  }
  if (entry.internal || entry.kind === "video_master") return "Rendering source";
  return "Rendering";
}

export function renderQueueProgressCopy(
  entry: RenderQueueEntry,
  entries?: RenderQueueEntry[],
): string | null {
  const source = sourceEntryFor(entry, entries);
  if (
    entry.status === "pending" &&
    source &&
    (source.status === "pending" || source.status === "running")
  ) {
    return renderQueueProgressCopy(source) ?? "Waiting for source render";
  }
  if (entry.status !== "running") return null;
  const parts = [progressPhaseLabel(entry)];
  const percent = progressPercent(entry);
  if (percent !== null) parts.push(`${percent}%`);
  if (
    typeof entry.progressEtaS === "number" &&
    Number.isFinite(entry.progressEtaS)
  ) {
    parts.push(`~${formatDuration(entry.progressEtaS)} left`);
  }
  return parts.join(" · ");
}

export function sourceDependencyFailure(
  entry: RenderQueueEntry,
  entries: RenderQueueEntry[],
): string | null {
  const source = sourceEntryFor(entry, entries);
  if (!source) return null;
  if (source.status === "failed") {
    return `source render failed: ${source.error ?? "unknown error"}`;
  }
  if (source.status === "cancelled") {
    return "source render was cancelled";
  }
  return null;
}

export function reframeMasterPathForEntry(
  entry: RenderQueueEntry,
  entries: RenderQueueEntry[],
  lastMasterPath: string | null,
): string | null {
  if (entry.reframeMasterPath) return entry.reframeMasterPath;
  const source = sourceEntryFor(entry, entries);
  if (source?.status === "done" && source.outputPath) return source.outputPath;
  return lastMasterPath;
}

export function hasRunnablePendingWithoutRunning(
  entries: RenderQueueEntry[],
): boolean {
  return entries.some((entry) => entry.status === "pending") &&
    !entries.some(
      (entry) =>
        entry.status === "running" ||
        (entry.status === "done" && hasActiveUpload(entry.uploadStates)),
    );
}

export function renderQueueApprovalCopy(entry: RenderQueueEntry): string | null {
  if (entry.reviewStatus === "approved") {
    if (hasActiveUpload(entry.uploadStates)) {
      return "Render approved. Publishing is still in progress.";
    }
    if (hasFailedUpload(entry.uploadStates)) {
      return "Render approved. Publishing needs action.";
    }
    return "Approved for delivery.";
  }
  if (entry.reviewStatus === "changes_requested") {
    return "Changes requested. Re-edit before delivery.";
  }
  return null;
}

type State = {
  entries: RenderQueueEntry[];
  enqueue: (entries: RenderQueueEntry[]) => void;
  markRunning: (id: string, jobId?: string) => void;
  markProgress: (
    id: string,
    percent: number,
    details?: RenderProgressDetails,
  ) => void;
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
  /** Store selected connected-account ids for upload targets. */
  setUploadAccountIds: (
    id: string,
    accountIdsByProvider: Record<string, string>,
  ) => void;
  /**
   * Replace this entry's per-target upload state map. Called by the
   * server-backed publish path after each target state transition.
   */
  setUploadStates: (
    id: string,
    states: Record<string, RenderUploadState>,
    publishedUrls: Record<string, string>,
  ) => void;
  /**
   * Replace this entry's per-target upload metadata snapshot. Called
   * by the worker just before server-backed publishing so the saved
   * queue entry carries the same metadata the backend was handed —
   * useful for retry, where we don't want to re-read the user's form
   * state (they may have edited the form for a *different* render
   * since then).
   */
  setUploadMetadata: (
    id: string,
    metadata: Record<string, UploadMetadata>,
  ) => void;
  /**
   * Stamp this entry's AI disclosure. Called by the worker right
   * after `compute_ai_disclosure` round trips so the RenderQueue row
   * can surface the AI chip + the per-target form can render the
   * banner. Pass `undefined` to clear (e.g. terminal cleanup).
   */
  setAiDisclosure: (id: string, disclosure: AiDisclosure | undefined) => void;
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

function isNonterminal(entry: RenderQueueEntry): boolean {
  return entry.status === "pending" || entry.status === "running";
}

function activeVisibleTargets(entries: RenderQueueEntry[]): Set<string> {
  return new Set(
    entries
      .filter((entry) => !entry.internal && isNonterminal(entry))
      .map((entry) => entry.targetId),
  );
}

function suppressDuplicateActiveTargets(
  existing: RenderQueueEntry[],
  incoming: RenderQueueEntry[],
): RenderQueueEntry[] {
  const existingActiveTargets = activeVisibleTargets(existing);
  if (existingActiveTargets.size === 0) return [...existing, ...incoming];

  const blockedSourceIds = new Set(
    incoming
      .filter(
        (entry) =>
          !entry.internal &&
          isNonterminal(entry) &&
          existingActiveTargets.has(entry.targetId),
      )
      .map((entry) => entry.sourceEntryId)
      .filter((id): id is string => Boolean(id)),
  );
  const filteredIncoming = incoming.filter((entry) => {
    if (entry.internal) return !blockedSourceIds.has(entry.id);
    return !(isNonterminal(entry) && existingActiveTargets.has(entry.targetId));
  });
  return [...existing, ...filteredIncoming];
}

export const useRenderQueueStore = create<State>((set) => ({
  entries: loadPersisted(),
  enqueue: (newEntries) => {
    set((state) => {
      const next = suppressDuplicateActiveTargets(state.entries, newEntries);
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
  markProgress: (id, percent, details) => {
    // Skip persistence for progress ticks — too chatty for localStorage.
    set((state) => ({
      entries: state.entries.map((e) =>
        e.id === id
          ? {
              ...e,
              progress: percent,
              progressPhase: details?.phase ?? e.progressPhase,
              progressEtaS:
                details && "etaS" in details ? details.etaS : e.progressEtaS,
              progressTimeDoneS:
                details && "timeDoneS" in details
                  ? details.timeDoneS
                  : e.progressTimeDoneS,
              progressLogExcerpt:
                details && "logExcerpt" in details
                  ? details.logExcerpt
                  : e.progressLogExcerpt,
            }
          : e,
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
  setUploadAccountIds: (id, accountIdsByProvider) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id ? { ...e, uploadAccountIds: accountIdsByProvider } : e,
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
  setUploadMetadata: (id, metadata) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id ? { ...e, uploadMetadata: metadata } : e,
      );
      persist(next);
      return { entries: next };
    });
  },
  setAiDisclosure: (id, disclosure) => {
    set((state) => {
      const next = state.entries.map((e) =>
        e.id === id ? { ...e, aiDisclosure: disclosure } : e,
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
