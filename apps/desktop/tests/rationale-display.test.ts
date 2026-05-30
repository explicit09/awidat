// Tests for the shared `RationaleHint` helper used by proposal-
// rendering surfaces outside the Brief row and ProposalInspector
// (which roll their own bespoke layouts).
//
// `RationaleHint` itself is a tiny React component. The test runner
// is plain `node --experimental-strip-types`, which can't load `.tsx`
// files, so we follow the pattern used by `status-pill.test.ts` and
// the rest of the desktop suite: extract the load-bearing logic
// (className resolution + the "should-render" predicate) into pure
// functions colocated with the component, then assert on those.
//
// The contract we lock in here:
//
//   1. Empty / undefined / null rationale → `shouldShowRationale`
//      returns `false`, so the component renders nothing — no
//      placeholder dash, no extra DOM weight.
//   2. Non-empty rationale → renders with the 11px italic muted
//      class stack the B5 spec calls for, including the `truncate`
//      single-line behaviour.
//   3. Caller-provided `className` extends the base classes rather
//      than replacing them (so a caller can add `mt-1` without
//      losing italic / muted styling).

import { strict as assert } from "node:assert";
import {
  rationaleHintClass,
  shouldShowRationale,
} from "../src/ui/components/rationaleHint.logic.ts";

console.log("# RationaleHint");

// 1. shouldShowRationale — empty / undefined / null / whitespace.
{
  assert.equal(
    shouldShowRationale(undefined),
    false,
    "undefined rationale must not render",
  );
  assert.equal(
    shouldShowRationale(null),
    false,
    "null rationale must not render",
  );
  assert.equal(
    shouldShowRationale(""),
    false,
    "empty string must not render",
  );
  assert.equal(
    shouldShowRationale("   "),
    false,
    "all-whitespace rationale must not render",
  );
  assert.equal(
    shouldShowRationale("trimmed 0.42s silence"),
    true,
    "non-empty rationale must render",
  );
  console.log(
    "  ok  shouldShowRationale gates rendering on non-blank content",
  );
}

// 2. Base className contract — italic, 11px, muted token, single-line.
{
  const base = rationaleHintClass();
  assert.ok(base.includes("italic"), "must include italic style");
  assert.ok(base.includes("text-[11px]"), "must use the 11px B5 spec size");
  assert.ok(
    base.includes("text-[var(--color-text-muted)]"),
    "must use the muted-text token, not a hard-coded hex",
  );
  assert.ok(base.includes("truncate"), "must single-line truncate");
  assert.ok(base.includes("leading-tight"), "must use tight line height");
  console.log(
    "  ok  base className uses spec'd italic/muted/11px/truncate tokens",
  );
}

// 3. className extends, never replaces.
{
  const merged = rationaleHintClass("mt-1");
  assert.ok(merged.includes("mt-1"), "caller className must be appended");
  assert.ok(
    merged.includes("italic"),
    "base italic must survive after extending",
  );
  assert.ok(
    merged.includes("text-[var(--color-text-muted)]"),
    "muted token must survive after extending",
  );
  console.log("  ok  caller className extends, base styles preserved");
}
