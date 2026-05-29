/**
 * Pure-logic tests for the proposal-history store helpers.
 * Exercises buildHistoryEntry / entriesForProject / serialize /
 * deserialize without spinning up a Zustand store or browser
 * localStorage.
 */
import { strict as assert } from "node:assert";
import {
  buildHistoryEntry,
  deserialize,
  entriesForProject,
  serialize,
  sortNewestFirst,
  type HistoryEntry,
} from "../src/state/proposalHistory.ts";
import type { BriefProposal } from "../src/state/briefProposals.ts";

function makeProposal(overrides: Partial<BriefProposal> = {}): BriefProposal {
  return {
    id: "call-1",
    source: "proposed_edit",
    medium: "cut",
    title: "Trim 0:12 — 0:12.42",
    rationale: "dead air",
    firstSeenAt: 1_700_000_000_000,
    toolName: undefined,
    ...overrides,
  };
}

// buildHistoryEntry copies BriefProposal fields and stamps the decision
{
  const entry = buildHistoryEntry({
    proposal: makeProposal(),
    projectPath: "/proj-a",
    decision: "accepted",
    decidedAt: 1_700_000_010_000,
  });
  assert.equal(entry.id, "call-1");
  assert.equal(entry.projectPath, "/proj-a");
  assert.equal(entry.medium, "cut");
  assert.equal(entry.source, "proposed_edit");
  assert.equal(entry.title, "Trim 0:12 — 0:12.42");
  assert.equal(entry.rationale, "dead air");
  assert.equal(entry.proposedAt, 1_700_000_000_000);
  assert.equal(entry.decidedAt, 1_700_000_010_000);
  assert.equal(entry.decision, "accepted");
  assert.equal(entry.toolName, undefined);
  assert.equal(entry.brollMetadata, undefined);
}

// buildHistoryEntry preserves brollMetadata for broll-source rows
{
  const entry = buildHistoryEntry({
    proposal: makeProposal({
      id: "job-7",
      source: "broll",
      medium: "broll",
      title: "Stock waterfall, 8s",
      brollMetadata: {
        prompt: "waterfall, lush, soft light",
        provider: "runway",
        model: "gen3",
        videoPath: "/proj/cache/broll/job-7.mp4",
      },
    }),
    projectPath: "/proj-a",
    decision: "accepted",
  });
  assert.equal(entry.brollMetadata?.provider, "runway");
  assert.equal(entry.brollMetadata?.videoPath, "/proj/cache/broll/job-7.mp4");
}

// buildHistoryEntry defaults decidedAt to Date.now() when omitted
{
  const before = Date.now();
  const entry = buildHistoryEntry({
    proposal: makeProposal(),
    projectPath: "/proj-a",
    decision: "rejected",
  });
  const after = Date.now();
  assert.ok(entry.decidedAt >= before && entry.decidedAt <= after);
}

// entriesForProject filters by project and sorts newest-first
{
  const entries: HistoryEntry[] = [
    buildHistoryEntry({
      proposal: makeProposal({ id: "a" }),
      projectPath: "/proj-a",
      decision: "accepted",
      decidedAt: 1000,
    }),
    buildHistoryEntry({
      proposal: makeProposal({ id: "b" }),
      projectPath: "/proj-b",
      decision: "rejected",
      decidedAt: 2000,
    }),
    buildHistoryEntry({
      proposal: makeProposal({ id: "c" }),
      projectPath: "/proj-a",
      decision: "rejected",
      decidedAt: 3000,
    }),
  ];
  const filtered = entriesForProject(entries, "/proj-a");
  assert.equal(filtered.length, 2);
  assert.equal(filtered[0].id, "c"); // newest first
  assert.equal(filtered[1].id, "a");
  assert.equal(entriesForProject(entries, "/missing").length, 0);
}

// sortNewestFirst is stable for equal timestamps (does not mutate input)
{
  const entries: HistoryEntry[] = [
    buildHistoryEntry({
      proposal: makeProposal({ id: "a" }),
      projectPath: "/proj-a",
      decision: "accepted",
      decidedAt: 1000,
    }),
    buildHistoryEntry({
      proposal: makeProposal({ id: "b" }),
      projectPath: "/proj-a",
      decision: "rejected",
      decidedAt: 1000,
    }),
  ];
  const sorted = sortNewestFirst(entries);
  // No mutation of the input array.
  assert.equal(entries[0].id, "a");
  // Both timestamps equal — sorted preserves order.
  assert.equal(sorted[0].id, "a");
  assert.equal(sorted[1].id, "b");
}

// serialize → deserialize round-trip preserves entries
{
  const entries: HistoryEntry[] = [
    buildHistoryEntry({
      proposal: makeProposal({ id: "a" }),
      projectPath: "/proj-a",
      decision: "accepted",
      decidedAt: 1000,
    }),
    buildHistoryEntry({
      proposal: makeProposal({
        id: "b",
        medium: "color",
        rationale: "exposure low",
      }),
      projectPath: "/proj-a",
      decision: "revised",
      decidedAt: 2000,
    }),
  ];
  const shape = serialize(entries);
  assert.equal(shape.version, 1);
  assert.equal(shape.entries.length, 2);
  const restored = deserialize(shape);
  assert.equal(restored.length, 2);
  assert.equal(restored[0].id, "a");
  assert.equal(restored[1].medium, "color");
  assert.equal(restored[1].decision, "revised");
}

// deserialize is defensive against unknown / malformed shapes
{
  assert.deepEqual(deserialize(undefined), []);
  assert.deepEqual(deserialize(null), []);
  assert.deepEqual(deserialize({}), []);
  assert.deepEqual(deserialize({ version: 99, entries: [] }), []);
  assert.deepEqual(deserialize({ version: 1, entries: "not-an-array" }), []);
  // Malformed entries get dropped, valid ones survive.
  const mixed = deserialize({
    version: 1,
    entries: [
      { id: "valid", projectPath: "/p", title: "ok", medium: "cut", source: "approval", proposedAt: 1, decidedAt: 2, decision: "accepted" },
      { id: "missing-decidedAt" },
      "not-an-object",
      null,
    ],
  });
  assert.equal(mixed.length, 1);
  assert.equal(mixed[0].id, "valid");
}

// deserialize rejects entries with unrecognized decision values
{
  const dropped = deserialize({
    version: 1,
    entries: [
      { id: "x", projectPath: "/p", title: "t", medium: "cut", source: "approval", proposedAt: 1, decidedAt: 2, decision: "ignored" },
    ],
  });
  assert.equal(dropped.length, 0);
}

console.log("proposal-history: OK");
