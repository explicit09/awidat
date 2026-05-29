// Tests for useBriefProposalsStore — the unified Brief stack that
// merges agent-initiated ApprovalRequests with user/agent-initiated
// ProposedEdits.
//
// The store mirrors `usePendingProposals` for ProposedEdit-backed rows
// (no copy of state), and owns its own ApprovalRequest map. We exercise:
//
//  1. ApprovalRequest enters via ingestApproval, lands on the stack.
//  2. ProposedEdit added via the proposals store is picked up by pending().
//  3. Medium discrimination — apply_edl → cut, set_color_correction →
//     color, use_broll → broll, bash → other, unknown → cut.
//  4. accept() and reject() route to the right backend dispatch and
//     remove the row from the stack.
//  5. Auto-decided exclusion — an ApprovalRequest that arrives
//     phase=completed without ever having been Started must NOT appear.

import { strict as assert } from "node:assert";
import {
  isApprovalRequestItem,
  isProposedEditItem,
  mediumFromApprovalRequest,
  titleFromApprovalRequest,
  useBriefProposalsStore,
  type BriefDispatch,
} from "../src/state/briefProposals.ts";
import { usePendingProposals } from "../src/timeline/pendingProposals.ts";

function resetAll(mockDispatch?: Partial<BriefDispatch>): {
  respondCalls: Array<{ callId: string; decision: string }>;
  acceptCalls: string[];
  rejectCalls: string[];
} {
  const respondCalls: Array<{ callId: string; decision: string }> = [];
  const acceptCalls: string[] = [];
  const rejectCalls: string[] = [];
  const dispatch: BriefDispatch = {
    respondApproval: async (callId, decision) => {
      respondCalls.push({ callId, decision });
    },
    acceptProposal: async (callId) => {
      acceptCalls.push(callId);
    },
    rejectProposal: async (callId) => {
      rejectCalls.push(callId);
    },
    ...mockDispatch,
  };
  useBriefProposalsStore.setState({ approvals: new Map(), dispatch });
  usePendingProposals.setState({ pending: [] });
  return { respondCalls, acceptCalls, rejectCalls };
}

type Phase = "started" | "delta" | "completed";

function makeApproval(
  id: string,
  phase: Phase,
  toolName: string,
  argsSummary = "do the thing",
): unknown {
  return {
    kind: "approval_request",
    id,
    phase,
    tool_name: toolName,
    args_summary: argsSummary,
    capability_metadata: null,
  };
}

function makeProposedEdit(
  id: string,
  phase: Phase,
  opts: { edlText?: string; summary?: string; rationale?: string } = {},
): unknown {
  return {
    kind: "proposed_edit",
    id,
    phase,
    source: { source: "agent", tool_name: "apply_edl" },
    edl_text: opts.edlText ?? "*** Begin EDL\n*** End EDL\n",
    snapshot: { duration_s: 0, tracks: [] },
    diff_hints: [],
    summary: opts.summary ?? "Trim 3 clips",
    revision: 0,
    evidence: [],
    alternatives: [],
    rationale: opts.rationale,
  };
}

// helpers cast loose fakes to the strict ingest signatures.
function ingestApproval(fake: unknown): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  useBriefProposalsStore.getState().ingestApproval(fake as any);
}

function ingestPending(fake: unknown): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  usePendingProposals.getState().ingest(fake as any);
}

// 1. ApprovalRequest enters via ingestApproval and is queryable
{
  resetAll();
  ingestApproval(makeApproval("a1", "started", "apply_edl", "Trim 0:12 — 0:12.42"));
  const pending = useBriefProposalsStore.getState().pending();
  assert.equal(pending.length, 1);
  const row = pending[0]!;
  assert.equal(row.id, "a1");
  assert.equal(row.source, "approval");
  assert.equal(row.medium, "cut");
  assert.equal(row.title, "Trim 0:12 — 0:12.42");
  assert.equal(row.toolName, "apply_edl");
  assert.equal(row.rationale, undefined);
}

// 2. ProposedEdit added via pending-proposals store is surfaced
{
  resetAll();
  ingestPending(
    makeProposedEdit("p1", "started", {
      edlText: "*** Begin EDL\n*** Set Color Correction\n@@ anchor: clip_uuid=a\n+ exposure_ev: 0.500\n*** End EDL\n",
      summary: "Boost exposure on clip A",
      rationale: "exposure was 0.5 stops below target",
    }),
  );
  const pending = useBriefProposalsStore.getState().pending();
  assert.equal(pending.length, 1);
  const row = pending[0]!;
  assert.equal(row.id, "p1");
  assert.equal(row.source, "proposed_edit");
  assert.equal(row.medium, "color");
  assert.equal(row.title, "Boost exposure on clip A");
  assert.equal(row.rationale, "exposure was 0.5 stops below target");
  assert.equal(row.toolName, undefined);
}

// 3a. mediumFromApprovalRequest — apply_edl → cut
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "apply_edl",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "cut");
}

// 3b. set_color_correction → color
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "set_color_correction",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "color");
}

// 3c. use_broll → broll
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "use_broll",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "broll");
}

// 3d. set_volume → audio
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "set_volume",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "audio");
}

// 3e. insert_title → title
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "insert_title",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "title");
}

// 3f. insert_transition → transition
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "insert_transition",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "transition");
}

// 3g. bash → other (infrastructure approval, not an editorial medium)
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "bash",
    args_summary: "rm -rf",
    capability_metadata: null,
  });
  assert.equal(medium, "other");
}

// 3h. apply_patch → other
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "apply_patch",
    args_summary: "edit foo.txt",
    capability_metadata: null,
  });
  assert.equal(medium, "other");
}

// 3i. unknown → cut fallback
{
  const medium = mediumFromApprovalRequest({
    kind: "approval_request",
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    id: "x" as any,
    phase: "started",
    tool_name: "some_future_tool",
    args_summary: "",
    capability_metadata: null,
  });
  assert.equal(medium, "cut");
}

// 4. titleFromApprovalRequest uses args_summary when present, tool name
//    otherwise. Trims and bounds length.
{
  assert.equal(
    titleFromApprovalRequest({
      kind: "approval_request",
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      id: "x" as any,
      phase: "started",
      tool_name: "apply_edl",
      args_summary: "Trim 0:12 — 0:12.42",
      capability_metadata: null,
    }),
    "Trim 0:12 — 0:12.42",
  );
  assert.equal(
    titleFromApprovalRequest({
      kind: "approval_request",
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      id: "x" as any,
      phase: "started",
      tool_name: "apply_edl",
      args_summary: "   ",
      capability_metadata: null,
    }),
    "apply_edl",
  );
}

// 5a. accept() on an approval dispatches respond_approval and removes row
{
  const tracker = resetAll();
  ingestApproval(makeApproval("a1", "started", "apply_edl"));
  await useBriefProposalsStore.getState().accept("a1");
  assert.deepEqual(tracker.respondCalls, [{ callId: "a1", decision: "allow" }]);
  assert.equal(useBriefProposalsStore.getState().pending().length, 0);
}

// 5b. reject() on an approval dispatches deny and removes row
{
  const tracker = resetAll();
  ingestApproval(makeApproval("a1", "started", "apply_edl"));
  await useBriefProposalsStore.getState().reject("a1", "user said no");
  assert.deepEqual(tracker.respondCalls, [{ callId: "a1", decision: "deny" }]);
  assert.equal(useBriefProposalsStore.getState().pending().length, 0);
}

// 5c. accept() on a proposed_edit dispatches editorDispatch.acceptProposal
{
  const tracker = resetAll();
  ingestPending(makeProposedEdit("p1", "started"));
  await useBriefProposalsStore.getState().accept("p1");
  assert.deepEqual(tracker.acceptCalls, ["p1"]);
  assert.equal(tracker.respondCalls.length, 0);
}

// 5d. reject() on a proposed_edit dispatches editorDispatch.rejectProposal
{
  const tracker = resetAll();
  ingestPending(makeProposedEdit("p1", "started"));
  await useBriefProposalsStore.getState().reject("p1");
  assert.deepEqual(tracker.rejectCalls, ["p1"]);
  assert.equal(tracker.respondCalls.length, 0);
}

// 6. Auto-decided exclusion — an ApprovalRequest that arrives directly
//    at phase=completed (e.g. codex auto-decided it on the same tick as
//    history replay, or a Started-Completed race) must never appear on
//    the stack. We send only the Completed phase; the row count stays 0.
{
  resetAll();
  ingestApproval(makeApproval("a-auto", "completed", "apply_edl"));
  assert.equal(useBriefProposalsStore.getState().pending().length, 0);
}

// 7. Completed phase removes an existing approval — proves the same
//    code path drops the row on user-decided AND auto-decided flows.
{
  resetAll();
  ingestApproval(makeApproval("a1", "started", "apply_edl"));
  assert.equal(useBriefProposalsStore.getState().pending().length, 1);
  ingestApproval(makeApproval("a1", "completed", "apply_edl"));
  assert.equal(useBriefProposalsStore.getState().pending().length, 0);
}

// 8. Mixed stack — approvals and proposed_edits interleave by firstSeenAt
//    newest-first. Lock the contract: an approval added LATER appears
//    BEFORE an earlier proposed_edit.
{
  resetAll();
  // Older proposed_edit first.
  const t0 = Date.now();
  const originalNow = Date.now;
  Date.now = () => t0;
  try {
    ingestPending(makeProposedEdit("p1", "started"));
  } finally {
    Date.now = originalNow;
  }
  // Newer approval.
  Date.now = () => t0 + 10_000;
  try {
    ingestApproval(makeApproval("a1", "started", "apply_edl"));
  } finally {
    Date.now = originalNow;
  }
  const pending = useBriefProposalsStore.getState().pending();
  assert.equal(pending.length, 2);
  assert.equal(pending[0]!.id, "a1");
  assert.equal(pending[0]!.source, "approval");
  assert.equal(pending[1]!.id, "p1");
  assert.equal(pending[1]!.source, "proposed_edit");
}

// 9. clear() drops only approvals — proposed_edits are owned by
//    usePendingProposals and aren't this store's to forget.
{
  resetAll();
  ingestApproval(makeApproval("a1", "started", "apply_edl"));
  ingestPending(makeProposedEdit("p1", "started"));
  useBriefProposalsStore.getState().clear();
  // Approvals gone, proposed_edits still visible through pending()
  // because they live in usePendingProposals.
  const pending = useBriefProposalsStore.getState().pending();
  assert.equal(pending.length, 1);
  assert.equal(pending[0]!.source, "proposed_edit");
}

// 10. Type guards are correct
{
  const approval = makeApproval("a1", "started", "apply_edl");
  const proposed = makeProposedEdit("p1", "started");
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  assert.equal(isApprovalRequestItem(approval as any), true);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  assert.equal(isProposedEditItem(approval as any), false);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  assert.equal(isApprovalRequestItem(proposed as any), false);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  assert.equal(isProposedEditItem(proposed as any), true);
}

console.log("pending-brief-proposals: OK");
