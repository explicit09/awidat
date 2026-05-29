/**
 * ProposalCard — one row in the Brief stack (Wave 3 B2).
 *
 * Layout (left → right):
 *   [medium chip] [title + rationale] [source pill] [Accept / Reject]
 *                                                   + Review link
 *
 * Routing: clicking "Review →" switches the center pane to the surface
 * most relevant to the medium. Best-effort focus is reserved for Wave 4.
 */

import { useState } from "react";
import { Button, cn } from "../../ui";
import type {
  BriefProposal,
  BriefMedium,
  BrollDisclosureMetadata,
} from "../../state/briefProposals";
import { useBriefProposalsStore } from "../../state/briefProposals";
import type { CenterMode } from "../../state/centerMode";

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

/** Medium → color-coded chip. Spec source: B2 task description. */
const MEDIUM_STYLE: Record<BriefMedium, { label: string; className: string }> = {
  cut: {
    label: "Cut",
    className: "bg-[rgba(34,211,238,0.12)] text-[#67E8F9] border-[rgba(34,211,238,0.32)]",
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
    // Spec calls captions + titles mint; the medium type collapses both
    // into `title`. Mint tone keeps the visual contract.
    label: "Title",
    className: "bg-[rgba(45,212,191,0.12)] text-[#5EEAD4] border-[rgba(45,212,191,0.32)]",
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

/**
 * Mediums → which center surface answers "review this proposal best".
 * Exported for tests and for the BriefSurface's tab-switch handler.
 */
export const REVIEW_MODE_FOR_MEDIUM: Record<BriefMedium, CenterMode> = {
  cut: "timeline",
  color: "source",
  broll: "source",
  audio: "timeline",
  transition: "timeline",
  // Title/caption land on the source preview where the user reads them.
  title: "source",
  mixed: "timeline",
  other: "brief",
};

/** One-line "Review on …" label per medium. */
const REVIEW_LABEL: Partial<Record<BriefMedium, string>> = {
  cut: "Review on timeline",
  color: "Review on preview",
  broll: "Review on preview",
  audio: "Review on timeline",
  transition: "Review on timeline",
  title: "Review on preview",
  mixed: "Review on timeline",
  // `other` (bash/apply_patch) deliberately omitted — no review link.
};

export interface ProposalCardProps {
  proposal: BriefProposal;
  /** Called when the user clicks "Review →". The Brief surface uses
   *  this to switch the center pane to the relevant tab. */
  onReview: (mode: CenterMode, proposal: BriefProposal) => void;
}

export function ProposalCard({ proposal, onReview }: ProposalCardProps) {
  const accept = useBriefProposalsStore((s) => s.accept);
  const reject = useBriefProposalsStore((s) => s.reject);
  const [busy, setBusy] = useState<"accept" | "reject" | null>(null);

  const medium = MEDIUM_STYLE[proposal.medium];
  const reviewLabel = REVIEW_LABEL[proposal.medium];
  const reviewMode = REVIEW_MODE_FOR_MEDIUM[proposal.medium];

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

  async function onReject() {
    if (busy) return;
    setBusy("reject");
    try {
      await reject(proposal.id);
    } catch (e) {
      console.warn("brief reject failed", proposal.id, e);
      setBusy(null);
    }
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
        "flex items-stretch gap-3",
        "rounded-[var(--radius-md)] border border-[var(--color-border-subtle)]",
        "bg-[var(--color-surface-card)] py-2 pl-2 pr-3",
        "transition-[background-color] duration-[120ms]",
        "hover:bg-[var(--color-surface-card-hover)]",
      )}
      aria-label={`Proposal: ${proposal.title}`}
    >
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
            onClick={() => onReview(reviewMode, proposal)}
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
          aria-label={`Reject ${proposal.title}`}
        >
          {busy === "reject" ? "…" : "Reject"}
        </Button>
      </div>
    </article>
  );
}
