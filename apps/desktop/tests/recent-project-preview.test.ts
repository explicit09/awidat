import { strict as assert } from "node:assert";

const previewModule = await import("../src/shell/empty/recentProjectPreview.ts").catch(() => null);
assert.ok(previewModule, "recent project preview loader must exist");

const calls: Array<{ command: string; args: unknown }> = [];
const preview = await previewModule.loadRecentProjectPreview(
  "/projects/demo",
  async (command, args) => {
    calls.push({ command, args });
    if (command === "project_thumbnail") return "/projects/demo/.montage/thumbnails/frame.jpg";
    if (command === "project_preview_url") return "http://127.0.0.1:4000/media/frame";
    throw new Error(`unexpected command: ${command}`);
  },
);

assert.deepEqual(preview, { src: "http://127.0.0.1:4000/media/frame" });
assert.deepEqual(calls, [
  { command: "project_thumbnail", args: { path: "/projects/demo" } },
  {
    command: "project_preview_url",
    args: {
      projectPath: "/projects/demo",
      mediaPath: "/projects/demo/.montage/thumbnails/frame.jpg",
    },
  },
]);

let urlRequested = false;
const missing = await previewModule.loadRecentProjectPreview(
  "/projects/missing",
  async (command) => {
    if (command === "project_thumbnail") return null;
    urlRequested = true;
    return "unexpected";
  },
);
assert.equal(missing, null);
assert.equal(urlRequested, false, "missing thumbnail must not allocate a streaming URL");

console.log("recent-project-preview: all assertions passed");
