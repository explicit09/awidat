/**
 * Pure-logic tests for `synthesizeLearnedPatterns`.
 *
 * Covers:
 *  1. Empty input → all fields zero/null/empty.
 *  2. Single-entry input.
 *  3. Multi-entry with a clear most-rejected medium.
 *  4. Mixed silent/reasoned rejects → silentRejectPct math.
 *  5. Curated suggestions emit for known (medium, reason) pairs.
 *  6. Generic fallback fires for ad-hoc custom reasons.
 *  7. Non-reject rows are ignored.
 *  8. Whitespace-only reasons count as silent.
 */
import { strict as assert } from "node:assert";
import {
  synthesizeLearnedPatterns,
} from "../src/state/learnedPatterns.ts";
import {
  buildHistoryEntry,
  type HistoryEntry,
} from "../src/state/proposalHistory.ts";
import type { BriefProposal } from "../src/state/briefProposals.ts";

function makeProposal(overrides: Partial<BriefProposal> = {}): BriefProposal {
  return {
    id: "call-1",
    source: "proposed_edit",
    medium: "cut",
    title: "Trim",
    rationale: undefined,
    firstSeenAt: 1,
    toolName: undefined,
    ...overrides,
  };
}

function reject(
  overrides: Partial<BriefProposal> & { rejectReason?: string; decidedAt?: number },
): HistoryEntry {
  const { rejectReason, decidedAt, ...rest } = overrides;
  return buildHistoryEntry({
    proposal: makeProposal(rest),
    projectPath: "/p",
    decision: "rejected",
    decidedAt: decidedAt ?? Date.now(),
    rejectReason,
  });
}

function accept(
  overrides: Partial<BriefProposal> & { decidedAt?: number },
): HistoryEntry {
  const { decidedAt, ...rest } = overrides;
  return buildHistoryEntry({
    proposal: makeProposal(rest),
    projectPath: "/p",
    decision: "accepted",
    decidedAt: decidedAt ?? Date.now(),
  });
}

// 1. Empty input → null/0/empty
{
  const out = synthesizeLearnedPatterns([]);
  assert.equal(out.mostRejectedMedium, null);
  assert.deepEqual(out.topRejectReasons, []);
  assert.equal(out.silentRejectPct, 0);
  assert.deepEqual(out.suggestedAgentsMdEdits, []);
  assert.equal(out.totalRejects, 0);
}

// 2. Single-entry input — 100% of that medium, one suggestion
{
  const out = synthesizeLearnedPatterns([
    reject({ medium: "cut", rejectReason: "Too aggressive" }),
  ]);
  assert.equal(out.totalRejects, 1);
  assert.deepEqual(out.mostRejectedMedium, { medium: "cut", count: 1, pct: 100 });
  assert.equal(out.topRejectReasons.length, 1);
  assert.equal(out.topRejectReasons[0]!.reason, "Too aggressive");
  assert.equal(out.topRejectReasons[0]!.medium, "cut");
  assert.equal(out.topRejectReasons[0]!.count, 1);
  assert.equal(out.silentRejectPct, 0);
  assert.equal(out.suggestedAgentsMdEdits.length, 1);
  // Curated template wins over the generic fallback.
  assert.ok(
    out.suggestedAgentsMdEdits[0]!.startsWith("Cuts: prefer narrower trims"),
    "expected curated cut/Too aggressive line",
  );
}

// 3. Multi-entry: 6 cut rejects + 2 color + 2 broll → cut is the top
//    bucket at 60%.
{
  const entries: HistoryEntry[] = [];
  for (let i = 0; i < 6; i++) {
    entries.push(reject({ id: `c-${i}`, medium: "cut", rejectReason: "Too aggressive" }));
  }
  for (let i = 0; i < 2; i++) {
    entries.push(reject({ id: `col-${i}`, medium: "color", rejectReason: "Too warm" }));
  }
  for (let i = 0; i < 2; i++) {
    entries.push(reject({ id: `b-${i}`, medium: "broll", rejectReason: "Off-topic" }));
  }
  const out = synthesizeLearnedPatterns(entries);
  assert.equal(out.totalRejects, 10);
  assert.equal(out.mostRejectedMedium?.medium, "cut");
  assert.equal(out.mostRejectedMedium?.count, 6);
  assert.equal(out.mostRejectedMedium?.pct, 60);
  // Top reasons descending by count: cut/Too aggressive (6), then the
  // 2-counts in stable order.
  assert.equal(out.topRejectReasons.length, 3);
  assert.equal(out.topRejectReasons[0]!.reason, "Too aggressive");
  assert.equal(out.topRejectReasons[0]!.count, 6);
  // Suggestions: 3 lines, all curated for these preset reasons.
  assert.equal(out.suggestedAgentsMdEdits.length, 3);
  assert.ok(out.suggestedAgentsMdEdits.some((s) => s.startsWith("Cuts:")));
  assert.ok(out.suggestedAgentsMdEdits.some((s) => s.startsWith("Color:")));
  assert.ok(out.suggestedAgentsMdEdits.some((s) => s.startsWith("B-roll:")));
}

// 4. Mixed silent + reasoned: 4 silent rejects + 6 reasoned → silent
//    is 40%.
{
  const entries: HistoryEntry[] = [];
  for (let i = 0; i < 4; i++) {
    entries.push(reject({ id: `s-${i}`, medium: "cut" })); // no reason
  }
  for (let i = 0; i < 6; i++) {
    entries.push(reject({ id: `r-${i}`, medium: "cut", rejectReason: "Pacing" }));
  }
  const out = synthesizeLearnedPatterns(entries);
  assert.equal(out.totalRejects, 10);
  assert.equal(out.silentRejectPct, 40);
  // Top reason is the reasoned one — silent rows don't bucket.
  assert.equal(out.topRejectReasons.length, 1);
  assert.equal(out.topRejectReasons[0]!.reason, "Pacing");
  assert.equal(out.topRejectReasons[0]!.count, 6);
}

// 5. Suggestion generation hits curated templates for several mediums.
{
  const entries: HistoryEntry[] = [
    reject({ id: "a", medium: "audio", rejectReason: "Wrong levels" }),
    reject({ id: "b", medium: "caption", rejectReason: "Timing off" }),
    reject({ id: "c", medium: "transition", rejectReason: "Distracting" }),
  ];
  const out = synthesizeLearnedPatterns(entries);
  assert.equal(out.suggestedAgentsMdEdits.length, 3);
  assert.ok(out.suggestedAgentsMdEdits.some((s) => s.startsWith("Audio: hold dialogue")));
  assert.ok(out.suggestedAgentsMdEdits.some((s) => s.startsWith("Captions: align caption onset")));
  assert.ok(out.suggestedAgentsMdEdits.some((s) => s.startsWith("Transitions: prefer subtle cuts")));
}

// 6. Generic fallback for an ad-hoc custom reason not in the templates.
{
  const out = synthesizeLearnedPatterns([
    reject({ medium: "cut", rejectReason: "Cuts off the laugh" }),
  ]);
  assert.equal(out.suggestedAgentsMdEdits.length, 1);
  assert.equal(
    out.suggestedAgentsMdEdits[0],
    'Cut: avoid "Cuts off the laugh" — recent rejections flagged this pattern.',
  );
}

// 7. Non-reject rows (accepted / revised / restored) are ignored.
{
  const entries: HistoryEntry[] = [
    accept({ id: "ok-1", medium: "color" }),
    accept({ id: "ok-2", medium: "color" }),
    reject({ id: "r-1", medium: "cut", rejectReason: "Pacing" }),
  ];
  const out = synthesizeLearnedPatterns(entries);
  assert.equal(out.totalRejects, 1);
  assert.equal(out.mostRejectedMedium?.medium, "cut");
  assert.equal(out.mostRejectedMedium?.pct, 100);
}

// 8. Whitespace-only reasons count as silent (matches the wire-side
//    normalization in feedback-log.test.ts).
{
  const out = synthesizeLearnedPatterns([
    reject({ medium: "cut", rejectReason: "   " }),
    reject({ medium: "cut", rejectReason: "Pacing" }),
  ]);
  assert.equal(out.totalRejects, 2);
  assert.equal(out.silentRejectPct, 50);
  assert.equal(out.topRejectReasons.length, 1);
  assert.equal(out.topRejectReasons[0]!.reason, "Pacing");
}

// 9. Top reasons capped at 3 even when more (medium, reason) pairs exist.
{
  const entries: HistoryEntry[] = [
    reject({ id: "1", medium: "cut", rejectReason: "Too aggressive" }),
    reject({ id: "2", medium: "color", rejectReason: "Too warm" }),
    reject({ id: "3", medium: "broll", rejectReason: "Off-topic" }),
    reject({ id: "4", medium: "audio", rejectReason: "Wrong levels" }),
    reject({ id: "5", medium: "caption", rejectReason: "Timing off" }),
  ];
  const out = synthesizeLearnedPatterns(entries);
  assert.equal(out.topRejectReasons.length, 3);
  assert.equal(out.suggestedAgentsMdEdits.length, 3);
}

console.log("learned-patterns: OK");
