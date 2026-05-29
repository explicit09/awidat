/**
 * RationaleHint — single-line italic muted label that surfaces the
 * agent's one-sentence rationale on proposal-rendering surfaces.
 *
 * Same visual contract everywhere the rationale is shown outside the
 * Brief row and ProposalInspector (which have their own bespoke
 * layouts): 11px italic, muted token color, truncated to a single
 * line with the full text in a native `title=` tooltip on hover.
 *
 * Renders `null` when rationale is blank — no placeholder dash, no
 * extra DOM, callers don't need to guard.
 *
 * Why a shared helper:
 *  - DRY: ProposalCard (ui), ChatStream `proposed_edit` item, and any
 *    future surface (Wave 4 ghost-clip review) all want the same
 *    visual + tooltip semantics. Centralizing keeps the look in lock-
 *    step if the rationale styling shifts.
 *  - SRP: One job — render rationale text; no proposal-shape coupling.
 *
 * The rendering decision + className stack live in a `.ts` sibling
 * (`rationaleHint.logic.ts`) so the desktop test runner can exercise
 * them without a JSX build step.
 */

import {
  rationaleHintClass,
  shouldShowRationale,
} from "./rationaleHint.logic";

export interface RationaleHintProps {
  /** The agent's one-sentence rationale. Blank → renders nothing. */
  rationale?: string | null;
  /** Extra classes for callers that need to override spacing
   *  (e.g. pull-up margin under a tight title row). */
  className?: string;
}

export function RationaleHint({ rationale, className }: RationaleHintProps) {
  if (!shouldShowRationale(rationale)) return null;
  // Safe to assert here: shouldShowRationale narrowed away null/undef/blank.
  const text = (rationale as string).trim();
  return (
    <span className={rationaleHintClass(className)} title={text}>
      {text}
    </span>
  );
}
