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

type ApprovalRequestItem = Extract<Item, { kind: "approval_request" }>;
type ProposedEditItem = Extract<Item, { kind: "proposed_edit" }>;

export type BriefProposalSource = "approval" | "proposed_edit";
export type BriefMedium = ProposalMedium;

/** Unified row the Brief renders. Discriminated by `source`. */
export interface BriefProposal {
  /** call_id for approvals; proposed_edit id for ProposedEdits. */
  id: string;
  source: BriefProposalSource;
  medium: BriefMedium;
  /** One-line title rendered on the row. */
  title: string;
  /**
   * Agent's one-sentence "why" — present on ProposedEdits when the
   * agent fills `rationale`, always `undefined` for ApprovalRequests
   * (no rationale field on that wire variant today).
   */
  rationale: string | undefined;
  /** When this entry first hit the stack (ms epoch). */
  firstSeenAt: number;
  /** Tool name for approval-source rows (kind chip); undefined otherwise. */
  toolName: string | undefined;
}

/**
 * Side-effect adapter. Defaults call Tauri + editorDispatch; tests
 * override via `useBriefProposalsStore.setState({ dispatch })`.
 */
export interface BriefDispatch {
  respondApproval: (callId: string, decision: "allow" | "deny") => Promise<void>;
  acceptProposal: (callId: string) => Promise<void>;
  rejectProposal: (callId: string) => Promise<void>;
}

interface BriefState {
  approvals: Map<string, ApprovalEntry>;
  dispatch: BriefDispatch;
  ingestApproval: (item: ApprovalRequestItem) => void;
  /** Combined view: approvals ∪ usePendingProposals.pending, newest first. */
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
};

export const useBriefProposalsStore = create<BriefState>((set, get) => ({
  approvals: new Map(),
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
      const entry: ApprovalEntry = {
        id: item.id,
        toolName: item.tool_name,
        argsSummary: item.args_summary,
        firstSeenAt: prev?.firstSeenAt ?? Date.now(),
        phase: item.phase,
      };
      const next = new Map(state.approvals);
      next.set(item.id, entry);
      return { approvals: next };
    });
  },

  pending() {
    const approvals = Array.from(get().approvals.values()).map(
      approvalToBriefProposal,
    );
    const proposedEdits = usePendingProposals
      .getState()
      .pending.map(pendingToBriefProposal);
    return [...approvals, ...proposedEdits].sort(
      (a, b) => b.firstSeenAt - a.firstSeenAt,
    );
  },

  async accept(id) {
    const state = get();
    if (state.approvals.has(id)) {
      await state.dispatch.respondApproval(id, "allow");
      // Optimistic remove. The backend's matching Completed will also
      // arrive and drop it via ingest; the optimistic path keeps the
      // Brief responsive against backend round-trip latency.
      set((s) => removeApproval(s, id));
      return;
    }
    // Otherwise it's a proposed_edit — usePendingProposals clears its
    // own entry when the backend emits Completed.
    await state.dispatch.acceptProposal(id);
  },

  async reject(id, _reason) {
    const state = get();
    if (state.approvals.has(id)) {
      await state.dispatch.respondApproval(id, "deny");
      set((s) => removeApproval(s, id));
      return;
    }
    await state.dispatch.rejectProposal(id);
  },

  clear() {
    set({ approvals: new Map() });
  },
}));

function removeApproval(state: BriefState, id: string): Partial<BriefState> {
  if (!state.approvals.has(id)) return state;
  const next = new Map(state.approvals);
  next.delete(id);
  return { approvals: next };
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
  // Captions / titles (caption variants ride the title medium today).
  if (
    toolName === "insert_caption" ||
    toolName === "set_caption" ||
    toolName === "insert_title" ||
    toolName === "set_title"
  ) {
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
    rationale: undefined,
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

/** Type-narrowing helper for the items subscription in appGlue. */
export function isApprovalRequestItem(item: Item): item is ApprovalRequestItem {
  return item.kind === "approval_request";
}

/** Re-export so callers don't need a second import for the sibling guard. */
export function isProposedEditItem(item: Item): item is ProposedEditItem {
  return item.kind === "proposed_edit";
}
