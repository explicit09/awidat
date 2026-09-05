import { deriveMedium } from "../src/timeline/proposalMedium.ts";
// Tests for useProposalStore — the stack store the Brief surface
// uses to render "5 proposals waiting — 2 cuts, 1 color, 2 captions".
//
// We exercise the same code path the runtime uses (the store's
// `ingest` consumes `Item::ProposedEdit` payloads), so the fixtures
// here are minimal `Item`-shaped objects rather than the projected
// `ActiveProposal` form. The store internally projects + classifies.

import { strict as assert } from "node:assert";
import {
  useProposalStore,
} from "../src/timeline/proposal.ts";
import type { ActiveProposal } from "../src/timeline/proposal.ts";

function reset(): void {
  useProposalStore.getState().clear();
}

type Phase = "started" | "delta" | "completed";

interface FakeOpts {
  edlText?: string;
  diffHints?: ReadonlyArray<unknown>;
  revision?: number;
  intent?: string;
  explanation?: string;
  rationale?: string;
}

function makeFakeItem(id: string, phase: Phase, opts: FakeOpts = {}): unknown {
  return {
    kind: "proposed_edit",
    id,
    phase,
    source: { source: "user" },
    edl_text: opts.edlText ?? "*** Begin EDL\n*** End EDL\n",
    snapshot: { duration_s: 0, tracks: [] },
    diff_hints: opts.diffHints ?? [],
    summary: "test",
    revision: opts.revision ?? 0,
    intent: opts.intent,
    explanation: opts.explanation,
    evidence: [],
    alternatives: [],
    rationale: opts.rationale,
  };
}

// helper: cast the loose fake to the strict ingest signature without
// dragging the protocol type into every call site.
function ingest(fake: unknown): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  useProposalStore.getState().ingest(fake as any);
}

// An older delta must never roll the review queue back.
{
  reset();
  ingest(makeFakeItem("race", "started", { revision: 0 }));
  ingest(makeFakeItem("race", "delta", { revision: 2, intent: "latest" }));
  ingest(makeFakeItem("race", "delta", { revision: 1, intent: "stale" }));
  assert.equal(useProposalStore.getState().pending[0]!.revision, 2);
  assert.equal(useProposalStore.getState().pending[0]!.intent, "latest");
}

// A late delta cannot resurrect an accepted or rejected proposal.
{
  reset();
  ingest(makeFakeItem("done", "started"));
  ingest(makeFakeItem("done", "completed"));
  ingest(makeFakeItem("done", "delta", { revision: 1 }));
  assert.equal(useProposalStore.getState().pending.length, 0);
}

// Selection and the queue share the same revision, even for background edits.
{
  reset();
  ingest(makeFakeItem("a", "started"));
  ingest(makeFakeItem("b", "started"));
  useProposalStore.getState().select("a");
  ingest(makeFakeItem("b", "delta", { revision: 1, intent: "background" }));
  assert.equal(useProposalStore.getState().active!.callId, "a");
  useProposalStore.getState().select("b");
  assert.strictEqual(useProposalStore.getState().active,
    useProposalStore.getState().pending.find((p) => p.callId === "b"));
  ingest(makeFakeItem("b", "started", { revision: 0 }));
  assert.equal(useProposalStore.getState().active!.revision, 1);
  ingest(makeFakeItem("b", "completed"));
  assert.equal(useProposalStore.getState().active!.callId, "a");
}

// ingest adds a new proposal
{
  reset();
  ingest(makeFakeItem("p1", "started"));
  assert.equal(useProposalStore.getState().pending.length, 1);
  assert.equal(useProposalStore.getState().pending[0]!.callId, "p1");
}

// re-ingest same id replaces, preserves firstSeenAt across started→delta
{
  reset();
  ingest(makeFakeItem("p1", "started", { revision: 1 }));
  const t0 = useProposalStore.getState().pending[0]!.firstSeenAt;
  // Force the clock forward so a regression that re-computes
  // firstSeenAt would be detected.
  const originalNow = Date.now;
  Date.now = () => t0 + 5_000;
  try {
    ingest(makeFakeItem("p1", "delta", { revision: 2, intent: "refined" }));
  } finally {
    Date.now = originalNow;
  }
  const state = useProposalStore.getState();
  assert.equal(state.pending.length, 1);
  assert.equal(state.pending[0]!.firstSeenAt, t0);
  assert.equal(state.pending[0]!.intent, "refined");
  assert.equal(state.pending[0]!.phase, "delta");
}

// rationale lands intact on Started and survives a rationale-less Delta.
// Locks the Wave 3 contract: the Brief / pill tooltip / inspector all
// read `pending[i].rationale`, so a regression that drops the field on
// projection would silently strip "why" from every proposal in flight.
{
  reset();
  ingest(
    makeFakeItem("p1", "started", {
      revision: 1,
      rationale: "trimmed 0.42s silence per podcast defaults",
    }),
  );
  assert.equal(
    useProposalStore.getState().pending[0]!.rationale,
    "trimmed 0.42s silence per podcast defaults",
  );
  // Drag-adjust Delta — no rationale on the wire; the projection must
  // preserve the prior value rather than clobber it to undefined.
  ingest(makeFakeItem("p1", "delta", { revision: 2 }));
  assert.equal(
    useProposalStore.getState().pending[0]!.rationale,
    "trimmed 0.42s silence per podcast defaults",
  );
  // A Delta that *does* carry a refined rationale wins.
  ingest(
    makeFakeItem("p1", "delta", {
      revision: 3,
      rationale: "kept handoff cadence under hostB laugh",
    }),
  );
  assert.equal(
    useProposalStore.getState().pending[0]!.rationale,
    "kept handoff cadence under hostB laugh",
  );
}

// Legacy producers omit rationale entirely; the projection must
// tolerate an undefined field rather than crash.
{
  reset();
  ingest(makeFakeItem("p1", "started"));
  assert.equal(
    useProposalStore.getState().pending[0]!.rationale,
    undefined,
  );
}

// completed phase removes from the stack
{
  reset();
  ingest(makeFakeItem("p1", "started"));
  ingest(makeFakeItem("p1", "completed"));
  assert.equal(useProposalStore.getState().pending.length, 0);
}

// clear wipes everything
{
  reset();
  ingest(makeFakeItem("p1", "started"));
  ingest(makeFakeItem("p2", "started"));
  useProposalStore.getState().clear();
  assert.equal(useProposalStore.getState().pending.length, 0);
}

// multiple ids accumulate, oldest first
{
  reset();
  ingest(makeFakeItem("p1", "started"));
  ingest(makeFakeItem("p2", "started"));
  ingest(makeFakeItem("p3", "started"));
  const ids = useProposalStore.getState().pending.map((p) => p.callId);
  assert.deepEqual(ids, ["p1", "p2", "p3"]);
}

// removing one keeps the order of the rest
{
  reset();
  ingest(makeFakeItem("p1", "started"));
  ingest(makeFakeItem("p2", "started"));
  ingest(makeFakeItem("p3", "started"));
  ingest(makeFakeItem("p2", "completed"));
  const ids = useProposalStore.getState().pending.map((p) => p.callId);
  assert.deepEqual(ids, ["p1", "p3"]);
}

// deriveMedium: pure-cut proposal (delete + trim)
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      "*** Begin EDL\n*** Delete Clip\n@@ anchor: clip_uuid=abc\n*** Trim Clip\n@@ anchor: clip_uuid=def\n+ start: 1.000\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "cut");
}

// deriveMedium: color (set_color_correction)
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      "*** Begin EDL\n*** Set Color Correction\n@@ anchor: clip_uuid=abc\n+ exposure_ev: 0.500\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "color");
}

// deriveMedium: color (apply_lut)
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      "*** Begin EDL\n*** Apply LUT\n@@ anchor: clip_uuid=abc\n+ lut_path: /foo.cube\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "color");
}

// deriveMedium: audio (volume + fade together still classifies as audio)
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      "*** Begin EDL\n*** Set Volume\n@@ anchor: clip_uuid=abc\n+ value: 0.500\n*** Set Audio Fade\n@@ anchor: clip_uuid=abc\n+ fade_in_s: 0.500\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "audio");
}

// deriveMedium: transition
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      "*** Begin EDL\n*** Insert Transition\n@@ between: clip_uuid=a and clip_uuid=b\n+ kind: dissolve\n+ duration_s: 0.500\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "transition");
}

// deriveMedium: title
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      '*** Begin EDL\n*** Insert Title\n+ start_s: 0.000\n+ end_s: 2.000\n+ text: "hi"\n*** End EDL\n',
  });
  assert.equal(deriveMedium(fake), "title");
}

// deriveMedium: b-roll from diff hints
{
  const fake: ActiveProposal = makeActiveFake({
    edlText: "*** Begin EDL\n*** End EDL\n",
    diffHints: [
      { kind: "insert_b_roll", op_index: 0, track_index: 1, item_index: 0 },
    ] as unknown as ActiveProposal["diffHints"],
  });
  assert.equal(deriveMedium(fake), "broll");
}

// deriveMedium: mixed (cut + color in the same proposal)
{
  const fake: ActiveProposal = makeActiveFake({
    edlText:
      "*** Begin EDL\n*** Delete Clip\n@@ anchor: clip_uuid=a\n*** Set Color Correction\n@@ anchor: clip_uuid=b\n+ exposure_ev: 0.500\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "mixed");
}

// deriveMedium: empty / unknown -> other
{
  const fake: ActiveProposal = makeActiveFake({
    edlText: "*** Begin EDL\n*** End EDL\n",
  });
  assert.equal(deriveMedium(fake), "other");
}

console.log("pending-proposals: OK");

// Build a minimal ActiveProposal-shaped object for direct deriveMedium
// tests. We only fill the fields deriveMedium actually reads.
function makeActiveFake(opts: {
  edlText: string;
  diffHints?: ActiveProposal["diffHints"];
}): ActiveProposal {
  return {
    callId: "x",
    phase: "started",
    source: { source: "user" },
    edlText: opts.edlText,
    snapshot: { duration_s: 0, tracks: [] } as unknown as ActiveProposal["snapshot"],
    diffHints: opts.diffHints ?? [],
    summary: "",
    revision: 0,
    evidence: [],
    alternatives: [],
  };
}
