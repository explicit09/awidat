/**
 * Pure-logic tests for the stage progress derivation. Run with:
 *   node --experimental-strip-types src/state/stages.test.ts
 * (or via the package test runner once it's wired up).
 *
 * Stays pure / no React / no DOM so it's cheap to run from CLI.
 */
import { strict as assert } from "node:assert";
import { stageProgress, STAGES, type Stage } from "./stages.ts";

function describe(name: string, fn: () => void) {
  console.log(`\n# ${name}`);
  fn();
}

function it(name: string, fn: () => void) {
  try {
    fn();
    console.log(`  ok  ${name}`);
  } catch (err) {
    console.error(`  FAIL  ${name}`);
    console.error("    " + (err as Error).message);
    process.exitCode = 1;
  }
}

describe("stageProgress", () => {
  it("marks the current stage as 'current'", () => {
    assert.equal(stageProgress("proposal", "proposal", new Set<Stage>(["intent", "indexing", "proposal"])), "current");
  });

  it("marks visited non-current stages as 'complete'", () => {
    const visited = new Set<Stage>(["intent", "indexing", "proposal"]);
    assert.equal(stageProgress("intent", "proposal", visited), "complete");
    assert.equal(stageProgress("indexing", "proposal", visited), "complete");
  });

  it("marks unvisited stages as 'upcoming'", () => {
    const visited = new Set<Stage>(["intent", "indexing", "proposal"]);
    assert.equal(stageProgress("review", "proposal", visited), "upcoming");
    assert.equal(stageProgress("deliver", "proposal", visited), "upcoming");
  });

  it("STAGES order matches the design spec", () => {
    assert.deepEqual([...STAGES], ["intent", "indexing", "proposal", "review", "revise", "deliver"]);
  });
});
