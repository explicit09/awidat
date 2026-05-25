# MotionScene and Remotion Backend Fit

Awidat's native MotionScene lane should remain the first renderer agents target
for procedural visuals that can be represented in the local schema. The native
path keeps previews immediate in the desktop app, keeps render limitations
visible in preflight, and lets simple layers lower through the existing timeline
render pipeline.

Current native subset:

- Text layers lower to title/drawtext behavior.
- Rectangle shape and solid layers lower to native preview overlays and FFmpeg
  drawbox filters.
- Image layers with project-relative still assets lower to native desktop
  preview overlays and FFmpeg image-input overlays. Use these for logos,
  screenshots, product stills, diagrams, charts, and generated PNG overlays.
- Shared transforms support `x`, `y`, `width`, `height`, `opacity`, `fit`,
  `scale`, `anchor_x`, `anchor_y`, and `rotation_deg`. Layer-local
  `params.animations` can keyframe supported overlay parameters such as
  `overlay.x`, `overlay.y`, `overlay.scale`, `overlay.opacity`, and
  `overlay.rotation_deg`; still-image render lowering uses these keyframes
  natively.
- Video/media layers stay stored in `metadata.awidat.motion_scenes` and must
  report explicit limitations until they have preview and render lowering.
  Actual footage should continue to use the existing B-roll/PiP/media overlay
  path.

Remotion can still fit later as an optional advanced backend when a scene needs
React-style composition, richer animation timing, or asset-heavy procedural
graphics that exceed the native subset. It should consume the same MotionScene
document or a deliberate extension of it, rather than becoming the only way
agents express motion graphics.

Practical rule for agents:

- Start with visual reasoning, not a renderer. Detect the need
  (abstract explanation, product/asset mention, factual reference,
  list/process, emotional emphasis, jump-cut cover, chapter transition,
  or sponsor/CTA), then classify the intent (explain, show evidence,
  summarize, decorate lightly, hide edit, emphasize quote, introduce
  chapter, or compare before/after).
- Use native MotionScene first for text, lower-thirds, simple panels, solid
  backgrounds, callout rectangles, still image overlays, and lightweight
  explainer graphics.
- Let `plan_motion_scene` build layered native scenes for explainers:
  background panel, headline, step labels, callout rectangles, and optional
  still image layers when assets exist.
- Use layer-local transform animations for simple procedural motion such as
  fades, slides, scale pops, and rotation on still overlays.
- Use B-roll/PiP for real footage, demos, interviews, and video cutaways.
- Use generated media for missing footage or still assets, then route the
  resulting video through B-roll/PiP or the resulting still through
  MotionScene image layers.
- Use existing FFmpeg/editorial tools for edits to existing footage, audio,
  cuts, speed, transitions, color, and direct clip polish.
- Consider a future Remotion backend only after native planning decides the
  requested visual exceeds the supported MotionScene subset and the user needs a
  generated motion-graphics scene rather than footage or B-roll.
