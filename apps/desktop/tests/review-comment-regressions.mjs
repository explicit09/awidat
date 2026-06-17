import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const gradeCanvas = readFileSync(resolve(root, "src/media/GradeCanvas.tsx"), "utf8");
const segmentedVideoView = readFileSync(resolve(root, "src/media/SegmentedVideoView.tsx"), "utf8");
const propertiesPane = readFileSync(resolve(root, "src/properties/PropertiesPane.tsx"), "utf8");
const briefProposals = readFileSync(resolve(root, "src/state/briefProposals.ts"), "utf8");
const appCss = readFileSync(resolve(root, "src/App.css"), "utf8");
const renderTimeline = readFileSync(
  resolve(root, "../../crates/render/src/timeline.rs"),
  "utf8",
);

const checks = [
  [
    "preview LUT cache key includes project root",
    /function previewLutCacheKey\([\s\S]*projectRoot[\s\S]*lutPath[\s\S]*\\u0000/.test(
      gradeCanvas,
    ) && /fetchPreviewLut\(projectRoot, lutPath\)/.test(gradeCanvas),
  ],
  [
    "GradeCanvas receives the active project root",
    /<GradeCanvas[\s\S]+projectRoot=\{projectRoot\}/.test(segmentedVideoView),
  ],
  [
    "LUT state clears before loading a new path",
    /setLut\(null\);[\s\S]+setLutKey\(""\);[\s\S]+fetchPreviewLut\(projectRoot, lutPath\)/.test(
      gradeCanvas,
    ),
  ],
  [
    "color preview clears when proposal dispatch fails",
    /proposeUserEdit\(\[op\]\)[\s\S]+catch\(\(err\) => \{[\s\S]+clearPreviewOverride\(clipUuid\)/.test(
      propertiesPane,
    ),
  ],
  [
    "color preview clears when a proposal is rejected",
    /useColorPreviewOverride/.test(briefProposals) &&
      /reject\(id, reason\)[\s\S]+clearOverride\(\)/.test(briefProposals),
  ],
  [
    "MotionScene preview layers share one ordered render pass",
    /function activeMotionSceneOverlays/.test(segmentedVideoView) &&
      /<TimelineMotionSceneOverlays[\s\S]+overlays=\{activeMotionSceneLayers\}/.test(
        segmentedVideoView,
      ) &&
      !/TimelineMotionShapeOverlays/.test(segmentedVideoView) &&
      !/TimelineMotionImageOverlays/.test(segmentedVideoView) &&
      /\.timeline-motion-scene-layer/.test(appCss),
  ],
  [
    "preloaded slot activation refreshes active media size immediately",
    /activeKeyRef\.current = activeKeyNow;[\s\S]+setActiveKey\(activeKeyNow\);[\s\S]+updateActiveMediaSize\(activeKeyNow, preloaded\)/.test(
      segmentedVideoView,
    ),
  ],
  [
    "MotionScene image opacity uses layer-local alpha time",
    renderTimeline.includes('"overlay.opacity"') &&
      renderTimeline.includes('&format!("(T+{})"') &&
      /motion_scene_image_opacity_uses_layer_local_alpha_clock/.test(renderTimeline),
  ],
];

for (const [label, ok] of checks) {
  if (!ok) throw new Error(`review regression missing: ${label}`);
}

console.log(`review-comment-regressions: OK (${checks.length} checks)`);
