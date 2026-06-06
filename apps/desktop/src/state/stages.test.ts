/**
 * Pure-logic tests for the stage progress derivation. Run with:
 *   node --experimental-strip-types src/state/stages.test.ts
 * (or via the package test runner once it's wired up).
 *
 * Stays pure / no React / no DOM so it's cheap to run from CLI.
 */
import { strict as assert } from "node:assert";
import {
  stageFromWorkspaceShortcut,
  stageProgress,
  STAGES,
  WORKSPACE_DESTINATIONS,
  WORKSPACE_SHORTCUTS,
  type Stage,
} from "./stages.ts";

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
    assert.equal(stageProgress("edit", "edit", new Set<Stage>(["edit"])), "current");
  });

  it("marks visited non-current stages as 'complete'", () => {
    const visited = new Set<Stage>(["edit", "deliver"]);
    assert.equal(stageProgress("edit", "deliver", visited), "complete");
  });

  it("marks unvisited stages as 'upcoming'", () => {
    const visited = new Set<Stage>(["edit"]);
    assert.equal(stageProgress("deliver", "edit", visited), "upcoming");
  });

  it("STAGES order matches the simplified editor workflow", () => {
    assert.deepEqual([...STAGES], ["edit", "deliver"]);
  });

  it("workspace destinations expose scheduling from the product chrome", () => {
    assert.deepEqual([...WORKSPACE_DESTINATIONS], [
      "edit",
      "deliver",
      "schedule",
      "skills",
      "history",
    ]);
  });

  it("workspace shortcuts route directly to schedule and other destinations", () => {
    assert.equal(stageFromWorkspaceShortcut("1", true), "edit");
    assert.equal(stageFromWorkspaceShortcut("2", true), "deliver");
    assert.equal(stageFromWorkspaceShortcut("3", true), "schedule");
    assert.equal(stageFromWorkspaceShortcut("4", true), "skills");
    assert.equal(stageFromWorkspaceShortcut("5", true), "history");
    assert.equal(stageFromWorkspaceShortcut("3", false), null);
    assert.equal(stageFromWorkspaceShortcut("6", true), null);
  });

  it("workspace shortcut labels match destination order", () => {
    assert.deepEqual(WORKSPACE_SHORTCUTS, [
      { stage: "edit", keys: "⌘1", label: "Edit" },
      { stage: "deliver", keys: "⌘2", label: "Deliver" },
      { stage: "schedule", keys: "⌘3", label: "Schedule" },
      { stage: "skills", keys: "⌘4", label: "Skills" },
      { stage: "history", keys: "⌘5", label: "History" },
    ]);
  });
});
