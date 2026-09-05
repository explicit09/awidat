// Wave 5 C2: store-level test for the rejection → append_feedback
// flow. Locks the contract that:
//
//   1. Rejecting any Brief row (approval / proposed_edit / broll) fires
//      the dispatch's `appendFeedback` with the correct shape.
//   2. The disk write is fire-and-forget — a rejected promise from
//      appendFeedback does NOT propagate into reject() (the
//      localStorage History store is the UI source of truth and must
//      stay consistent regardless).
//   3. No project loaded → no appendFeedback call (silent drop, same
//      policy as the localStorage logger).
//   4. Silent rejects (no reason supplied) still log a row with
//      `reason: null` so frequency analysis is possible.
//
// This is a wire-shape test, not a backend integration test: we mock
// the dispatch's appendFeedback and assert on the payload. The
// `cargo test commands::feedback` suite covers the on-disk side.

import { strict as assert } from "node:assert";
import {
  useBriefProposalsStore,
  type BriefDispatch,
  type FeedbackPayload,
} from "../src/state/briefProposals.ts";
import { useProposalStore } from "../src/timeline/proposal.ts";
import { useProjectStore } from "../src/app/state.ts";

interface FeedbackCall {
  projectPath: string;
  entry: FeedbackPayload;
}

function reset(opts: {
  projectPath?: string | null;
  appendFeedback?: BriefDispatch["appendFeedback"];
} = {}): { feedbackCalls: FeedbackCall[] } {
  const feedbackCalls: FeedbackCall[] = [];
  const dispatch: BriefDispatch = {
    respondApproval: async () => {},
    acceptProposal: async () => {},
    rejectProposal: async () => {},
    acceptBroll: async () => {},
    rejectBroll: async () => {},
    captureTimelineSnapshot: async () => undefined,
    appendFeedback:
      opts.appendFeedback ??
      (async (projectPath, entry) => {
        feedbackCalls.push({ projectPath, entry });
      }),
  };
  useBriefProposalsStore.setState({
    approvals: new Map(),
    brollProposals: new Map(),
    brollDecided: new Set(),
    dispatch,
  });
  useProposalStore.setState({ pending: [] });
  // Set / unset project root for logFeedback's gate.
  useProjectStore.setState({
    current: opts.projectPath === undefined ? "/proj-a" : opts.projectPath,
  });
  return { feedbackCalls };
}

function ingestApproval(
  id: string,
  toolName: string,
  argsSummary: string,
  rationale?: string,
): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  useBriefProposalsStore.getState().ingestApproval({
    kind: "approval_request",
    id,
    phase: "started",
    tool_name: toolName,
    args_summary: argsSummary,
    capability_metadata: null,
    rationale,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  } as any);
}

function ingestProposedEdit(id: string, summary: string, rationale?: string): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  useProposalStore.getState().ingest({
    kind: "proposed_edit",
    id,
    phase: "started",
    source: { source: "agent", tool_name: "apply_edl" },
    edl_text: "*** Begin EDL\n*** End EDL\n",
    snapshot: { duration_s: 0, tracks: [] },
    diff_hints: [],
    summary,
    revision: 0,
    evidence: [],
    alternatives: [],
    rationale,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  } as any);
}

// settle() drains microtasks so the fire-and-forget appendFeedback
// promise from inside reject() has a chance to land before we assert.
async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

// 1. Rejecting an approval row with a reason emits the feedback row.
{
  const { feedbackCalls } = reset({ projectPath: "/proj-a" });
  ingestApproval("a1", "apply_edl", "Trim 0:12 — 0:12.42", "dead air");
  const t0 = 1_700_000_000_000;
  const origNow = Date.now;
  Date.now = () => t0;
  try {
    await useBriefProposalsStore.getState().reject("a1", "Too aggressive");
  } finally {
    Date.now = origNow;
  }
  await settle();
  assert.equal(feedbackCalls.length, 1, "exactly one feedback row written");
  const { projectPath, entry } = feedbackCalls[0]!;
  assert.equal(projectPath, "/proj-a");
  assert.equal(entry.ts, Math.floor(t0 / 1000));
  assert.equal(entry.medium, "cut");
  assert.equal(entry.title, "Trim 0:12 — 0:12.42");
  assert.equal(entry.rationale, "dead air");
  assert.equal(entry.reason, "Too aggressive");
}

// 2. Rejecting a proposed_edit row also emits the row. The medium is
//    whatever the pending-proposals store derived from the EDL — an
//    empty EDL falls through to "other", which is the exact case we
//    want to log so the agent sees those rejections too.
{
  const { feedbackCalls } = reset({ projectPath: "/proj-a" });
  ingestProposedEdit("p1", "Boost exposure", "exposure under target");
  await useBriefProposalsStore.getState().reject("p1", "Off-topic");
  await settle();
  assert.equal(feedbackCalls.length, 1);
  assert.equal(feedbackCalls[0]!.entry.title, "Boost exposure");
  assert.equal(feedbackCalls[0]!.entry.rationale, "exposure under target");
  assert.equal(feedbackCalls[0]!.entry.reason, "Off-topic");
}

// 3. Silent reject (no reason) still emits a row with reason: null —
//    we want the agent-side frequency view to see these.
{
  const { feedbackCalls } = reset({ projectPath: "/proj-a" });
  ingestApproval("a1", "apply_edl", "Trim 0:12", undefined);
  await useBriefProposalsStore.getState().reject("a1");
  await settle();
  assert.equal(feedbackCalls.length, 1);
  assert.equal(feedbackCalls[0]!.entry.reason, null);
  assert.equal(feedbackCalls[0]!.entry.rationale, null);
}

// 4. Empty / whitespace-only reason normalises to null.
{
  const { feedbackCalls } = reset({ projectPath: "/proj-a" });
  ingestApproval("a1", "apply_edl", "Trim 0:12");
  await useBriefProposalsStore.getState().reject("a1", "   ");
  await settle();
  assert.equal(feedbackCalls.length, 1);
  assert.equal(feedbackCalls[0]!.entry.reason, null);
}

// 5. No project loaded → no appendFeedback call. The reject still
//    succeeds (logDecision drops the same way) — the disk log is
//    non-load-bearing chrome.
{
  const { feedbackCalls } = reset({ projectPath: null });
  ingestApproval("a1", "apply_edl", "Trim 0:12");
  await useBriefProposalsStore.getState().reject("a1", "Too aggressive");
  await settle();
  assert.equal(feedbackCalls.length, 0);
}

// 6. A failing appendFeedback must NOT unwind reject(). The row
//    leaves the stack and the await on reject() resolves cleanly.
{
  let invoked = false;
  reset({
    projectPath: "/proj-a",
    appendFeedback: async () => {
      invoked = true;
      throw new Error("backend unavailable");
    },
  });
  ingestApproval("a1", "apply_edl", "Trim 0:12");
  await useBriefProposalsStore.getState().reject("a1", "Too aggressive");
  await settle();
  assert.equal(invoked, true, "appendFeedback was invoked");
  assert.equal(
    useBriefProposalsStore.getState().pending().length,
    0,
    "row left the stack despite the feedback write failing",
  );
}

// 7. Rejecting a broll proposal emits a feedback row with medium=broll.
{
  const { feedbackCalls } = reset({ projectPath: "/proj-a" });
  useBriefProposalsStore.getState().ingestBroll({
    id: "job-1",
    title: "Generated B-roll · openrouter",
    rationale: "quiet office",
    firstSeenAt: 1_700_000_000_000,
    metadata: {
      prompt: "quiet office",
      provider: "openrouter",
      videoPath: "/abs/job-1.mp4",
    },
  });
  await useBriefProposalsStore.getState().reject("job-1", "Wrong vibe");
  await settle();
  assert.equal(feedbackCalls.length, 1);
  assert.equal(feedbackCalls[0]!.entry.medium, "broll");
  assert.equal(feedbackCalls[0]!.entry.title, "Generated B-roll · openrouter");
  assert.equal(feedbackCalls[0]!.entry.reason, "Wrong vibe");
}

// 8. Accept does NOT emit a feedback row — the JSONL is reject-only.
{
  const { feedbackCalls } = reset({ projectPath: "/proj-a" });
  ingestApproval("a1", "apply_edl", "Trim 0:12");
  await useBriefProposalsStore.getState().accept("a1");
  await settle();
  assert.equal(feedbackCalls.length, 0);
}

console.log("feedback-log: OK");
