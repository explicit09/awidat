# Motion Graphics & Animation

## Scope

Animation engine, keyframe/curve model, motion paths, transform/opacity/blur
parameters, tracking-driven motion, and graphic primitives (titles,
lower-thirds, callouts, logo reveals) that an agent can plan, persist, and
deterministically render. Covers the planner -> proto -> render -> preview
chain that makes static shots feel dynamic. Excludes shot transitions
(Category 5), pure VFX/compositing (Category 7), and typography styling
(Category 9).

- Keyframe data model and validation
- Easing curves, bezier handles, springs, tangent modes, extrapolation
- Motion paths (x/y over time)
- Transform parameters: position, scale, rotation, opacity, blur
- Tracking attachment and motion-driven attachment
- Text/lower-third/callout/logo-reveal templates with animation lowering
- GPU acceleration for motion effects
- Agent-facing planning tools

## Current state in Awidat

### Keyframe / curve model — strong

`crates/proto/src/professional.rs` defines `Keyframe`, `BezierHandles`,
`TangentMode` (`Auto`, `Aligned`, `Flat`, `Broken`), `SpringParameters`
(mass/stiffness/damping), `ExtrapolationMode` (`Hold`/`Linear`), and
`KeyframeInterpolation` (`Hold`/`Step`/`Linear`/`Bezier`/`Spring`).
`ParameterAnimation` (line 450) attaches keyframes to clip or track
parameters via `AnimationTarget` and includes optional `motion_path`.
Validation (lines 580-712) enforces finite Bezier handles in `[0,1]`,
positive spring mass/stiffness, aligned-tangent collinearity, and
parameter-specific value ranges (opacity in `[0,1]`, font_size/scale
positive).

Evaluator in `crates/render/src/animation.rs` evaluates keyframes
(`evaluate_keyframes`, `evaluate_animation`), motion paths
(`evaluate_motion_path`), and exposes a deterministic FFmpeg lowering
(`keyframes_to_ffmpeg_expr_with_extrapolation`,
`motion_path_to_ffmpeg_expr`) used at lines 1975, 3795, 4206, 4245,
5553, 5646 of `crates/render/src/timeline.rs`. Easing catalog (line
331) covers 28 named curves (sine/cubic/quart/quint/expo/circ/back/
elastic/bounce, all in/out/inOut). Spring evaluation uses physical
underdamped/critical formulas and lowers to a 64-sample piecewise
linear FFmpeg expression with parity tested against the Rust evaluator
through `fixtures/motion/animation-parity.json`.

### Runtime parameter surface — broader, still registry-backed

`RUNTIME_CLIP_PARAMETERS` (`crates/proto/src/professional.rs:562-581`)
declares the direct executable set: `title.opacity/x/y/position/font_size`,
`overlay.opacity/x/y/position/scale/rotation_deg/blur`, and the canonical
`awidat.blur`, `awidat.shake`, and `awidat.warp` parameter paths. The runtime
also accepts the generic `effects.<effect_id>.params.<param>` namespace for
those in-tree effects and canonicalizes aliases before validation/render
selection. Track surface is still `volume` and `volume_db`. The remaining gap
is breadth: new effects still need to declare animatable params through this
registry as they land.

### Motion paths — present, linear only

`MotionPath`/`MotionPathPoint` (proto line 885) store 2-D points with
time. Evaluator and FFmpeg lowering do straight linear segments
between points (`crates/render/src/animation.rs:216-277`); no bezier
or spline control over the path itself, no orient-along-path, no
arc-length parameterization. Unverified: motion path likely only
applies to overlay x/y (filter wiring at `timeline.rs:4206`).

### Emphasis / planning tools — present

`crates/core/src/tools/plan_emphasis.rs` (853 lines) is the
clip-level emphasis planner. It produces one of `scale_punch`,
`slow_zoom`, `slide_in`, or `rotate_nudge` and emits a
`ParameterAnimation` validated through the proto validator. Easing
mapping (`easing_for`, `spring_for` at lines 441-469) picks from a
small style palette (`snappy`/`loose`/`heavy`/neutral). Beat-driven
mode multiplies pulses with diminishing intensity. Sibling planner
`plan_look_regions.rs` (1279 lines) plans look-region/zoom paths
across longer durations.

### Motion-graphics templates — fixed catalog of 9

`crates/render/src/professional.rs:1561-1671` ships `lower-third`,
`callout`, `punch-in-zoom`, `focus-highlight`, `title-reveal`,
`pip-emphasis`, `product-insert-emphasis`, `shake-emphasis`,
`logo-reveal`. Each has typed slots (target_clip, text, color,
scale, intensity, image/video_asset, safe_area_profile) and lowers
to titles + media overlays + `ParameterAnimation` records via
`lower_motion_template` (line 1897) and `template_parameter_animations`
(line 1999). Lower-third generates opacity fade + y-slide; logo-reveal
fades + scale-up; shake-emphasis emits x/y/rotation keyframes
procedurally. Per-aspect safe-area validation runs before render
(`platform_safe_areas` 16:9 / 9:16 / 1:1 at line 1819).

### Title animation primitives — limited

Title plans support `FadeInOut`, `SlideIn`, `FadeIn`, `None`, plus
`TextReveal`/`WriteOn` progressive variants (professional.rs around
line 1539). No per-character/per-word stagger, no path-attached text,
no kinetic typography primitives beyond progressive reveal.

### Tracking-driven motion — strong

`TrackingPackage`, `TrackKind`, `TrackSample`, `TrackSidecar`
(proto, used at `crates/render/src/professional.rs:21`) drive
`reframe_paths` and mask/matte attachment. `generate_tracking_package`
(line 322) and `summarize_tracking_package` (line 453) feed the
runtime; timeline.rs (lines 1239, 1267, 1312, 1415) lowers tracking
into render-time mask/matte/reframe filters. Motion-blur shutter
plan (`OverlayMotionBlurPlan`, line 415; reader line 1822) clamps
shutter to <= 0.2s. Unverified: tracking-attached parameter
animation (e.g. "callout glued to tracker rect") may not yet be a
first-class target — `AnimationTarget` enumerates ClipParameter,
TrackParameter, CompositionNodeParameter, Unset only.

### GPU acceleration — transitions-only

`crates/render-gpu/src/lib.rs` (1277 lines) is exclusively a
transition shader runner: `CrossDissolve`, `Shake`, `ChromaticSplit`,
`Blur`, `LumaMask`, `LightLeak`, `SwirlVortex`, `CinematicPan`.
Pipeline is fixed two-input (`t_from`/`t_to`) with `progress` and 8
generic extra params. Animation parameters (overlay scale/rot/x/y/
opacity/blur, motion paths) are lowered to FFmpeg `scale`, `rotate`,
`overlay`, `geq`, etc. - not to a GPU compositor. Effects crate is
a single ~1080-LOC `lib.rs` stub (`crates/effects/src/lib.rs`).

### Overlay-animation skill — asset workflow

`skills/overlay-animation/SKILL.md` plans externally generated motion
graphic assets (canvas / external renderer) and inserts them via the
existing video overlay pipeline. It does not extend the in-engine
animation system; bespoke motion-graphic looks are an asset, not a
node graph.

## Reference repo signals

### opencut — closest peer to Awidat's keyframe model

`apps/web/src/animation/types.ts` defines `ScalarAnimationKey`
with `leftHandle`/`rightHandle` (`{dt,dv}`), `segmentToNext`
(`step`/`linear`/`bezier`), `tangentMode` (`auto`/`aligned`/`broken`/
`flat`), per-channel `extrapolation: {before, after}`
(`hold`/`linear`), plus separate `DiscreteAnimationKey` for
boolean/string channels. `ANIMATION_PROPERTY_PATHS` enumerates
~15 paths (`transform.positionX/Y`, `transform.scaleX/Y`,
`transform.rotate`, `opacity`, `volume`, `color`, plus
`background.*`). Effect params live under `effects.<id>.params.<p>`
and graphic params under `params.<p>`, so the channel system is a
single unified namespace, not parameter-specific. Files:
`apps/web/src/animation/{types.ts,graph-channels.ts,effect-param-channel.ts}`,
`apps/web/src/clipboard/handlers/keyframes.ts`,
`apps/web/src/timeline/animation-snap-points.ts`.

### revideo — code-first scene as generator-driven tween

`packages/core/src/tweening/` ships `tween.ts`, `spring.ts`, and
`timingFunctions.ts` (28+ named easing functions matching Awidat's
catalog 1:1, derived from easings.net). Spring is generator-based
(`yield`-driven step integration with `settleTolerance` and
`mass/stiffness/damping/initialVelocity`). Animations are scenes:
JSX-like `Layout`, `Txt`, `Img`, `Video` components
(`packages/2d/src/lib/components/`) animated via signals
(`packages/core/src/signals/`). Lower-thirds and titles are just
composed components, not a fixed template catalog. No FFmpeg
expression lowering — playback happens in the browser/headless
renderer.

### twick — declarative "animation" addon with named presets

`packages/timeline/src/core/addOns/animation.ts` is a tiny
`ElementAnimation` class with `name`, `interval`, `duration`,
`intensity`, `animate` (`enter`/`exit`/`both`), `mode` (`in`/`out`),
`direction` (`up`/`down`/`left`/`right`/`center`). No keyframes,
no curves — the renderer interprets the named animation. Effects/
animation logic lives in `packages/effects/` and
`packages/visualizer/src/controllers/animation.controller.ts`. Much
weaker than Awidat for low-level control.

### hyperframes — HTML/CSS/GSAP for motion graphics

`packages/core/src/parsers/gsapParser.ts` parses real GSAP
`set/to/from/fromTo` calls with `position`, `duration`, `ease`. The
`SUPPORTED_EASES` list (~24 entries: `power1-4.{in,out,inOut}`,
`back`, `elastic`, `bounce`, `expo`) is a direct subset of Awidat's
easing catalog. `SUPPORTED_PROPS` is GSAP-style: `opacity`,
`visibility`, `x/y`, `scale/scaleX/scaleY`, `rotation`, `autoAlpha`,
`width/height`. Animation is a parsed AST that drives CSS-targeted
DOM in a headless renderer (`packages/engine/`), and rendering is
strictly HTML/CSS/GSAP under the hood. Template catalog is in
`packages/core/src/templates/`.

### openshot-qt — preset-as-keyframe-table

`src/animation_presets.py` is a single Python dict mapping preset
name -> property -> `(frame, value, easing_name)` triples (~30
Animate.css-inspired presets: `bounce`, `flash`, `pulse`,
`rubberBand`, `shakeX/Y`, `swing`, `tada`, `wobble`, `bounceIn`,
`fadeIn*`, `slideIn*`, `zoomIn*`, etc.). `KEYFRAME_EASING` is the
CSS `cubic-bezier(x1,y1,x2,y2)` form; the comment explicitly maps
those to libopenshot `Point.handle_right`/`handle_left`. Tight
agent-friendly format Awidat could mine for additional emphasis
presets.

### olive — node-graph keyframes per parameter

`app/node/keyframe.h` defines `NodeKeyframe` with `Type`
(`kLinear`/`kHold`/`kBezier`), `BezierType` (`kInHandle`/`kOutHandle`)
with `QPointF` control points and signals for control-point changes
(`BezierControlInChanged`, `BezierControlOutChanged`). Per-channel
keyframes live on `NodeInput` parameters across the node graph
(`app/node/input/`). Olive does not appear to ship a Spring or
Elastic interpolation type at the keyframe level — only Linear /
Hold / Bezier — which makes Awidat's curve catalog richer than
Olive's by interpolation kind even though Olive's node graph is
broader in scope.

### kdenlive — MLT-backed keyframe enums

`src/assets/keyframes/model/keyframemodel.hpp` defines
`KeyframeType::KeyframeEnum` converted from `mlt_keyframe_type`,
with `addKeyframe(pos, type, value)` over `GenTime` positions and
`updateKeyframeType` for type swaps. Storage is
`std::map<GenTime, pair<KeyframeType, QVariant>>`. Titler lives in a
separate subsystem (`src/titler/{titledocument,titlewidget}.{cpp,h}`,
`graphicsscenerectmove.cpp`) and is QGraphicsScene-driven for
authoring rather than a template-fill flow.

## Gap analysis

| Area | Awidat today | Reference signal | Gap |
| --- | --- | --- | --- |
| Runtime parameter surface | Direct `title.*` / `overlay.*` paths plus canonical `awidat.blur`, `awidat.shake`, `awidat.warp` params and `effects.<effect_id>.params.<param>` aliases for those effects | opencut animates `transform.*`, `opacity`, `color`, `background.*` and arbitrary `effects.<id>.params.<p>` | Namespace is now present for executable in-tree effects; remaining work is breadth — every new effect module must declare its animatable params through the registry |
| Motion paths | Linear segments only (`animation.rs:216`) | Olive bezier handles per keyframe; opencut `CurveHandle` per scalar key | Add bezier/spline interpolation on `MotionPathPoint`, optional orient-along-path, and arc-length param so a callout can travel smoothly |
| Tracking-attached animation | Tracking drives masks/mattes/reframe_paths; not a direct `AnimationTarget` variant | Common pro pattern (After Effects null-object) | Add `AnimationTarget::TrackerAttachment { tracker_id, offset }` so an overlay/title can be glued to a tracked point with one keyframe set |
| GPU acceleration of motion | GPU is transitions-only (`render-gpu/src/lib.rs`); animations lower to FFmpeg `scale/rotate/overlay/geq` expressions | revideo, hyperframes both render motion in a GPU/DOM compositor for preview parity | Generalize the GPU pipeline to a per-frame transform/composite pass so preview is shader-driven and matches export bit-for-bit |
| Effects crate | `crates/effects/src/lib.rs` is the only file (~1080 LOC) | opencut has a full effect-channel system with per-effect param channels | Split `effects/` into per-effect modules with declared animatable params; wire them into `RUNTIME_CLIP_PARAMETERS` |
| Title/text animation | Plan-level enum: `FadeInOut`/`SlideIn`/`FadeIn`/`None` + progressive `TextReveal`/`WriteOn` | hyperframes/GSAP supports per-character/word stagger via DOM | Add per-character/word stagger primitive (`title.char_stagger_s`, `title.word_stagger_s`) and bind it to the existing easing catalog |
| Template catalog breadth | 9 templates, no infographics/stat-cards/tickers/lower-third style variants | hyperframes + opencut + openshot preset libraries are far broader | Either add 1-2 dozen template variants or push generation to the overlay-animation skill consistently (the current split is unclear) |
| Curve editor UI | No evidence of a curve editor in apps/desktop | Olive `keyframeview/`, Kdenlive `keyframemodel`, opencut animation graph | Build a graph editor surface so humans can sculpt curves the agent emitted |
| Velocity / time-warp | Clip-local `TimeRemapPlan` exists in render and Cat 5 now has `TransitionPrimitiveOp::TimeRemap`; `plan_emphasis` still gates its `time_ramp` alternate | Olive/Kdenlive have time-remap nodes | Wire planner emission and UI/editing surfaces to the existing retime primitives |
| Spring on FFmpeg path | Lowered as 64 piecewise-linear samples per segment | revideo runs spring as live step integration | Acceptable for export; if preview is direct FFmpeg, jitter on long durations may be visible — verify experimentally |
| Bezier/Spring/Elastic in TangentMode | TangentMode applies to bezier handles only; spring keyframes ignore TangentMode | Olive bezier-only model | Document that TangentMode is bezier-scoped (currently implicit) |
| Motion-graphic asset bridge | `skills/overlay-animation/SKILL.md` is an asset workflow that bypasses the engine | hyperframes treats motion graphics as first-class engine nodes | Long-term: collapse the skill's generated overlays into composable engine nodes so animations remain editable after generation |

## Suggested next steps

1. **Keep expanding the effect-parameter registry**: `effects.<id>.params.<p>`
   namespacing now exists for executable in-tree blur, shake, and warp
   params. New effects should declare animatable params through the same
   registry as they land.
2. **Bezier motion paths**: add optional per-`MotionPathPoint`
   `in_handle`/`out_handle` and an "orient along path" flag; reuse the
   existing bezier evaluator/sampler from scalar keyframes.
3. **TrackerAttachment animation target**: new `AnimationTarget` variant
   that resolves to per-frame x/y from a `TrackingPackage` track id,
   then routes through the existing overlay x/y lowering.
4. **Planner-visible time remap**: connect `plan_emphasis` to the existing
   clip-local retime support and Cat 5's `TimeRemap` transition primitive
   instead of keeping `time_ramp` as a gated alternate.
5. **Per-character/word title stagger**: extend `TitleAnimation` enum
   with a `Stagger { unit, delay_s, easing }` variant and lower in
   `progressive_titles`.
6. **Generalize render-gpu**: today it's a transition compositor; add
   a transform/blur/opacity pass keyed off `RenderParameterAnimation`
   so preview no longer depends on FFmpeg expression strings for the
   most common motion.

## Open questions

- Are tracking-driven `reframe_paths` already animatable through
  `ParameterAnimation`, or are they a parallel system? Unverified.
- Does `motion_path` apply only to `overlay.x`/`overlay.y`, or also to
  `title.x/y`? `timeline.rs:4206,5553` need a closer read to confirm.
- Is there an existing roadmap entry for time-remap? `plan_emphasis.rs`
  comments imply it is gated on a runtime that does not yet exist.
- Is overlay-animation skill output expected to round-trip back into
  `ParameterAnimation` records, or is it an opaque pre-baked asset?
  The SKILL.md treats it as opaque; that may be intentional but
  fragments the animation surface.
- How does the desktop UI surface keyframes today? Implied absence of a
  curve editor is uncertain — `apps/desktop/` was not exhaustively
  scanned (unverified).
- Are spring-lowering's 64 samples sufficient for an 8-second animation
  rendered at 60fps without visible stepping? Empirical check needed.
