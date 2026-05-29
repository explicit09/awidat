/**
 * Pure-logic helpers backing the `RationaleHint` React component.
 *
 * Kept in a `.ts` (no JSX) sibling so the desktop test runner —
 * `node --experimental-strip-types`, which can't load `.tsx` —
 * can exercise the rendering contract without a JSX build step.
 *
 * The component itself is a thin presentational wrapper: it
 * decides whether to render via `shouldShowRationale` and picks
 * its className stack via `rationaleHintClass`. Both are tested
 * by `tests/rationale-display.test.ts`.
 */

import { cn } from "../cn.ts";

/**
 * `true` when the rationale text should be rendered. Treat empty
 * strings and all-whitespace as absent so the agent emitting an
 * accidental `""` or `"   "` still collapses cleanly to no DOM.
 */
export function shouldShowRationale(
  rationale: string | null | undefined,
): boolean {
  if (rationale === null || rationale === undefined) return false;
  return rationale.trim().length > 0;
}

/**
 * Resolves the className stack for the rationale label.
 *
 * Base classes match the Wave 3 B5 spec:
 *   - 11px italic, leading-tight (max two visual lines collapsed
 *     into one truncated line; full text lives in the title=
 *     tooltip from the component layer)
 *   - muted text token (NOT a hard-coded hex)
 *   - block + truncate so the line never wraps and never spills
 *     past its column.
 *
 * Caller-provided `extra` is appended via `cn` so the caller can add
 * spacing utilities without losing the base styles (Tailwind merges
 * later tokens in `cn` so caller-supplied tokens win on conflict).
 */
export function rationaleHintClass(extra?: string): string {
  return cn(
    "block truncate text-[11px] italic leading-tight text-[var(--color-text-muted)]",
    extra,
  );
}
