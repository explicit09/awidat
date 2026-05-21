# Transitions

## Scope

The professional brief frames transitions as a sparingly-used craft: most pro
edits are hard cuts, J/L cuts, or match cuts, and visible transitions only
appear when they have an editorial job (hide a motion jump, mark a chapter
break, sell a beat, hold an emotional drift). The audit covers Awidat's
transition engine, transition catalog, J/L cut grammar, and speed-ramp
transitions — i.e., the engine and planner that decides *which* transition
to use and how to render it, not the general animation kernel (Cat 4),
compositing/mask compositor (Cat 7), or speed remapping (Cat 8).

- Transition planning (agent-facing decision packet, recommendation, validator)
- Transition catalog (semantic ids, families, FFmpeg/GPU backends, metadata)
- Rendering path (xfade splicing, GPU shader path, composition primitives)
- J-cut / L-cut split-edit grammar (EDL ops, planner, dedicated skill)
- Speed-ramp transitions (intentionally separate from Cat 8 speed remap)

## Current state in Awidat

### Transition planning (agent-facing)

- `crates/core/src/tools/transition_context.rs:42` — `TransitionContextTool`
  builds a read-only decision packet for one adjacent timeline boundary.
  Returns adjacent clip metadata, source/timeline ranges, transition handle
  availability (incoming/outgoing/max-centered), continuity verdict,
  transcript snippets before/after, suggested frame timestamps, per-side
  motion magnitudes and screen directions, motion-match classification
  (aligned/opposed/orthogonal/unknown), and a missing-signal list.
- `crates/core/src/tools/plan_transition.rs:46` — `PlanTransitionTool` takes
  that packet plus an optional `objective` (one of `hide_motion_jump`,
  `beat_hit`, `soft_time_passage`, `chapter_reset`, `visual_match`,
  `style_accent`) and `direction`, then recommends *either* a hard cut with
  `*** Set Cut Intent` metadata *or* one motivated transition with safe
  duration, intent, energy, direction, reason, an alternate, and an EDL
  fragment ready for `apply_edl`. Falls back to a hard cut when handles are
  insufficient, motion is opposed, or static-side metadata flags the
  transition as inappropriate (`incompatibility_reason` at `:244`).
- `crates/core/src/tools/validate_transition_choice.rs:43` — post-application
  validator that compares a transition's declared `motion_alignment` against
  measured `dominant_direction` on each side, returning
  `acceptable` / `wrong_direction` / `no_signal`. Phase-5 perf budget asserts
  the call returns in under 2 seconds.

Verdict: agent-facing planning is mature and unusually deliberate for the
category — it actively *resists* gratuitous transitions, mirroring the
professional brief.

### Transition catalog

- `crates/proto/src/transitions.rs:565` — `BUILTIN_TRANSITIONS` array, the
  single source of truth. Counted approximately 33 entries (unverified exact
  count) across these families: `cut`, `custom`, `dissolve`, `fade`, `flash`,
  `wipe`, `slide`, `motion_blur`, `occlusion`, `iris`, `invisible_cut`,
  `zoom`, `glitch`, `stylized`.
- Each entry carries `id`, `family`, `display_name`, optional `ffmpeg_xfade`
  name, default/min/max durations, `audio_policy` (Crossfade or Cut),
  `best_for` / `avoid_for` taste tags, `requires_motion_continuity`,
  `motion_alignment` (Left/Right/Up/Down/In/Out), `color_sensitivity`
  (Insensitive / AvoidDarkToBright / AvoidBrightToDark / AvoidColorShift).
- Phase-3 catalog expansion (`crates/proto/src/transitions.rs:917`) adds
  wipe_up, wipe_down, slide_up, barn_door_open/close (H/V), diag_tl,
  distance_zoom, dissolve_grain. Luma-mask family
  (`:1075`) adds clock_wipe, venetian_blinds_h, checkerboard_dissolve,
  spiral_wipe, burst_wipe, jaws_wipe — all GPU-only (`ffmpeg_xfade: None`).
  Hyperframes-port (`:1168`) adds light_leak, swirl_vortex,
  cinematic_pan_left/right — all GPU-only.
- `SemanticTransitionSpec` and `TransitionComposition`
  (`crates/proto/src/transitions.rs:11`, `:39`) let the agent author one-off
  recipes as *data*, using a stable primitive vocabulary: `Opacity`, `Push`,
  `Wipe`, `Zoom`, `Blur`, `Flash`, `Shake`, `ChromaticSplit`, `Pixelize`,
  `LumaMask`, `LightLeak`, `SwirlVortex`, `CinematicPan`, `Atomic`. Curves
  can be `Const` or multi-keyframe `Keyframes` with per-segment easing.
- `TransitionManifest` (`:502`) + `stable_builtin_transition_manifests`
  (`:1276`) export the registry in the shape a future external
  `awidat-transitions` package will consume; validator at `:1325` rejects
  entries with empty backends or out-of-bounds durations.

Verdict: catalog is broad in *grammar* (dissolves, fades, wipes, slides,
pushes, zoom, motion blur, iris, luma-mask family) but the brief's named
asks for **film burns** and a dedicated **glitch** family are thin —
`pixelize` is the only `family: "glitch"` member and it's the FFmpeg
`pixelize` xfade, not RGB-split/datamosh/scan-lines (unverified for absence
of any GPU stylized glitch beyond `chromatic_split` primitive).

### Rendering path

- `crates/render/src/timeline.rs:1137` lowers an OTIO `Transition.1` node to
  FFmpeg by calling `transitions::resolve_ffmpeg_xfade(&t.transition_type)`,
  then queues a `pending_transition` that will splice an
  `xfade=transition=<kind>:duration=<dur>:offset=<off>` filter between the
  two adjacent segments (graph build at `:3261` and `:7280`).
- `crates/render/src/timeline.rs:1623` rejects compositions that have no
  FFmpeg `xfade` equivalent at the FFmpeg backend; GPU-only luma-mask /
  hyperframes-port transitions route through the GPU composer instead.
- `crates/render-gpu/src/lib.rs:30` ships nine WGSL shaders
  (`fullscreen.wgsl` plus `cross_dissolve`, `shake`, `chromatic_split`,
  `blur`, `luma_mask`, `light_leak`, `swirl_vortex`, `cinematic_pan`).
  `TransitionShader` enum (`:79`) and `from_stable_id` (`:146`) bind a
  composition's resolved shader id to a concrete WGSL pipeline.
- `crates/render/src/raw_stream_render.rs:45` explicitly forbids *mixing*
  xfade and GPU transitions inside one render (`Phase-2`-style limitation,
  test `rejects_mixed_xfade_and_gpu_transitions` at `:353`).
- Handle-availability is enforced at render time
  (`crates/render/src/timeline.rs:82` — `TransitionHandleUnavailable`).

Verdict: dual-backend pipeline (FFmpeg xfade + wgpu shader) is wired and
type-checked; the "can't mix in one render" constraint is a real authoring
limitation but is gated, not silent.

### J/L cut tooling

- `crates/core/src/edl/op.rs:229` / `:239` define `SetAudioLead` (J-cut) and
  `SetAudioTrail` (L-cut) as first-class EDL ops with optional reason and
  confidence (`SplitEditSpec`).
- `crates/core/src/edl/parser.rs:663,689` parses these from the EDL syntax;
  `crates/core/src/tools/apply_edl.rs:327,331` tags timeline rollout entries
  with `split_edit:j_cut` / `split_edit:l_cut`.
- `crates/core/src/tools/assess_edit_quality.rs:308,314` recommends a J-cut
  or L-cut as the *preferred* lower-attention repair before reaching for a
  visible transition; learned guidance at `:1206` adjusts `lead_s` /
  `trail_s` based on prior accepted edits.
- `skills/split-edit-director/SKILL.md` is a dedicated skill that asserts
  split edits are "basic editing grammar, not polish" and routes the agent
  away from visible transitions when audio continuity is the real fix.
- `skills/transition-director/SKILL.md:31-35` explicitly defers to
  `assess_edit_quality` and uses split-edit verdicts to *suppress* visible
  transitions.

Verdict: J/L cuts are modeled as audio-picture grammar (the pro-brief
framing), separated cleanly from visible transitions, and wired into the
recommend-then-apply loop. This is unusually well-done for an agentic
editor.

### Speed-ramp transitions

- `TransitionPrimitiveOp::TimeRemap { speed: ParamCurve }` now represents a
  transition-local playback-speed curve. `awidat.ramp_in_beat` and
  `awidat.ramp_out_chapter` are registered `family: "speed_ramp"` recipes
  that pair the retime semantic with a visual fallback primitive.
- `plan_transition` now maps `objective: "beat_hit"` to
  `awidat.ramp_in_beat` instead of collapsing directly to `awidat.flash_white`.
  Export backends still render the paired visual primitive (`fadewhite` /
  `fadeblack`) until transition-local retime execution is wired into the
  render path.

Verdict: semantic/planner gap closed; render execution of transition-local
retime remains the next implementation step.

## Reference repo signals

- **kdenlive** (`data/transitions/`): 15 MLT transition XMLs (luma, wipe,
  dissolve, slide, composite, qtblend, mix, affine, matte, region, vqm) plus
  46 frei0r XMLs (cairoblend, addition, screen, darken, lighten, divide,
  difference, grain_merge, wipe-circle, etc.). Transitions are declarative
  XML over the MLT framework; each XML file lists parameters with min/max,
  factor, and human names (e.g., `data/transitions/luma.xml` exposes
  `softness`, `invert`, `reverse`, `alpha_over`, `fix_background_alpha`).
  J/L cuts are not modeled as a transition; split edits are achieved by
  trimming audio and video on separate tracks
  (`src/timeline2/model/timelinefunctions.cpp`).
- **shotcut**: ships a `lumamixtransition` widget
  (`src/widgets/lumamixtransition.{h,cpp,ui}`) that exposes the MLT luma
  mix transition with selectable wipe textures and a softness slider. Like
  kdenlive, no dedicated J/L cut UI primitive — split edits are manual
  track-level trims.
- **olive**: transition is a `TransitionBlock` node
  (`app/node/block/transition/transition.h`) with `in_offset` / `out_offset`
  / `offset_center` / `is_dual_transition` and `ShaderJobEvent` /
  `SampleJobEvent` hooks; concrete subclasses include
  `crossdissolvetransition.cpp` and `diptocolortransition.cpp`, each
  pairing a tiny C++ class with one GLSL fragment shader
  (`app/shaders/crossdissolve.frag`). One node tree handles both video and
  audio sample-blending, which is conceptually cleaner than ffmpeg's
  separated `xfade` + `acrossfade`.
- **openshot-qt**: ships **412+ transition wipe assets** — 7 in
  `src/transitions/common/` (fade plus four wipes plus iris in/out) and
  ~405 in `src/transitions/extra/` (ripple, twirl, spiral, distortion,
  fogg, spots, postime, etc., each preview + luma mask). All are
  fundamentally luma-mask wipes parameterized by softness and reverse —
  same model as kdenlive's `luma.xml`.
- **revideo**: transitions are scene-to-scene render callbacks
  (`packages/core/src/transitions/useTransition.ts`) — `fadeTransition`,
  `slideTransition`, `zoomInTransition`, `zoomOutTransition`. The model is
  procedural / code-driven (manipulate `CanvasRenderingContext2D` for the
  outgoing/incoming scene), not a registry of named effects. Closest to
  Awidat's `TransitionComposition` primitive recipe, but without a
  semantic/intent layer.
- **opencut**: no transition primitive in the timeline data model
  (`apps/web/src/timeline/`) — only CSS `transition-colors` references for
  UI animation. Confirms that a meaningful subset of "modern web NLE"
  projects skip the transition engine entirely.

## Gap analysis

| Sub-area | Awidat today | Reference signal | Gap | Severity |
|---|---|---|---|---|
| Transition catalog breadth | ~33 named transitions across 14 families, FFmpeg + WGSL backends | openshot-qt ships 400+ wipe luma masks; kdenlive ships 60+ MLT entries including 46 frei0r blends | Catalog is editorially curated (which is correct for an agent), but the **image-based luma mask primitive only references procedural masks** (clock/blinds/checker/spiral/burst/jaws) — no path to load openshot-style PNG/PGM luma textures referenced in `TransitionPrimitive::LumaMask` doc comment ("Future image-based masks (Kdenlive ports) extend the same primitive with a `texture` asset reference") | Medium |
| Film burns | Not present | Common in commercial NLEs and OFX plugins; openshot/kdenlive use frei0r blends + texture composites | No `awidat.film_burn` id, no `FilmBurn` primitive, no warm-grain GPU shader path that combines `light_leak` + grain + soft-cross | Medium |
| Glitches | Single entry `awidat.pixelize` (FFmpeg `pixelize` xfade) plus `ChromaticSplit` primitive | Common pro grammar (RGB shift, scan-lines, datamosh, signal loss) | No `awidat.glitch_*` ids, no scan-line / RGB-shift / datamosh shader, no `family: "glitch"` for the chromatic split primitive on its own as a transition id | Medium |
| Speed-ramp transitions | `TransitionPrimitiveOp::TimeRemap { speed: ParamCurve }` plus `awidat.ramp_in_beat` / `awidat.ramp_out_chapter`; `beat_hit` planner routing now selects `awidat.ramp_in_beat` | Pro grammar: slow-out / fast-in across the cut to land a beat; cinematic_pan models blur while the new primitive records retime intent | Semantic and planner coverage are present; render backends currently use the paired visual primitive as fallback until transition-local retime execution is implemented | Medium |
| J/L cut tooling | First-class EDL ops, planner, dedicated skill, learned timing guidance | kdenlive/shotcut/olive treat J/L cuts as manual multi-track trim with no preference/recommendation layer | Awidat is **ahead** here — no gap | None |
| Backend portability | FFmpeg xfade + WGSL shader; mixing forbidden inside one render | Olive runs both video shader + audio sample-blend on one `TransitionBlock` node | Cannot mix xfade and GPU transitions in one timeline export (`raw_stream_render.rs:45`); GPU-only luma masks force a whole-project GPU render | Medium |
| Motion-aware validation | `validate_transition_choice` reads measured per-side `dominant_direction` and emits `wrong_direction` / `acceptable` / `no_signal` | No reference repo has this loop | Awidat is **ahead** here — no gap | None |
| Composition primitive set | 14 primitives + `Atomic` escape hatch; multi-keyframe `ParamCurve` with per-segment easing | Olive shaders are per-transition `.frag`; revideo is freeform code; kdenlive is XML parameter sliders | Strong — but no `time_remap`, no `displacement_map` (for liquid/morphs), no `mesh_warp` primitive | Medium |
| Audio-side policy | Per-transition `TransitionAudioPolicy::Crossfade` or `Cut` flag honored by the renderer | Olive pairs the same node with `SampleJobEvent`; kdenlive uses MLT crossfade on a separate audio track | Per-transition policy exists; `unverified` whether `Cut` policy actually emits a hard audio cut or just no fade curve at the renderer | Low |

## Suggested next steps

1. **Execute transition-local retime in render.** The data model and planner
   now express speed-ramp transitions via `TransitionPrimitiveOp::TimeRemap`
   and `awidat.ramp_in_beat` / `awidat.ramp_out_chapter`; the next step is
   lowering that speed curve into the render path instead of using only the
   paired visual fallback.
2. **Wire the image-based luma mask path the registry already documents.**
   `TransitionPrimitive::LumaMask` already declares "Future image-based
   masks (Kdenlive ports) extend the same primitive with a `texture` asset
   reference." Adding a `texture: Option<AssetRef>` field opens the door to
   importing openshot's 400+ wipe library without bloating the named-id
   catalog.
3. **Add a glitch family with real grammar.** Promote `ChromaticSplit` and
   add `ScanLines`, `Datamosh`, `SignalLoss` primitives plus 2–3 named ids
   (`awidat.rgb_shift`, `awidat.scan_line_flicker`,
   `awidat.signal_loss_short`). Update `plan_transition`'s `style_accent`
   objective so glitch is a real candidate rather than collapsing to
   `motion_blur`.
4. **Add film-burn ids.** Compose `LightLeak` + grain + cross-dissolve into
   a named `awidat.film_burn` (and a short version) so the planner can
   reach for the warm-organic family beyond just `light_leak`.
5. **Allow mixed-backend renders.** Lift the
   `rejects_mixed_xfade_and_gpu_transitions` constraint in
   `raw_stream_render.rs` by routing each boundary through its declared
   backend and re-concatenating; this lets a project use the broad FFmpeg
   xfade set on most cuts and reserve GPU shaders for the ones that need
   them.

## Open questions

- Is `TransitionAudioPolicy::Cut` rendered as a true audio hard cut, or
  just absence of `acrossfade`? `crates/render/src/timeline.rs:3072`
  references "`xfade` + `acrossfade` graph" but the exact branch on
  `Cut` policy is unverified.
- The `audio_policy` field is present on every transition but the brief
  treats split-edit audio offsets as a separate axis. Should a visible
  transition with `Crossfade` policy also be allowed to carry a J/L offset,
  or does that combination always fall back to `split-edit-director`?
- Phase-1 docs say the catalog is "deliberately limited to transitions
  FFmpeg can export through the current render path"
  (`crates/proto/src/transitions.rs:563`), yet the luma-mask family ships
  `ffmpeg_xfade: None`. Is the comment stale, or are GPU-only entries
  considered "phase-1.5"?
- The validator only checks measured motion; should it also flag
  `color_sensitivity` violations (a `flash_white` going dark-to-bright,
  for example)? Today that check lives in `incompatibility_reason` at
  plan-time only.
