# Branded Animated Motion Graphics — Implementation Plan

## Problem (gap #4)

Montage cannot natively produce branded animated graphics: custom-font cards,
animated social icons, vector/motion overlays (intro/outro stings,
lower-thirds, animated stat cards, social handles, subscribe bumpers). The
native MotionScene lane only lowers a constrained subset — text/drawtext,
rectangle/solid drawbox, and project-relative still-image overlays — and
MotionScene *video* layers are explicitly **not** lowered:

- `crates/render/src/timeline.rs:1986-1993` — `MotionSceneLayerKind::Shape |
  Solid | Image => {}` are handled; everything else (notably video/media
  layers) pushes a `"layer kind is stored but not lowered by the native
  MotionScene renderer yet"` limitation.
- `docs/motion-scene-remotion-backend.md:24-27` — "Video/media layers stay
  stored in `metadata.montage.motion_scenes` and must report explicit
  limitations until they have preview and render lowering. Actual footage
  should continue to use the existing B-roll/PiP/media overlay path."
- `crates/proto/src/transitions.rs:499-500` — `TransitionBackend::Remotion`
  exists only as an enum variant ("Future Remotion backend"); there is no
  Remotion renderer behind it.

The intended path today is "render the motion graphic externally → import it as
a PiP / B-roll / media overlay" — see `skills/overlay-animation/SKILL.md:38-45,
171-182` ("Generated animation is an asset workflow, not a custom render path …
The asset enters through `Insert PiP` or overlay `Insert BRoll`").

This document picks the pragmatic path and specifies it concretely.

## Decision

**Formalize a first-class "external motion-graphics asset" workflow** and defer
the native Remotion backend. Concretely: a motion-graphics comp (Remotion /
Lottie / After Effects) is rendered to a **transparent ProRes 4444 `.mov`** (or
VP9 `yuva420p` `.webm`) and imported as a **timed foreground overlay layer**
that is alpha-composited over the program timeline by the existing FFmpeg
overlay path. No new renderer is added to the render crate.

### Why this path (honest scope/effort)

| Option | What it buys | Effort | Verdict |
| --- | --- | --- | --- |
| **A. External alpha-overlay asset workflow** (this plan) | Branded intros/outros/lower-thirds/social cards today, using the full power of Remotion/AE/Lottie offline, with a stable alpha-compositing render path already in the codebase. | Low — mostly enablement + docs + the vertical slice below. | **Chosen.** |
| **B. Native Remotion backend** | In-app procedural composition, live previews, props-templating wired straight into render. | High — bundler/headless-Chromium render farm, asset pipeline, preflight, preview parity, security sandboxing, CI. | Deferred; tracked in `docs/motion-scene-remotion-backend.md`. |
| **C. Extend native MotionScene to cover video/keyframed-vector layers** | Avoids an external comp tool for medium-complexity graphics. | Medium-high per feature, and never reaches AE/Lottie expressivity. | Partial, orthogonal; continue incrementally but not the answer for "branded animated graphics." |

Option A reuses a render path that already alpha-composites correctly (see the
vertical slice), so the marginal cost of shipping branded graphics is the
*authoring* side (templates + brand kit), not the *render* side.

## EDL op surface

**Reuse the existing overlay ops — do not add a new op.** A transparent
motion-graphic clip is, from the timeline's perspective, just a foreground video
clip with an alpha channel:

- `EdlOp::InsertPiP` — `crates/core/src/edl/op.rs:161-176`. Use for corner cards
  (social handle, animated logo bug, subscribe bumper). Carries `anchor`,
  `asset`, `duration_s`, `source_start_s`, `corner`, `scale`, `margin_pct`.
- `EdlOp::InsertBRoll` — `crates/core/src/edl/op.rs:150-159`. Use for full-frame
  overlays (intro/outro stings, full-screen lower-third treatments) via
  `BRollPosition`.

Both are parsed (`crates/core/src/edl/parser.rs:60,229-230`) and applied
(`crates/core/src/edl/apply.rs:396-432`) today. They land the clip on an upper
video track, which the render planner reads as a `VideoOverlayPlan`
(`crates/render/src/timeline.rs:418-441, 1737`). A full-frame insert with an
alpha asset lowers to `VideoOverlayMode::FullFrame`
(`crates/render/src/timeline.rs:493-498`) and is composited with FFmpeg
`overlay=` (the alpha-compositing path), **not** `blend=` (the opaque path) —
see `crates/render/src/timeline.rs:5112-5124`.

A dedicated `Insert Motion Asset` op is **not** warranted (YAGNI): it would
duplicate the PiP/B-roll surface. If product later wants motion-graphic clips to
be *visually distinct* in the editor or to auto-carry template provenance, add a
thin `metadata.montage.motion_asset` tag on the inserted clip rather than a new
op.

## Transparency / alpha through render

This is the load-bearing requirement and the subject of the vertical slice.

- **Decode:** overlay inputs are decoded with no forced pixel format
  (`append_video_overlay_input_args`, `crates/render/src/timeline.rs:9674-9694`).
  A transparent ProRes 4444 `.mov` decodes to `yuva444p10le`; a VP9 `.webm`
  decodes to `yuva420p` — alpha intact. (Still images — PNG/SVG/WebP — go through
  the `-loop 1` branch, also alpha-capable.)
- **Composite:** the default overlay path (no opacity/rotation/mask/matte
  animation) now forces `format=rgba` immediately after `scale`, guaranteeing
  the overlay's secondary input carries an alpha channel into the `overlay=`
  compositor — `crates/render/src/timeline.rs` `append_video_overlays`
  (the `scale={scale_expr},format=rgba` step). The transform paths
  (rotation/mask/matte/opacity) already re-assert `format=rgba`
  (`timeline.rs:5082, 5100, ~5464, ~5528`), so the fix is idempotent for them.
- **FFmpeg semantics:** `overlay=` alpha-blends the top input over the bottom
  using the top's alpha. Without an explicit alpha format, FFmpeg's filter
  negotiation can let `scale` emit an opaque format (`yuv420p`) — the overlay
  then renders as a solid rectangle. Forcing `format=rgba` is the minimal fix.
- **Final encode:** the program output remains opaque (the overlay composites
  *onto* the program), so no special output pixel-format handling is needed.
  If a transparent *master* export is ever required, that is a separate
  output-format concern, out of scope here.

### Worked EDL example (full-frame branded intro)

```text
Insert BRoll anchor=clip_uuid=clip-intro asset=generated/overlays/intro-sting.mov duration_s=3.0 position=Over
```

Resulting (abridged) FFmpeg `-filter_complex` for the overlay leg:

```text
[1:v:0]setpts=PTS-STARTPTS+0/TB[media_overlay_pts0];
[media_overlay_pts0]scale=w=1920:h=1080,format=rgba[media_overlay_scaled0];
[<program>][media_overlay_scaled0]overlay=x=0:y=0:enable='between(t\,0\,3)'[media_overlay_v0]
```

The `,format=rgba` on the scaled leg is what preserves the intro sting's alpha;
`overlay=` then composites it over the program with transparency intact.

### Worked EDL example (corner social card / logo bug)

```text
Insert PiP anchor=clip_uuid=clip-a asset=generated/overlays/handle-card.webm duration_s=4.0 source_start_s=0 corner=top_right scale=0.28 margin_pct=0.05
```

Lowers to `VideoOverlayMode::PiP`, scaled `scale=w=1920*0.28:h=-2,format=rgba`,
positioned in the requested corner, alpha-composited over the program.

## Timing / anchor

- `anchor` (clip uuid / timeline time) + `duration_s` come straight from the op
  and set the overlay's `track_start_s` and segment duration
  (`VideoOverlayPlan { track_start_s, segment }`, `timeline.rs:1737-1748`). The
  composite is gated with `enable='between(t,start,end)'`
  (`timeline.rs:5121`).
- Match the asset's rendered duration to `duration_s` exactly (the
  overlay-animation skill already mandates this — `SKILL.md:172`). Do not let the
  timeline absorb drift.
- Corner/scale/margin for PiP come from the op fields; full-frame uses `x=0:y=0`.

## Props-templating for reusable cards

Reusable branded cards (intro / outro / lower-third / social) are **Remotion
compositions parameterized by props**, rendered offline. The contract:

1. Each template is a Remotion composition with a typed `defaultProps` schema
   (e.g. `{ name, title, handle, brandColor, logoSrc, accentFont }`).
2. A slots file (already specified by the overlay-animation skill —
   `SKILL.md:49-79`) names the slot, anchor, `start_s`, `duration_s`, and the
   props payload.
3. `npx remotion render <comp> out.mov --props=props.json --codec=prores
   --prores-profile=4444 --pixel-format=yuva444p10le` produces the transparent
   asset under `generated/overlays/<slug>/` (skill convention,
   `SKILL.md:38-41`). VP9 alpha WebM (`--codec=vp9 --pixel-format=yuva420p`) is
   the smaller alternative.
4. The asset is verified (`overlay_asset_verify.py`, `SKILL.md:97-107`) then
   inserted via the `Insert PiP` / `Insert BRoll` example above.

This deliberately mirrors the **native** templating that already exists for
text/rect cards — `EdlOp::InstantiateMotionTemplate`
(`crates/core/src/edl/op.rs`, applied at `crates/core/src/edl/apply.rs:1150-1209`
via `montage_render::professional::fill_motion_template` /
`lower_motion_template`). Native templates fill text + rectangle slots and lower
to drawtext/drawbox; *branded animated* templates fill the same kind of
slot-value map but render offline to an alpha asset. The two share the mental
model (template + slot values + timing window); they differ only in renderer.

## Brand kit → Remotion template

The brand kit (logo asset, brand fonts, social handles, palette) feeds a
template as props + bundled assets:

- **Logo / handles / palette** → JSON props (`logoSrc`, `handle`, `brandColor`).
- **Fonts** → bundled with the Remotion project (`@remotion/google-fonts` or
  local `loadFont`) so custom typography renders deterministically offline —
  this is exactly what the native drawtext path *cannot* guarantee for arbitrary
  branded fonts, and a core reason to use the external path for branded cards.
- A single `brand.json` in the project can be the source of truth that both the
  slots generator and the Remotion `defaultProps` read, so a brand change
  re-renders every card consistently.

See `skills/remotion-best-practices` for composition/render conventions.

## Existing pieces referenced

- `skills/overlay-animation/SKILL.md:38-45, 49-79, 97-107, 171-182` — the
  asset-workflow skill: slots → manifest → generate → verify → insert via
  PiP/BRoll. This plan formalizes its render-side guarantee (alpha) and its
  branded-template authoring contract.
- `BroadcastOverlayPlan` / broadcast overlay path —
  `crates/render/src/timeline.rs:184-187, 1917, 4472, 4729`. A *browser-rendered*
  overlay (HTML/CSS) is already composited as a foreground layer
  (`build_timeline_argv_full`, `timeline.rs:9592-9594`). It is the closest
  existing precedent for "render a graphic externally, composite it over the
  program," and validates the architecture; branded Remotion assets follow the
  same compositing shape but via the file-based PiP/BRoll overlay path.
- `EdlOp::InstantiateMotionTemplate` — `crates/core/src/edl/apply.rs:1150-1209`.
  Native (text/rect) templating; the branded path reuses its slot-value mental
  model with an offline renderer.
- `TransitionBackend::Remotion` — `crates/proto/src/transitions.rs:499-500`.
  Stub only; left for option B.

## Vertical slice (shipped in this change)

Enabled and verified that a transparent foreground asset is alpha-composited
(not rendered opaque) by the existing overlay path:

- **Fix:** `crates/render/src/timeline.rs` `append_video_overlays` — the default
  overlay leg now emits `scale={scale_expr},format=rgba` so alpha survives
  FFmpeg format negotiation into `overlay=`. Previously only the
  opacity/rotation/mask/matte sub-paths forced `format=rgba`; a plain
  transparent overlay could be negotiated to an opaque format and render as a
  solid box.
- **Test:** `timeline::tests::transparent_overlay_preserves_alpha_into_overlay_filter`
  asserts the filter graph contains `scale=w=1920:h=1080,format=rgba[media_overlay_scaled0]`
  and that the rgba leg feeds `overlay=` (the alpha compositor), not `blend=`.

## What remains for full native support (option B)

- A Remotion render backend behind `TransitionBackend::Remotion` /
  MotionScene video layers: headless-Chromium bundling + render farm, props
  schema → MotionScene mapping, asset resolution, output-format negotiation.
- Preview parity in the desktop app (today the external asset only previews
  after it is rendered and imported).
- Preflight reporting for Remotion render capability/limitations, mirroring the
  native MotionScene preflight.
- Lowering MotionScene video/media layers (`timeline.rs:1986-1993`) instead of
  emitting limitations.
- Security sandboxing for executing template code, and CI for the render path.

Until then, branded animated graphics ship through the external alpha-overlay
workflow specified above.
