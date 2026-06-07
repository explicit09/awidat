import assert from "node:assert/strict";
import { droppedImportPaths } from "../src/media/dropImportPaths.ts";

const paths = droppedImportPaths([
  { path: "/Volumes/Media/a.mp4" },
  { path: "" },
  { path: "/Volumes/Media/a.mp4" },
  { name: "browser-only.mov" },
  { path: "/Volumes/Media/b.mov" },
]);

assert.deepEqual(paths, ["/Volumes/Media/a.mp4", "/Volumes/Media/b.mov"]);

console.log("media-drop-paths ok");
