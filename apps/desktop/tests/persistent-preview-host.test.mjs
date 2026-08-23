import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";

const stageShell = await readFile(new URL("../src/shell/StageShell.tsx", import.meta.url), "utf8");
const segmentedView = await readFile(
  new URL("../src/media/SegmentedVideoView.tsx", import.meta.url),
  "utf8",
);

assert.doesNotMatch(
  stageShell,
  /if \(!hasProject\)\s*\{\s*return/,
  "Project Manager must not unmount the preview host",
);
assert.match(
  stageShell,
  /aria-hidden=\{!hasProject\}/,
  "the inactive workspace must stay mounted but hidden",
);
assert.doesNotMatch(
  segmentedView,
  /if \(segments\.length === 0\)\s*\{\s*const awaitingProxy/,
  "empty timelines must keep the persistent media elements mounted",
);
assert.match(
  segmentedView,
  /releasePreviewMediaElement\(video\);[\s\S]{0,180}\}, \[projectRoot\]\);/,
  "project changes must unload media without replacing the persistent elements",
);

console.log("persistent-preview-host: all assertions passed");
