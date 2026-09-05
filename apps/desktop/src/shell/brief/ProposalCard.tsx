/**
 * ProposalCard — one row in the Brief stack (Wave 3 B2 + Wave 4 W4.6).
 *
 * Layout (left → right):
 *   [medium chip] [title + rationale] [source pill] [Accept / Reject]
 *                                                   + Review link
 *
 * Routing: clicking "Review →" hands the proposal id + medium +
 * diff_hints to `useFocusController`, which switches the tab AND
 * focuses the affected entity (timeline scroll, transcript jump,
 * canvas flash). Callers no longer have to know how to focus — the
 * controller owns the per-medium contract.
 */

import { useState } from "react";
import { Button, cn } from "../../ui";
import type {
  BriefProposal,
  BriefMedium,
  BrollDisclosureMetadata,
} from "../../state/briefProposals";
import { useBriefProposalsStore } from "../../state/briefProposals";
import { useFocusController } from "../../state/focusController";
import { usePendingProposals } from "../../timeline/pendingProposals";
import { RejectReasonPicker } from "./RejectReasonPicker";

/**
 * Build the muted disclosure label rendered under the prompt for
 * generated-broll proposals. The "AI-generated" prefix is invariant —
 * the requires_disclosure flag is upstream and assumed for any row
 * that reached this branch.
 */
function brollDisclosureLabel(metadata: BrollDisclosureMetadata): string {
  const parts = ["Disclosure: AI-generated"];
  if (metadata.provider) parts.push(`provider ${metadata.provider}`);
  if (metadata.model) parts.push(`model ${metadata.model}`);
  return parts.join(" · ");
}

/** Medium → color-coded chip. Spec source: B2 task description.
 *  Exported so other surfaces (e.g. HistorySurface) can share the
 *  visual contract instead of redefining the palette per surface. */
export const MEDIUM_STYLE: Record<BriefMedium, { label: string; className: string }> = {
  cut: {
    label: "Cut",
    className: "bg-[rgba(34,211,238,0.12)] text-[#FCA5A5] border-[rgba(34,211,238,0.32)]",
  },
  color: {
    label: "Color",
    className: "bg-[rgba(245,158,11,0.12)] text-[#FCD34D] border-[rgba(245,158,11,0.32)]",
  },
  broll: {
    label: "B-roll",
    className: "bg-[rgba(168,85,247,0.14)] text-[#D8B4FE] border-[rgba(168,85,247,0.32)]",
  },
  audio: {
    label: "Audio",
    className: "bg-[rgba(100,116,139,0.18)] text-[#CBD5E1] border-[rgba(100,116,139,0.40)]",
  },
  transition: {
    label: "Transition",
    className: "bg-[rgba(168,85,247,0.14)] text-[#D8B4FE] border-[rgba(168,85,247,0.32)]",
  },
  title: {
    // Picture-side titles (insert_title / set_title). Teal-mint per spec.
    label: "Title",
    className: "bg-[rgba(45,212,191,0.12)] text-[#5EEAD4] border-[rgba(45,212,191,0.32)]",
  },
  caption: {
    // Caption is its own medium — transcript-track overlay. Spec keeps
    // the mint family but uses the emerald-mint shade so the user can
    // tell captions (transcript) and titles (picture) apart at a glance.
    label: "Caption",
    className: "bg-[rgba(16,185,129,0.12)] text-[#6EE7B7] border-[rgba(16,185,129,0.32)]",
  },
  mixed: {
    label: "Mixed",
    className: "bg-[rgba(148,163,184,0.16)] text-[#E2E8F0] border-[rgba(148,163,184,0.34)]",
  },
  other: {
    label: "Other",
    className: "bg-[rgba(148,163,184,0.12)] text-[var(--color-text-muted)] border-[var(--color-border-subtle)]",
  },
};

/** One-line "Review on …" label per medium. */
const REVIEW_LABEL: Partial<Record<BriefMedium, string>> = {
  cut: "Review on timeline",
  color: "Review on preview",
  broll: "Review on preview",
  audio: "Review on timeline",
  transition: "Review on timeline",
  title: "Review on preview",
  caption: "Review on transcript",
  mixed: "Review on timeline",
  // `other` (bash/apply_patch) deliberately omitted — no review link.
};

/**
 * Preset reject reasons per medium (Wave 5 C1). The chip a user clicks
 * is captured INSTANTLY (no debounce, no Submit) and forwarded to
 * `useBriefProposalsStore.reject(id, reason)`. The list is intentionally
 * tight — every extra chip costs a moment of editorial-flow latency.
 *
 * Exported so tests and the History tab can share the catalog without
 * redefining it per surface.
 */
export const REJECT_REASONS: Record<BriefMedium, readonly string[]> = {
  cut: ["Too aggressive", "Wrong segment", "Pacing", "Loses context", "Wrong speaker"],
  color: ["Too warm", "Too cool", "Wrong skin tone", "Style mismatch"],
  broll: ["Off-topic", "Disclosure unclear", "Style mismatch", "Wrong moment"],
  audio: ["Wrong levels", "Wrong ducking", "Loses ambience"],
  caption: ["Wrong word", "Wrong speaker label", "Timing off"],
  transition: ["Style mismatch", "Distracting", "Timing off"],
  title: ["Style mismatch", "Distracting", "Timing off"],
  mixed: ["Not now", "Wrong approach", "Misunderstood intent"],
  other: ["Not now", "Wrong approach", "Misunderstood intent"],
};

export interface ProposalCardProps {
  proposal: BriefProposal;

}

export function ProposalCard({ proposal }: ProposalCardProps) {
  const accept = useBriefProposalsStore((s) => s.accept);
  const reject = useBriefProposalsStore((s) => s.reject);
  const focusProposal = useFocusController((s) => s.focusProposal);
  const [busy, setBusy] = useState<"accept" | "reject" | null>(null);
  // Inline reason picker — opens below the Accept/Reject row when the
  // user first clicks Reject. Token-styled, no modal. Closing via Esc or
  // "Reject without reason" still rejects (the latter) or cancels (Esc).
  const [pickerOpen, setPickerOpen] = useState(false);
  const [customReason, setCustomReason] = useState("");

  const medium = MEDIUM_STYLE[proposal.medium];
  const reviewLabel = REVIEW_LABEL[proposal.medium];
  const presetReasons = REJECT_REASONS[proposal.medium] ?? [];

  function handleReview(): void {
    // Hand the controller everything it needs to focus the entity.
    // diff_hints + proposed snapshot live on the matching
    // PendingProposal entry — we look them up by callId rather than
    // adding them to the BriefProposal projection (keeps that view
    // load-bearing-data-free for the row chrome).
    const pending = usePendingProposals
      .getState()
      .pending.find((p) => p.callId === proposal.id);
    focusProposal({
      proposalId: proposal.id,
      medium: proposal.medium,
      diffHints: pending?.diffHints,
      proposedSnapshot: pending?.snapshot,
    });

  }

  async function onAccept() {
    if (busy) return;
    setBusy("accept");
    try {
      await accept(proposal.id);
    } catch (e) {
      console.warn("brief accept failed", proposal.id, e);
      setBusy(null);
    }
    // No reset on success — the row removes itself from the stack so
    // the component unmounts. If it stays (race), `busy` stale-state
    // would block re-clicks; the rare case isn't worth a useEffect.
  }

  function onReject() {
    if (busy) return;
    // First click opens the inline picker. The existing reject flow only
    // fires once the user picks a reason — or clicks "Reject without
    // reason" — so the silent-rejection path is preserved.
    setPickerOpen(true);
  }

  async function applyReject(reason?: string) {
    if (busy) return;
    setBusy("reject");
    setPickerOpen(false);
    try {
      await reject(proposal.id, reason);
    } catch (e) {
      console.warn("brief reject failed", proposal.id, e);
      setBusy(null);
    }
  }

  function cancelPicker() {
    setPickerOpen(false);
    setCustomReason("");
  }

  const sourceLabel =
    proposal.source === "broll"
      ? "Agent"
      : proposal.source === "approval"
        ? "Agent"
        : "User";

  const isBroll =
    proposal.medium === "broll" &&
    proposal.source === "broll" &&
    proposal.brollMetadata !== undefined;

  return (
    <article
      className={cn(
        "flex flex-col",
        "rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-card)]",
        "transition-[background-color] duration-[120ms]",
        "hover:bg-[var(--color-surface-card-hover)]",
      )}
      aria-label={`Proposal: ${proposal.title}`}
    >
      <div className="flex items-stretch gap-3 py-2 pl-2 pr-3">
      {/* Medium chip — left edge */}
      <span
        className={cn(
          "flex items-center px-2",
          "rounded-[var(--radius-sm)] border",
          "text-[var(--text-caption)] font-medium uppercase tracking-wider whitespace-nowrap",
          "h-6 self-start mt-0.5",
          medium.className,
        )}
      >
        {medium.label}
      </span>

      {/* Optional broll thumbnail — left of the body, after the chip.
          Renders only when the broll branch has a thumbnail path. */}
      {isBroll && proposal.brollMetadata?.thumbnailPath && (
        <img
          src={proposal.brollMetadata.thumbnailPath}
          alt=""
          aria-hidden
          className={cn(
            "shrink-0 self-center rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)]",
            "h-10 w-16 object-cover bg-[var(--color-surface-panel)]",
          )}
        />
      )}

      {/* Title + rationale + source */}
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-2">
          <span
            className="truncate text-[13px] font-semibold text-[var(--color-text-primary)]"
            title={proposal.title}
          >
            {proposal.title}
          </span>
          <span
            className={cn(
              "shrink-0 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)]",
              "bg-[var(--color-surface-panel)] px-1.5 py-[1px]",
              "text-[10px] font-medium uppercase tracking-[0.08em] text-[var(--color-text-muted)]",
            )}
            title={`Originated by: ${sourceLabel}`}
          >
            {sourceLabel}
          </span>
        </div>
        {isBroll && proposal.brollMetadata ? (
          <>
            <div
              className="truncate text-[12px] italic text-[var(--color-text-primary)]"
              title={proposal.brollMetadata.prompt}
            >
              {proposal.brollMetadata.prompt}
            </div>
            <div
              className="truncate text-[10px] text-[var(--color-text-muted)]"
              title={brollDisclosureLabel(proposal.brollMetadata)}
            >
              {brollDisclosureLabel(proposal.brollMetadata)}
            </div>
          </>
        ) : (
          proposal.rationale && (
            <div
              className="truncate text-[11px] italic text-[var(--color-text-muted)]"
              title={proposal.rationale}
            >
              {proposal.rationale}
            </div>
          )
        )}
        {reviewLabel && (
          <button
            type="button"
            onClick={handleReview}
            className={cn(
              "self-start mt-0.5 text-[11px] font-medium",
              "text-[var(--color-brand)] hover:text-[var(--color-brand-hover)]",
              "underline-offset-2 hover:underline",
            )}
            aria-label={`${reviewLabel} for ${proposal.title}`}
          >
            {reviewLabel} →
          </button>
        )}
      </div>

      {/* Accept / Reject — right edge */}
      <div className="flex items-start gap-1.5 self-center">
        <Button
          variant="primary"
          size="sm"
          onClick={onAccept}
          disabled={busy !== null}
          aria-label={`Accept ${proposal.title}`}
        >
          {busy === "accept" ? "…" : "Accept"}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={onReject}
          disabled={busy !== null}
          aria-pressed={pickerOpen}
          aria-expanded={pickerOpen}
          aria-controls={`reject-picker-${proposal.id}`}
          aria-label={`Reject ${proposal.title}`}
        >
          {busy === "reject" ? "…" : "Reject"}
        </Button>
      </div>
      </div>

      {pickerOpen && (
        <RejectReasonPicker
          proposalId={proposal.id}
          proposalTitle={proposal.title}
          presets={presetReasons}
          customReason={customReason}
          onCustomChange={setCustomReason}
          onPick={applyReject}
          onCancel={cancelPicker}
        />
      )}
    </article>
  );
}
