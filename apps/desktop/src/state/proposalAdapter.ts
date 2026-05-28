/**
 * Bridge between the existing useProposalStore and the new
 * ProposalInspector (src/shell/ProposalInspector.tsx).
 *
 * The store carries the raw protocol payload; the inspector wants a
 * presentation-shaped object. This adapter does the shape change in
 * one place so the inspector stays decoupled from the protocol
 * types — Phase 3+ may swap the source (e.g. for batched proposals)
 * without touching the component.
 */

import { useProposalStore } from "../timeline/proposal";
import type {
  Alternative,
  EvidenceItem,
  EvidenceKind,
  ProposalInspectorData,
} from "../shell";
import type {
  ProposalEvidence,
  ProposalEvidenceKind,
  RiskLevel,
} from "../protocol";

const KIND_MAP: Record<ProposalEvidenceKind["kind"], EvidenceKind> = {
  transcript: "transcript",
  audio_energy: "audio_energy",
  speaker_handoff: "speaker_handoff",
  silence: "silence",
  pacing: "pacing",
  filler: "filler",
  visual: "visual",
  cut_boundary: "cut_boundary",
};

function mapEvidence(rows: readonly ProposalEvidence[]): EvidenceItem[] {
  return rows.map((r, i) => ({
    id: `${r.kind.kind}-${i}`,
    kind: KIND_MAP[r.kind.kind],
    label: r.label,
    confidence: r.confidence ?? undefined,
    confidenceLevel:
      r.confidence_level === "high"
        ? "high"
        : r.confidence_level === "medium"
          ? "medium"
          : r.confidence_level === "low"
            ? "low"
            : r.confidence_level === "very_low"
              ? "very-low"
              : undefined,
  }));
}

function mapRisk(r: RiskLevel | undefined): ProposalInspectorData["risk"] {
  if (!r) return undefined;
  switch (r) {
    case "low":
      return "low";
    case "medium":
      return "medium";
    case "high":
      return "high";
    case "very_high":
      return "very-high";
  }
}

/**
 * Hook: subscribe to the active proposal and return inspector-shaped data.
 * Returns `undefined` when no proposal is active so the inspector renders
 * its empty state.
 */
export function useProposalInspectorData(): ProposalInspectorData | undefined {
  const active = useProposalStore((s) => s.active);
  if (!active) return undefined;

  // Title: prefer the agent's summary as the title chip; if we ever
  // get a structured `title` field we'll swap it in.
  const title = active.summary || "Proposal";

  // Map our internal phase to a proposal-family pill state. The inspector
  // surface is always proposal-flavored; Started/Delta phases render as
  // "proposed" with a "Pending" label until Accept/Reject upstream flips
  // the store, which clears `active`.
  const proposalState: ProposalInspectorData["proposalState"] = "proposed";
  const statusLabel = "Pending";

  const alternatives: Alternative[] =
    active.alternatives?.map((a) => ({
      id: a.id.toString(),
      label: a.label,
      detail: a.detail ?? undefined,
    })) ?? [];

  return {
    title,
    proposalState,
    statusLabel,
    intent: active.intent,
    explanation: active.explanation,
    confidence: active.confidence,
    risk: mapRisk(active.risk),
    evidence: mapEvidence(active.evidence ?? []),
    alternatives,
    alternativesCountLabel:
      alternatives.length > 0 ? `View all (${alternatives.length})` : undefined,
  };
}
