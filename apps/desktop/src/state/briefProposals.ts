// useBriefProposalsStore — single source of truth for the Brief stack.
//
// Two flows feed the Brief surface (Wave 3 B1):
//
//  1. Agent-initiated `Item::ApprovalRequest` (apply_edl, apply_patch,
//     bash) — owned by this store, kept in its own approvals map.
//
//  2. User/agent-initiated `Item::ProposedEdit` — owned by
//     `usePendingProposals` (timeline ghost overlay lifecycle). We
//     mirror it read-only here; one source of truth, two views.
//
// Auto-decided exclusion: codex itself decides which server-requests
// reach the desktop. Anything matching the active permission mode is
// resolved inside codex and never emits an ApprovalRequest. As a belt-
// and-braces guard, the ingest path treats `phase === "completed"` as
// "remove-or-no-op" — an ApprovalRequest that arrives already-Completed
// (history replay, race) is dropped without ever joining the stack.

import { create } from "zustand";
import type { Item, ItemLifecycle } from "../protocol";
import {
  usePendingProposals,
  type PendingProposal,
  type ProposalMedium,
} from "../timeline/pendingProposals.ts";
import { useProjectStore } from "../app/state.ts";
import {
  buildHistoryEntry,
  useProposalHistoryStore,
  type HistoryDecision,
} from "./proposalHistory.ts";

type ApprovalRequestItem = Extract<Item, { kind: "approval_request" }>;
type ProposedEditItem = Extract<Item, { kind: "proposed_edit" }>;

export type BriefProposalSource = "approval" | "proposed_edit" | "broll";
export type BriefMedium = ProposalMedium;

/**
 * Disclosure metadata for generated-broll proposals. Mirrors the
 * fields the user must see to make an informed accept/reject:
 * provider, model, prompt, when, and (if available) a thumbnail.
 */
export interface BrollDisclosureMetadata {
  prompt: string;
  provider: string;
  model?: string;
  thumbnailPath?: string;
  generatedAt?: number;
  /** Absolute path to the rendered video; used by accept(). */
  videoPath?: string;
}

/** Unified row the Brief renders. Discriminated by `source`. */
export interface BriefProposal {
  /** call_id for approvals; proposed_edit id for ProposedEdits;
   *  generated-media job_id for broll. */
  id: string;
  source: BriefProposalSource;
  medium: BriefMedium;
  /** One-line title rendered on the row. */
  title: string;
  /**
   * Agent's one-sentence "why" — populated on ProposedEdits when the
   * agent fills `rationale`, and on ApprovalRequest rows when the
   * bridge captures `reasoning` from the underlying tool args. Stays
   * `undefined` for older producers or approvals whose tool doesn't
   * yet emit a `reasoning` field.
   */
  rationale: string | undefined;
  /** When this entry first hit the stack (ms epoch). */
  firstSeenAt: number;
  /** Tool name for approval-source rows (kind chip); undefined otherwise. */
  toolName: string | undefined;
  /** Set only when this row is a generated-broll proposal. */
  brollMetadata?: BrollDisclosureMetadata;
}

/**
 * Side-effect adapter. Defaults call Tauri + editorDispatch; tests
 * override via `useBriefProposalsStore.setState({ dispatch })`.
 */
export interface BriefDispatch {
  respondApproval: (callId: string, decision: "allow" | "deny") => Promise<void>;
  acceptProposal: (callId: string) => Promise<void>;
  rejectProposal: (callId: string) => Promise<void>;
  /** Place a ready generated-broll asset on the timeline. */
  acceptBroll: (jobId: string, videoPath: string) => Promise<void>;
  /** Dismiss a generated-broll proposal — does not remove the asset. */
  rejectBroll: (jobId: string) => Promise<void>;
  /**
   * Snapshot the project's raw `project.otio.json` text for History's
   * ↺ Restore. Returns `undefined` when no project is loaded or the
   * backend read fails — callers must treat undefined as "no snapshot,
   * proceed without one." Never throws; the audit log is non-load-
   * bearing and a failed capture must not unwind an accept dispatch.
   */
  captureTimelineSnapshot: () => Promise<string | undefined>;
  /**
   * Append a rejection record to `<project>/.awidat/feedback.jsonl`
   * (Wave 5 C2). Fire-and-forget — callers must never block the UI
   * decision on the disk write, and the JSONL is the AGENT-facing
   * source (the localStorage History store remains the UI source of
   * truth). Failures must not unwind a reject dispatch.
   */
  appendFeedback: (projectPath: string, entry: FeedbackPayload) => Promise<void>;
}

/**
 * Wire-shape of one row written to `<project>/.awidat/feedback.jsonl`.
 * Mirrors `commands::feedback::FeedbackEntry` on the Rust side — keep
 * the two in lock-step. `rationale` and `reason` are nullable so silent
 * rejects (no reason supplied) still log a row, which lets the agent
 * spot frequency patterns ("user just keeps rejecting cuts").
 */
export interface FeedbackPayload {
  /** Unix epoch seconds. */
  ts: number;
  medium: BriefMedium;
  title: string;
  rationale: string | null;
  reason: string | null;
}

/**
 * Row tracking a generated-broll job that has reached "ready" and is
 * waiting on the user to decide whether to insert it. Lives in this
 * store (not `useGeneratedMediaStore`) because Brief-side decision
 * state — accepted/rejected — is a Brief concern.
 */
export interface BrollEntry {
  id: string;
  title: string;
  rationale: string | undefined;
  firstSeenAt: number;
  metadata: BrollDisclosureMetadata;
}

interface BriefState {
  approvals: Map<string, ApprovalEntry>;
  brollProposals: Map<string, BrollEntry>;
  /** Job ids the user accepted/rejected via the Brief; the Media-tab
   *  panel and the broll ingester use this to filter the stack. */
  brollDecided: Set<string>;
  dispatch: BriefDispatch;
  ingestApproval: (item: ApprovalRequestItem) => void;
  /**
   * Upsert a generated-broll proposal. Idempotent on jobId; preserves
   * `firstSeenAt` across re-ingest. Callers (appGlue) pass every
   * "ready" entry on every refresh; once a job is `brollDecided` it
   * is silently ignored.
   */
  ingestBroll: (entry: BrollEntry) => void;
  /** Drop a generated-broll row when the underlying job disappears. */
  removeBroll: (jobId: string) => void;
  /** Combined view: approvals ∪ usePendingProposals.pending ∪ broll, newest first. */
  pending: () => BriefProposal[];
  accept: (id: string) => Promise<void>;
  reject: (id: string, reason?: string) => Promise<void>;
  clear: () => void;
}

interface ApprovalEntry {
  id: string;
  toolName: string;
  argsSummary: string;
  firstSeenAt: number;
  phase: ItemLifecycle;
  /**
   * Agent's `reasoning` argument captured by the bridge mapper, when
   * the underlying tool emits one. `undefined` for tools (bash,
   * permissions, legacy callers) that have no `reasoning` arg.
   */
  rationale: string | undefined;
}

// Lazy-load Tauri so tests can run in node without the import erroring.
const defaultDispatch: BriefDispatch = {
  respondApproval: async (callId, decision) => {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("respond_approval", { callId, decision });
  },
  acceptProposal: async (callId) => {
    const { editorDispatch } = await import("../editor/tauriDispatch");
    await editorDispatch.acceptProposal(callId);
  },
  rejectProposal: async (callId) => {
    const { editorDispatch } = await import("../editor/tauriDispatch");
    await editorDispatch.rejectProposal(callId);
  },
  // Generated-broll has no backend "accept" command yet — we place the
  // rendered video file on the timeline through the existing
  // `insert_media_on_timeline` path (same code the Media-tab panel
  // used). Rejection is a Brief-local dismiss; the registry entry
  // stays so the user can find it under Media → Generated history.
  acceptBroll: async (_jobId, videoPath) => {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("insert_media_on_timeline", { assetId: videoPath, atS: null });
  },
  rejectBroll: async (_jobId) => {
    // No-op on the backend; the Brief store records the dismissal.
  },
  captureTimelineSnapshot: async () => {
    // Best-effort read of project.otio.json. Returns undefined on any
    // failure — accept paths must continue regardless because the
    // History audit trail is non-load-bearing chrome.
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const raw = await invoke<string>("read_timeline_otio_raw");
      const trimmed = raw.trim();
      return trimmed.length > 0 ? raw : undefined;
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("captureTimelineSnapshot failed", e);
      return undefined;
    }
  },
  appendFeedback: async (projectPath, entry) => {
    // Lazy-load Tauri so this module stays importable from node tests
    // that don't run inside the desktop runtime.
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("append_feedback", { projectPath, entry });
  },
};

export const useBriefProposalsStore = create<BriefState>((set, get) => ({
  approvals: new Map(),
  brollProposals: new Map(),
  brollDecided: new Set(),
  dispatch: defaultDispatch,

  ingestApproval(item) {
    set((state) => {
      // Completed always removes — covers both manual decisions and
      // the auto-decided race path described in the header.
      if (item.phase === "completed") {
        if (!state.approvals.has(item.id)) return state;
        const next = new Map(state.approvals);
        next.delete(item.id);
        return { approvals: next };
      }
      // Started / Delta — upsert, preserving firstSeenAt across phases
      // so the "waiting 2m" badge stays anchored to first sight.
      const prev = state.approvals.get(item.id);
      // Rationale follows the same preserve-on-omit policy used for
      // ProposedEdit's optional inspector fields: a Delta that omits
      // `rationale` keeps whatever the Started phase carried, so the
      // Brief row never loses "why" mid-lifecycle.
      const incomingRationale = (item as { rationale?: string | null })
        .rationale;
      const rationale =
        incomingRationale == null || incomingRationale === ""
          ? prev?.rationale
          : incomingRationale;
      const entry: ApprovalEntry = {
        id: item.id,
        toolName: item.tool_name,
        argsSummary: item.args_summary,
        firstSeenAt: prev?.firstSeenAt ?? Date.now(),
        phase: item.phase,
        rationale,
      };
      const next = new Map(state.approvals);
      next.set(item.id, entry);
      return { approvals: next };
    });
  },

  ingestBroll(entry) {
    set((state) => {
      // A job the user has already decided on must not re-enter the
      // stack on the next registry refresh.
      if (state.brollDecided.has(entry.id)) return state;
      const prev = state.brollProposals.get(entry.id);
      const merged: BrollEntry = {
        ...entry,
        firstSeenAt: prev?.firstSeenAt ?? entry.firstSeenAt,
      };
      const next = new Map(state.brollProposals);
      next.set(entry.id, merged);
      return { brollProposals: next };
    });
  },

  removeBroll(jobId) {
    set((state) => {
      if (!state.brollProposals.has(jobId)) return state;
      const next = new Map(state.brollProposals);
      next.delete(jobId);
      return { brollProposals: next };
    });
  },

  pending() {
    const approvals = Array.from(get().approvals.values()).map(
      approvalToBriefProposal,
    );
    const proposedEdits = usePendingProposals
      .getState()
      .pending.map(pendingToBriefProposal);
    const broll = Array.from(get().brollProposals.values()).map(
      brollEntryToBriefProposal,
    );
    return [...approvals, ...proposedEdits, ...broll].sort(
      (a, b) => b.firstSeenAt - a.firstSeenAt,
    );
  },

  async accept(id) {
    const state = get();
    const snapshot = snapshotProposal(state, id);
    // Capture the *post*-accept OTIO snapshot so the History tab's
    // ↺ Restore can replay this exact timeline state. We snapshot
    // AFTER the dispatch lands because accept mutates disk, and the
    // user wants "restore to what the timeline looked like immediately
    // after I accepted this" — not "before". Capture is best-effort:
    // if the read fails (no project, IO error) we record the entry
    // without a snapshot and the Restore button stays hidden.
    if (state.approvals.has(id)) {
      await state.dispatch.respondApproval(id, "allow");
      // Optimistic remove. The backend's matching Completed will also
      // arrive and drop it via ingest; the optimistic path keeps the
      // Brief responsive against backend round-trip latency.
      set((s) => removeApproval(s, id));
      const timelineSnapshot = await state.dispatch.captureTimelineSnapshot();
      logDecision(snapshot, "accepted", timelineSnapshot);
      return;
    }
    if (state.brollProposals.has(id)) {
      const entry = state.brollProposals.get(id)!;
      const videoPath = entry.metadata.videoPath;
      if (videoPath) {
        await state.dispatch.acceptBroll(id, videoPath);
      }
      set((s) => decideBroll(s, id));
      const timelineSnapshot = await state.dispatch.captureTimelineSnapshot();
      logDecision(snapshot, "accepted", timelineSnapshot);
      return;
    }
    // Otherwise it's a proposed_edit — usePendingProposals clears its
    // own entry when the backend emits Completed.
    await state.dispatch.acceptProposal(id);
    const timelineSnapshot = await state.dispatch.captureTimelineSnapshot();
    logDecision(snapshot, "accepted", timelineSnapshot);
  },

  async reject(id, reason) {
    const state = get();
    const snapshot = snapshotProposal(state, id);
    const trimmedReason = reason?.trim();
    const rejectReason =
      trimmedReason && trimmedReason.length > 0 ? trimmedReason : undefined;
    if (state.approvals.has(id)) {
      await state.dispatch.respondApproval(id, "deny");
      set((s) => removeApproval(s, id));
      logDecision(snapshot, "rejected", undefined, rejectReason);
      logFeedback(state, snapshot, rejectReason);
      return;
    }
    if (state.brollProposals.has(id)) {
      await state.dispatch.rejectBroll(id);
      set((s) => decideBroll(s, id));
      logDecision(snapshot, "rejected", undefined, rejectReason);
      logFeedback(state, snapshot, rejectReason);
      return;
    }
    await state.dispatch.rejectProposal(id);
    logDecision(snapshot, "rejected", undefined, rejectReason);
    logFeedback(state, snapshot, rejectReason);
  },

  clear() {
    set({
      approvals: new Map(),
      brollProposals: new Map(),
      brollDecided: new Set(),
    });
  },
}));

function removeApproval(state: BriefState, id: string): Partial<BriefState> {
  if (!state.approvals.has(id)) return state;
  const next = new Map(state.approvals);
  next.delete(id);
  return { approvals: next };
}

function decideBroll(state: BriefState, id: string): Partial<BriefState> {
  const nextProposals = new Map(state.brollProposals);
  nextProposals.delete(id);
  const nextDecided = new Set(state.brollDecided);
  nextDecided.add(id);
  return { brollProposals: nextProposals, brollDecided: nextDecided };
}

/**
 * Tool-name → medium. We route by tool name because args summaries are
 * opaque text and args JSON has no stable cross-tool schema. Unknown
 * tools fall through to `cut` — the most common edit surface — so the
 * Brief always has a destination.
 */
export function mediumFromApprovalRequest(item: ApprovalRequestItem): BriefMedium {
  return mediumFromToolName(item.tool_name);
}

function mediumFromToolName(toolName: string): BriefMedium {
  // EDL mutations.
  if (toolName === "apply_edl" || toolName === "delete_clip") return "cut";
  if (toolName.startsWith("ripple_") || toolName.startsWith("trim_")) return "cut";
  // Color family.
  if (toolName === "set_color_correction" || toolName === "apply_lut" || toolName === "remove_lut") {
    return "color";
  }
  // B-roll / picture-in-picture.
  if (toolName === "use_broll" || toolName === "search_broll" || toolName === "insert_b_roll") {
    return "broll";
  }
  // Audio mixing / ducking / FX.
  if (
    toolName === "set_volume" ||
    toolName === "set_audio_fade" ||
    toolName === "set_ducking" ||
    toolName.startsWith("set_audio_") ||
    toolName.startsWith("set_clip_audio_") ||
    toolName.startsWith("set_track_audio_")
  ) {
    return "audio";
  }
  // Caption family — distinct medium so the transcript-first review
  // surface (mint chip) is reachable. Routed separately from titles
  // because captions live on the transcript track, not the title
  // overlay surface.
  if (
    toolName === "insert_caption" ||
    toolName === "set_caption" ||
    toolName.startsWith("caption_")
  ) {
    return "caption";
  }
  // Titles — picture-side overlays (insert_title / set_title).
  if (toolName === "insert_title" || toolName === "set_title") {
    return "title";
  }
  if (toolName === "insert_transition" || toolName === "delete_transition") return "transition";
  // Infrastructure approvals — not editorial. Bucket separately so a
  // bash call isn't mis-tagged as a cut.
  if (toolName === "bash" || toolName === "apply_patch" || toolName === "permissions") {
    return "other";
  }
  return "cut";
}

/**
 * Args summary is the wire-level one-liner built by `mappers::*` in
 * the bridge. We trim and bound length; on empty summary we fall back
 * to the tool name so the row still reads as something.
 */
export function titleFromApprovalRequest(item: ApprovalRequestItem): string {
  return titleFromSummary(item.args_summary, item.tool_name);
}

function titleFromSummary(argsSummary: string, toolName: string): string {
  const summary = argsSummary?.trim();
  if (summary && summary.length > 0) {
    return summary.length > 120 ? `${summary.slice(0, 119)}…` : summary;
  }
  return toolName;
}

function approvalToBriefProposal(entry: ApprovalEntry): BriefProposal {
  return {
    id: entry.id,
    source: "approval",
    medium: mediumFromToolName(entry.toolName),
    title: titleFromSummary(entry.argsSummary, entry.toolName),
    rationale: entry.rationale,
    firstSeenAt: entry.firstSeenAt,
    toolName: entry.toolName,
  };
}

function pendingToBriefProposal(p: PendingProposal): BriefProposal {
  return {
    id: p.callId,
    source: "proposed_edit",
    medium: p.medium,
    title: p.summary && p.summary.trim().length > 0 ? p.summary : "Proposed edit",
    rationale: p.rationale,
    firstSeenAt: p.firstSeenAt,
    toolName: undefined,
  };
}

function brollEntryToBriefProposal(entry: BrollEntry): BriefProposal {
  return {
    id: entry.id,
    source: "broll",
    medium: "broll",
    title: entry.title,
    rationale: entry.rationale,
    firstSeenAt: entry.firstSeenAt,
    toolName: undefined,
    brollMetadata: entry.metadata,
  };
}

/**
 * Snapshot the BriefProposal row by id BEFORE the decision mutates the
 * stack. Returns `null` when the row isn't on the stack — caller logs
 * nothing in that case. We snapshot up front because the optimistic
 * remove inside accept/reject would otherwise hide the row from the
 * synchronous lookup that follows the dispatch await.
 */
function snapshotProposal(
  state: BriefState,
  id: string,
): BriefProposal | null {
  const pending = state.pending();
  return pending.find((p) => p.id === id) ?? null;
}

/**
 * Append a decision to the persisted history log. Best-effort: if no
 * project is loaded (snapshot is null, or no project root yet) we drop
 * the event silently — the log is non-load-bearing chrome and a missing
 * entry should never crash a decision dispatch.
 *
 * `timelineSnapshot` is the raw OTIO bytes captured by the dispatch
 * adapter at decision time. Only populated for the "accepted" decision
 * so the History tab's ↺ Restore action has a rollback target.
 */
function logDecision(
  proposal: BriefProposal | null,
  decision: HistoryDecision,
  timelineSnapshot?: string,
  rejectReason?: string,
): void {
  if (!proposal) return;
  const projectPath = useProjectStore.getState().current;
  if (!projectPath) return;
  const entry = buildHistoryEntry({
    proposal,
    projectPath,
    decision,
    timelineSnapshot,
    rejectReason,
  });
  useProposalHistoryStore.getState().record(entry);
}

/**
 * Append a rejection record to the per-project JSONL log
 * (Wave 5 C2). Fire-and-forget — the disk write must never block the
 * UI decision dispatch, and a failed write must not unwind the reject
 * (the localStorage History store has already recorded the event for
 * the UI). C3 will read this file from the backend to inject recent
 * rejections into the next agent turn.
 *
 * No project loaded → silently drop, same as `logDecision`: the JSONL
 * is non-load-bearing chrome on the reject path.
 */
function logFeedback(
  state: BriefState,
  proposal: BriefProposal | null,
  rejectReason: string | undefined,
): void {
  if (!proposal) return;
  const projectPath = useProjectStore.getState().current;
  if (!projectPath) return;
  const payload: FeedbackPayload = {
    ts: Math.floor(Date.now() / 1000),
    medium: proposal.medium,
    title: proposal.title,
    rationale: proposal.rationale ?? null,
    reason: rejectReason ?? null,
  };
  // Fire-and-forget — fail loud in console but never throw upstream.
  void state.dispatch
    .appendFeedback(projectPath, payload)
    // eslint-disable-next-line no-console
    .catch((e) => console.warn("appendFeedback failed", e));
}

/** Type-narrowing helper for the items subscription in appGlue. */
export function isApprovalRequestItem(item: Item): item is ApprovalRequestItem {
  return item.kind === "approval_request";
}

/** Re-export so callers don't need a second import for the sibling guard. */
export function isProposedEditItem(item: Item): item is ProposedEditItem {
  return item.kind === "proposed_edit";
}
