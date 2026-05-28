/**
 * Pure-logic tests for the StatusPill primitive. We don't render the
 * React tree — we exercise the label/percent computation that drives it.
 */
import { strict as assert } from "node:assert";
import { resolveStatusLabel } from "../src/ui/primitives/StatusPill.ts";

// Default labels per family/state
assert.equal(resolveStatusLabel({ family: "job", state: "idle" }), "Idle");
assert.equal(resolveStatusLabel({ family: "job", state: "running" }), "Running");
assert.equal(resolveStatusLabel({ family: "job", state: "ready" }), "Ready");
assert.equal(resolveStatusLabel({ family: "job", state: "failed" }), "Failed");
assert.equal(resolveStatusLabel({ family: "proposal", state: "proposed" }), "Proposed");
assert.equal(resolveStatusLabel({ family: "proposal", state: "accepted" }), "Accepted");
assert.equal(resolveStatusLabel({ family: "proposal", state: "rejected" }), "Rejected");
assert.equal(resolveStatusLabel({ family: "proposal", state: "revised" }), "Revised");

// Custom label override wins
assert.equal(
  resolveStatusLabel({ family: "job", state: "running", label: "Indexing" }),
  "Indexing",
);

// Percent appends to running label
assert.equal(
  resolveStatusLabel({ family: "job", state: "running", percent: 56 }),
  "Running · 56%",
);
assert.equal(
  resolveStatusLabel({ family: "job", state: "running", label: "Indexing", percent: 56 }),
  "Indexing · 56%",
);

// Percent on non-running is a type error at compile time AND ignored at runtime
// (we test runtime; the type system enforces the rest)
assert.equal(
  resolveStatusLabel({ family: "job", state: "ready", percent: 100 } as any),
  "Ready",
  "percent must be ignored when state !== running",
);

// Percent clamping
assert.equal(resolveStatusLabel({ family: "job", state: "running", percent: -5 }), "Running · 0%");
assert.equal(resolveStatusLabel({ family: "job", state: "running", percent: 1000 }), "Running · 100%");
assert.equal(resolveStatusLabel({ family: "job", state: "running", percent: 56.7 }), "Running · 57%");

console.log("status-pill: OK");
