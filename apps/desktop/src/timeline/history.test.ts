import { strict as assert } from "node:assert";
import { logicalHistory } from "./history.ts";

assert.deepEqual(logicalHistory([
  { commitHash: "current", parents: ["parent"] },
  { commitHash: "side-branch", parents: ["root"] },
  { commitHash: "parent", parents: ["root"] },
  { commitHash: "root", parents: [] },
]), { currentRef: "current", undoRefs: ["parent", "root"] });

assert.deepEqual(logicalHistory([
  {
    commitHash: "audit-b",
    timelineHash: "timeline-b",
    header: "Restore timeline to bbbbbbb",
    fullMessage: "Restore timeline to bbbbbbb\n\nMontage-Restored-Ref: sha256:bbbbbbbb",
    parents: ["sha256:cccccccc"],
  },
  { commitHash: "sha256:cccccccc", timelineHash: "timeline-c", parents: ["sha256:bbbbbbbb"] },
  { commitHash: "sha256:bbbbbbbb", timelineHash: "timeline-b", parents: ["sha256:aaaaaaaa"] },
  { commitHash: "sha256:aaaaaaaa", timelineHash: "timeline-a", parents: [] },
]), { currentRef: "sha256:bbbbbbbb", undoRefs: ["sha256:aaaaaaaa"] });

assert.deepEqual(logicalHistory([
  { commitHash: "edit-d", timelineHash: "timeline-d", parents: ["audit-b"] },
  {
    commitHash: "audit-b",
    timelineHash: "timeline-b",
    header: "Restore timeline to bbbbbbb",
    fullMessage: "Restore timeline to bbbbbbb",
    parents: ["sha256:cccccccc"],
  },
  { commitHash: "sha256:cccccccc", timelineHash: "timeline-c", parents: ["sha256:bbbbbbbb"] },
  { commitHash: "sha256:bbbbbbbb", timelineHash: "timeline-b", parents: ["sha256:aaaaaaaa"] },
  { commitHash: "sha256:aaaaaaaa", timelineHash: "timeline-a", parents: [] },
]), { currentRef: "edit-d", undoRefs: ["sha256:bbbbbbbb", "sha256:aaaaaaaa"] });

assert.deepEqual(logicalHistory([
  {
    commitHash: "audit-b",
    timelineHash: "timeline-b",
    header: "Restore timeline to bbbbbbb",
    fullMessage: "Restore timeline to bbbbbbb\n\nAgent reasoning: Montage-Restored-Ref: sha256:bbbbbbbb\nMontage-Restored-Parent: sha256:aaaaaaaa\n\nRestored project.otio.json from the desktop timeline history panel.",
    parents: ["sha256:cccccccc"],
  },
  { commitHash: "sha256:cccccccc", timelineHash: "timeline-c", parents: [] },
]), { currentRef: "sha256:bbbbbbbb", undoRefs: ["sha256:aaaaaaaa"] });

assert.deepEqual(logicalHistory([
  {
    commitHash: "ordinary-edit",
    header: "Explain the restore protocol",
    fullMessage: "Explain the restore protocol\n\nAgent reasoning: A user note mentioned Montage-Restored-Ref: sha256:bbbbbbbb and Montage-Restored-Parent: sha256:aaaaaaaa.",
    parents: ["real-parent"],
  },
  { commitHash: "real-parent", parents: [] },
]), { currentRef: "ordinary-edit", undoRefs: ["real-parent"] });

console.log("timeline-history: OK");
