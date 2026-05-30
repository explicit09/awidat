/**
 * Pure-logic tests for the proposal-history store helpers.
 * Exercises buildHistoryEntry / entriesForProject / serialize /
 * deserialize / enforcePerProjectCap / buildRestoreEntry without
 * spinning up a Zustand store or browser localStorage.
 */
import { strict as assert } from "node:assert";
import {
  MAX_ENTRIES_PER_PROJECT,
  buildHistoryEntry,
  buildRestoreEntry,
  deserialize,
  enforcePerProjectCap,
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

// deserialize accepts the new "restored" decision (Wave 4 W4.5)
{
  const kept = deserialize({
    version: 1,
    entries: [
      {
        id: "restore:abc:99",
        projectPath: "/p",
        title: "Restored: trim",
        medium: "cut",
        source: "proposed_edit",
        proposedAt: 1,
        decidedAt: 2,
        decision: "restored",
      },
    ],
  });
  assert.equal(kept.length, 1);
  assert.equal(kept[0].decision, "restored");
}

// buildHistoryEntry round-trips an inline timelineSnapshot
{
  const snapshot = '{"OTIO_SCHEMA":"Timeline.1","name":"test","tracks":{"name":"tracks","children":[]}}';
  const entry = buildHistoryEntry({
    proposal: makeProposal(),
    projectPath: "/proj-a",
    decision: "accepted",
    decidedAt: 1_700_000_010_000,
    timelineSnapshot: snapshot,
  });
  assert.equal(entry.timelineSnapshot, snapshot);
  // serialize → deserialize preserves the snapshot field.
  const shape = serialize([entry]);
  const restored = deserialize(shape);
  assert.equal(restored.length, 1);
  assert.equal(restored[0].timelineSnapshot, snapshot);
}

// buildRestoreEntry stamps the source title and "restored" decision
{
  const source = buildHistoryEntry({
    proposal: makeProposal({ id: "trim-1", title: "Trim 0:12 — 0:12.42" }),
    projectPath: "/proj-a",
    decision: "accepted",
    decidedAt: 1_000,
    timelineSnapshot: "{}",
  });
  const restore = buildRestoreEntry({
    source,
    projectPath: "/proj-a",
    postRestoreSnapshot: '{"OTIO_SCHEMA":"Timeline.1"}',
    decidedAt: 2_000,
  });
  assert.equal(restore.decision, "restored");
  assert.equal(restore.projectPath, "/proj-a");
  assert.equal(restore.title, "Restored: Trim 0:12 — 0:12.42");
  assert.equal(restore.timelineSnapshot, '{"OTIO_SCHEMA":"Timeline.1"}');
  assert.ok(restore.id.startsWith("restore:trim-1:"));
  // Source decidedAt rides as proposedAt so the row shows when the
  // restored decision originally happened.
  assert.equal(restore.proposedAt, 1_000);
  assert.equal(restore.decidedAt, 2_000);
}

// enforcePerProjectCap is a no-op below the cap
{
  const entries = [
    buildHistoryEntry({
      proposal: makeProposal({ id: "a" }),
      projectPath: "/p",
      decision: "accepted",
      decidedAt: 1,
    }),
    buildHistoryEntry({
      proposal: makeProposal({ id: "b" }),
      projectPath: "/p",
      decision: "accepted",
      decidedAt: 2,
    }),
  ];
  const capped = enforcePerProjectCap(entries);
  assert.equal(capped.length, 2);
  assert.equal(capped[0].id, "a");
  assert.equal(capped[1].id, "b");
}

// enforcePerProjectCap drops the oldest entries beyond MAX per project
{
  // Build (MAX + 5) entries for /p, newest-first (id matches recency).
  const entries: HistoryEntry[] = [];
  for (let i = MAX_ENTRIES_PER_PROJECT + 4; i >= 0; i--) {
    entries.push(
      buildHistoryEntry({
        proposal: makeProposal({ id: `id-${i}` }),
        projectPath: "/p",
        decision: "accepted",
        decidedAt: i, // 0 = oldest, N = newest. Newest already at front.
      }),
    );
  }
  // Sanity: entries arrive newest-first.
  assert.equal(entries[0].decidedAt, MAX_ENTRIES_PER_PROJECT + 4);
  const capped = enforcePerProjectCap(entries);
  assert.equal(capped.length, MAX_ENTRIES_PER_PROJECT);
  // The cap keeps the newest entries — the first one in the cap is the
  // most-recent entry.
  assert.equal(capped[0].decidedAt, MAX_ENTRIES_PER_PROJECT + 4);
  // The last kept entry is exactly at index (MAX - 1) by decidedAt,
  // which means the dropped entries are the oldest 5.
  assert.equal(capped[capped.length - 1].decidedAt, 5);
}

// enforcePerProjectCap is per-project: different project paths each
// get their own MAX budget
{
  const entries: HistoryEntry[] = [];
  // 60 entries for /a, 60 entries for /b, interleaved (newest-first).
  for (let i = 59; i >= 0; i--) {
    entries.push(
      buildHistoryEntry({
        proposal: makeProposal({ id: `a-${i}` }),
        projectPath: "/a",
        decision: "accepted",
        decidedAt: i * 2,
      }),
    );
    entries.push(
      buildHistoryEntry({
        proposal: makeProposal({ id: `b-${i}` }),
        projectPath: "/b",
        decision: "accepted",
        decidedAt: i * 2 + 1,
      }),
    );
  }
  // 120 total, both buckets under MAX (100).
  assert.equal(entries.length, 120);
  const capped = enforcePerProjectCap(entries);
  assert.equal(capped.length, 120); // no eviction — both buckets ≤ MAX.
  const aCount = capped.filter((e) => e.projectPath === "/a").length;
  const bCount = capped.filter((e) => e.projectPath === "/b").length;
  assert.equal(aCount, 60);
  assert.equal(bCount, 60);
}

// enforcePerProjectCap evicts only the over-cap project; the other
// project's entries are untouched
{
  // /a gets MAX + 10 entries (over-cap). /b gets 5 entries (under-cap).
  const entries: HistoryEntry[] = [];
  for (let i = MAX_ENTRIES_PER_PROJECT + 9; i >= 0; i--) {
    entries.push(
      buildHistoryEntry({
        proposal: makeProposal({ id: `a-${i}` }),
        projectPath: "/a",
        decision: "accepted",
        decidedAt: 1_000_000 + i,
      }),
    );
  }
  for (let i = 4; i >= 0; i--) {
    entries.push(
      buildHistoryEntry({
        proposal: makeProposal({ id: `b-${i}` }),
        projectPath: "/b",
        decision: "accepted",
        decidedAt: 500_000 + i,
      }),
    );
  }
  const capped = enforcePerProjectCap(entries);
  const aCount = capped.filter((e) => e.projectPath === "/a").length;
  const bCount = capped.filter((e) => e.projectPath === "/b").length;
  assert.equal(aCount, MAX_ENTRIES_PER_PROJECT);
  assert.equal(bCount, 5);
}

console.log("proposal-history: OK");
