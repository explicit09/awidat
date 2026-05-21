# Motion And Transition Gap Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the confirmed Category 04 motion-graphics runtime-parameter gap and Category 05 speed-ramp transition gap without changing unrelated editor behavior.

**Architecture:** Keep the existing keyframe and transition composition models. Add a small runtime effect-parameter registry that recognizes `effects.<effect_id>.params.<param>` aliases for the existing executable blur, shake, and warp effects, then canonicalize those aliases at validation and render-selection boundaries. Add a data-only `TimeRemap` transition primitive and named ramp transition recipes so `plan_transition` can select a retime-aware beat-hit transition while existing renderers can preserve a visual fallback.

**Tech Stack:** Rust 2024 workspace, `serde`, Awidat proto transition registry, core planner tools, render timeline animation lowering, existing Markdown/HTML gap-analysis docs.

---

## File Structure

- Modify `crates/proto/src/professional.rs`: add effect-parameter registry helpers, canonicalization, validation tests, and update runtime parameter docs.
- Modify `crates/render/src/timeline.rs`: canonicalize clip animation parameters before selecting blur/shake/warp render animations.
- Modify `crates/render/src/professional.rs`: canonicalize template/runtime animation validation and `RenderParameterAnimation` construction.
- Modify `crates/core/src/edl/apply.rs`: canonicalize runtime parameter checks and effect auto-attachment logic for EDL-applied animations.
- Modify `crates/proto/src/transitions.rs`: add `TransitionPrimitiveOp::TimeRemap`, validation, backend priority handling, named `awidat.ramp_in_beat` and `awidat.ramp_out_chapter` entries, manifests, and tests.
- Modify `crates/core/src/tools/plan_transition.rs`: map `objective: "beat_hit"` to `awidat.ramp_in_beat` and keep `awidat.flash_white` as an alternate/fallback.
- Modify `crates/render/src/raw_stream_render.rs`: make `extract_param_curves` ignore `TimeRemap` for GPU shader slots so visual fallback primitives still drive shaders.
- Modify `.reference-research/pro-editing-gap-analysis/04-motion-graphics.md` and `.html`: update the stale “12 clip parameters” wording and mark the `effects.*.params.*` namespace covered for existing in-tree effects.
- Modify `.reference-research/pro-editing-gap-analysis/05-transitions.md` and `.html`: update speed-ramp transition status once the named retime primitive is present.

## Task 1: Runtime Effect Parameter Namespace

**Files:**
- Modify: `crates/proto/src/professional.rs`

- [ ] **Step 1: Write failing proto tests**

Add these tests inside the existing `#[cfg(test)]` module in `crates/proto/src/professional.rs`:

```rust
#[test]
fn runtime_effect_parameter_aliases_are_executable() {
    assert!(is_runtime_clip_parameter("effects.awidat.blur.params.radius_px"));
    assert!(is_runtime_clip_parameter("effects.awidat.shake.params.intensity_px"));
    assert!(is_runtime_clip_parameter("effects.awidat.shake.params.frequency_hz"));
    assert!(is_runtime_clip_parameter("effects.awidat.warp.params.k1"));
    assert!(is_runtime_clip_parameter("effects.awidat.warp.params.center_x"));

    assert_eq!(
        canonical_runtime_clip_parameter("effects.awidat.blur.params.radius_px"),
        Some("awidat.blur.radius_px")
    );
    assert_eq!(
        canonical_runtime_clip_parameter("effects.awidat.warp.params.center_y"),
        Some("awidat.warp.center_y")
    );
    assert_eq!(
        canonical_runtime_clip_parameter("effects.unknown.params.amount"),
        None
    );
}

#[test]
fn runtime_effect_parameter_aliases_use_existing_value_validation() {
    let animation = ParameterAnimation {
        id: "bad-blur".to_string(),
        target: AnimationTarget::ClipParameter {
            clip_id: "clip-a".to_string(),
            parameter: "effects.awidat.blur.params.radius_px".to_string(),
        },
        keyframes: vec![Keyframe {
            time_s: 0.0,
            value: -1.0,
            interpolation: KeyframeInterpolation::Linear,
            easing: Easing::Linear,
        }],
        motion_path: None,
        rationale: None,
    };

    let diagnostics = animation.validate();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("must be non-negative")),
        "expected blur alias to share canonical blur validation, got {diagnostics:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-proto runtime_effect_parameter_aliases
```

Expected: FAIL because `canonical_runtime_clip_parameter` and alias support do not exist yet. If the machine still reports `No space left on device`, stop and report disk availability as the blocker.

- [ ] **Step 3: Add registry and canonicalization helpers**

Add this near `RUNTIME_CLIP_PARAMETERS`:

```rust
/// Runtime effect parameter exposed through the generic
/// `effects.<effect_id>.params.<param>` namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeEffectParameter {
    /// Stable effect id.
    pub effect_id: &'static str,
    /// Parameter name inside the effect.
    pub param: &'static str,
    /// Existing canonical runtime parameter path.
    pub canonical: &'static str,
}

/// Effect parameters executable by the current preview/render runtime.
pub const RUNTIME_EFFECT_PARAMETERS: &[RuntimeEffectParameter] = &[
    RuntimeEffectParameter {
        effect_id: "awidat.blur",
        param: "radius_px",
        canonical: "awidat.blur.radius_px",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.shake",
        param: "intensity_px",
        canonical: "awidat.shake.intensity_px",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.shake",
        param: "frequency_hz",
        canonical: "awidat.shake.frequency_hz",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "k1",
        canonical: "awidat.warp.k1",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "k2",
        canonical: "awidat.warp.k2",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "center_x",
        canonical: "awidat.warp.center_x",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "center_y",
        canonical: "awidat.warp.center_y",
    },
];

/// Return the canonical runtime clip parameter for a direct path or
/// generic effect namespace path.
pub fn canonical_runtime_clip_parameter(parameter: &str) -> Option<&'static str> {
    if let Some(runtime) = RUNTIME_CLIP_PARAMETERS
        .iter()
        .copied()
        .find(|runtime| *runtime == parameter)
    {
        return Some(runtime);
    }

    let (effect_id, param) = parse_effect_parameter_alias(parameter)?;
    RUNTIME_EFFECT_PARAMETERS
        .iter()
        .find(|runtime| runtime.effect_id == effect_id && runtime.param == param)
        .map(|runtime| runtime.canonical)
}

fn parse_effect_parameter_alias(parameter: &str) -> Option<(&str, &str)> {
    let rest = parameter.strip_prefix("effects.")?;
    let (effect_id, param) = rest.split_once(".params.")?;
    if effect_id.is_empty() || param.is_empty() {
        return None;
    }
    Some((effect_id, param))
}
```

Change `is_runtime_clip_parameter` to:

```rust
pub fn is_runtime_clip_parameter(parameter: &str) -> bool {
    canonical_runtime_clip_parameter(parameter).is_some()
}
```

- [ ] **Step 4: Canonicalize value validation**

In `ParameterAnimation::validate`, replace the direct validation call:

```rust
validate_parameter_animation_value(&mut diagnostics, &self.id, parameter, keyframe);
```

with:

```rust
let value_parameter = canonical_runtime_clip_parameter(parameter).unwrap_or(parameter);
validate_parameter_animation_value(&mut diagnostics, &self.id, value_parameter, keyframe);
```

- [ ] **Step 5: Run proto tests**

Run:

```bash
cargo test -p awidat-proto runtime_effect_parameter_aliases
```

Expected: PASS.

## Task 2: Render And EDL Canonicalization

**Files:**
- Modify: `crates/render/src/timeline.rs`
- Modify: `crates/render/src/professional.rs`
- Modify: `crates/core/src/edl/apply.rs`

- [ ] **Step 1: Write failing render selection test**

Add a test near the existing animation-selection tests in `crates/render/src/timeline.rs`:

```rust
#[test]
fn effect_parameter_alias_selects_blur_radius_animation() {
    let animations = vec![awidat_proto::professional::ParameterAnimation {
        id: "blur-alias".to_string(),
        target: awidat_proto::professional::AnimationTarget::ClipParameter {
            clip_id: "clip-a".to_string(),
            parameter: "effects.awidat.blur.params.radius_px".to_string(),
        },
        keyframes: vec![
            awidat_proto::professional::Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: awidat_proto::professional::KeyframeInterpolation::Linear,
                easing: awidat_proto::professional::Easing::Linear,
            },
            awidat_proto::professional::Keyframe {
                time_s: 1.0,
                value: 12.0,
                interpolation: awidat_proto::professional::KeyframeInterpolation::Linear,
                easing: awidat_proto::professional::Easing::Linear,
            },
        ],
        motion_path: None,
        rationale: None,
    }];

    let selected = select_blur_radius_animation("clip-a", &animations)
        .expect("alias should select canonical blur radius animation");
    assert_eq!(selected.parameter, "awidat.blur.radius_px");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-render effect_parameter_alias_selects_blur_radius_animation
```

Expected: FAIL because render selection currently matches direct canonical strings only.

- [ ] **Step 3: Canonicalize render parameters**

In `crates/render/src/timeline.rs`, import `canonical_runtime_clip_parameter` from `awidat_proto::professional`. In functions that turn `ParameterAnimation` into `RenderParameterAnimation`, set the rendered parameter with:

```rust
let rendered_parameter = canonical_runtime_clip_parameter(parameter)
    .unwrap_or(parameter)
    .to_string();
```

Use `rendered_parameter` for `RenderParameterAnimation.parameter` and for all direct comparisons against `awidat.blur.*`, `awidat.shake.*`, and `awidat.warp.*`.

- [ ] **Step 4: Canonicalize professional template/runtime handling**

In `crates/render/src/professional.rs`, use `canonical_runtime_clip_parameter` anywhere an animation parameter is checked or copied into `RenderParameterAnimation`. The constructed render parameter must be canonical, while the persisted `ParameterAnimation` remains unchanged.

- [ ] **Step 5: Canonicalize EDL apply validation and effect auto-attachment**

In `crates/core/src/edl/apply.rs`, import `canonical_runtime_clip_parameter`. Use the canonical value when checking whether an applied animation requires adding `awidat.blur`, `awidat.shake`, or `awidat.warp` to a clip.

- [ ] **Step 6: Run focused checks**

Run:

```bash
cargo test -p awidat-render effect_parameter_alias_selects_blur_radius_animation
cargo test -p awidat-core blur_animation_auto_attaches_effect
cargo test -p awidat-proto runtime_effect_parameter_aliases
```

Expected: PASS. If specific test names differ, use `rg -n "auto_attaches|blur.*animation|select_blur" crates` to find the local names and run the closest focused tests.

## Task 3: TimeRemap Transition Primitive

**Files:**
- Modify: `crates/proto/src/transitions.rs`
- Modify: `crates/render/src/raw_stream_render.rs`

- [ ] **Step 1: Write failing transition primitive tests**

Add tests in `crates/proto/src/transitions.rs`:

```rust
#[test]
fn time_remap_primitive_validates_speed_curve() {
    let composition = TransitionComposition {
        version: 1,
        primitives: vec![TransitionPrimitive {
            start: 0.0,
            end: 1.0,
            easing: TransitionEasing::EaseInOut,
            op: TransitionPrimitiveOp::TimeRemap {
                speed: ParamCurve::Keyframes(vec![
                    Keyframe {
                        t: 0.0,
                        v: 0.75,
                        easing: TransitionEasing::EaseIn,
                    },
                    Keyframe {
                        t: 0.5,
                        v: 1.8,
                        easing: TransitionEasing::EaseOut,
                    },
                    Keyframe {
                        t: 1.0,
                        v: 1.0,
                        easing: TransitionEasing::Linear,
                    },
                ]),
            },
        }],
    };

    validate_transition_composition(&composition).unwrap();
    assert_eq!(resolve_composition_ffmpeg_xfade(&composition), None);
    assert_eq!(resolve_composition_gpu_shader(&composition), None);
}

#[test]
fn time_remap_primitive_rejects_non_positive_speed() {
    let composition = TransitionComposition {
        version: 1,
        primitives: vec![primitive(TransitionPrimitiveOp::TimeRemap {
            speed: ParamCurve::Const(0.0),
        })],
    };

    let err = validate_transition_composition(&composition).unwrap_err();
    assert!(err.to_string().contains("speed"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p awidat-proto time_remap_primitive
```

Expected: FAIL because `TransitionPrimitiveOp::TimeRemap` does not exist.

- [ ] **Step 3: Add primitive**

Add to `TransitionPrimitiveOp`:

```rust
/// Playback-speed curve applied across the transition window.
/// Values are multipliers where `1.0` is realtime, `0.5` is half speed,
/// and `2.0` is double speed.
TimeRemap {
    /// Speed multiplier curve in `[0.05, 8.0]`.
    speed: ParamCurve,
},
```

In `validate_primitive_op`, add:

```rust
TransitionPrimitiveOp::TimeRemap { speed } => {
    validate_curve_range(idx, "speed", speed, 0.05, 8.0)?;
}
```

In `primitive_ffmpeg_xfade`, `primitive_ffmpeg_priority`, `primitive_gpu_shader`, and `primitive_gpu_priority`, add `TimeRemap` branches that return no visual backend and priority `0`.

- [ ] **Step 4: Keep GPU curve extraction visual-only**

In `crates/render/src/raw_stream_render.rs`, leave `TimeRemap` ignored in `extract_param_curves` by adding an explicit `_ => continue` match arm or a direct `TransitionPrimitiveOp::TimeRemap { .. } => continue` arm if exhaustiveness requires it.

- [ ] **Step 5: Run proto tests**

Run:

```bash
cargo test -p awidat-proto time_remap_primitive
```

Expected: PASS.

## Task 4: Named Speed-Ramp Transitions And Planner Routing

**Files:**
- Modify: `crates/proto/src/transitions.rs`
- Modify: `crates/core/src/tools/plan_transition.rs`

- [ ] **Step 1: Write failing registry and planner tests**

Add to `crates/proto/src/transitions.rs` tests:

```rust
#[test]
fn ramp_in_beat_transition_has_time_remap_and_visual_accent() {
    let transition = lookup_builtin_transition("awidat.ramp_in_beat").unwrap();
    assert_eq!(transition.family, "speed_ramp");
    assert!(transition.best_for.contains(&"beat_hit"));

    let composition = stable_transition_composition("awidat.ramp_in_beat").unwrap();
    assert!(composition
        .primitives
        .iter()
        .any(|primitive| matches!(primitive.op, TransitionPrimitiveOp::TimeRemap { .. })));
    assert!(resolve_composition_ffmpeg_xfade(&composition).is_some());
}
```

Add to `crates/core/src/tools/plan_transition.rs` tests or create a small unit test for `transition_for_job` if the module already exposes private tests:

```rust
#[test]
fn beat_hit_prefers_speed_ramp_transition() {
    assert_eq!(transition_for_job("beat_hit", None), "awidat.ramp_in_beat");
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo test -p awidat-proto ramp_in_beat_transition_has_time_remap
cargo test -p awidat-core beat_hit_prefers_speed_ramp_transition
```

Expected: FAIL because the named transitions and planner mapping do not exist.

- [ ] **Step 3: Add built-in transition entries**

Add two `BuiltinTransition` entries to `BUILTIN_TRANSITIONS`:

```rust
BuiltinTransition {
    id: "awidat.ramp_in_beat",
    family: "speed_ramp",
    display_name: "Ramp In Beat",
    ffmpeg_xfade: Some("fadewhite"),
    default_duration_s: 0.32,
    min_duration_s: 0.16,
    max_duration_s: 0.65,
    audio_policy: TransitionAudioPolicy::Cut,
    best_for: &["beat_hit", "energy_jump", "music_sync"],
    avoid_for: &["static_dialogue", "clinical_documentary"],
    requires_motion_continuity: false,
    motion_alignment: None,
    color_sensitivity: ColorSensitivity::AvoidBrightToDark,
},
BuiltinTransition {
    id: "awidat.ramp_out_chapter",
    family: "speed_ramp",
    display_name: "Ramp Out Chapter",
    ffmpeg_xfade: Some("fadeblack"),
    default_duration_s: 0.55,
    min_duration_s: 0.24,
    max_duration_s: 1.0,
    audio_policy: TransitionAudioPolicy::Crossfade,
    best_for: &["chapter_reset", "soft_time_passage"],
    avoid_for: &["hard_beat_hit"],
    requires_motion_continuity: false,
    motion_alignment: None,
    color_sensitivity: ColorSensitivity::Insensitive,
},
```

- [ ] **Step 4: Add stable compositions**

In `stable_transition_composition`, add:

```rust
"awidat.ramp_in_beat" => Some(composition(vec![
    primitive(TransitionPrimitiveOp::TimeRemap {
        speed: ParamCurve::Keyframes(vec![
            Keyframe {
                t: 0.0,
                v: 0.7,
                easing: TransitionEasing::EaseIn,
            },
            Keyframe {
                t: 0.55,
                v: 1.85,
                easing: TransitionEasing::EaseOutExpo,
            },
            Keyframe {
                t: 1.0,
                v: 1.0,
                easing: TransitionEasing::EaseOut,
            },
        ]),
    }),
    primitive(TransitionPrimitiveOp::Flash {
        color: "#ffffff".to_string(),
        peak: 0.85,
    }),
])),
"awidat.ramp_out_chapter" => Some(composition(vec![
    primitive(TransitionPrimitiveOp::TimeRemap {
        speed: ParamCurve::Keyframes(vec![
            Keyframe {
                t: 0.0,
                v: 1.0,
                easing: TransitionEasing::EaseOut,
            },
            Keyframe {
                t: 0.7,
                v: 0.45,
                easing: TransitionEasing::EaseInOut,
            },
            Keyframe {
                t: 1.0,
                v: 1.0,
                easing: TransitionEasing::Linear,
            },
        ]),
    }),
    primitive(TransitionPrimitiveOp::Opacity {
        from: ParamCurve::Const(1.0),
        to: ParamCurve::Const(1.0),
    }),
])),
```

- [ ] **Step 5: Update planner routing**

In `transition_for_job`, change:

```rust
"beat_hit" => "awidat.flash_white",
```

to:

```rust
"beat_hit" => "awidat.ramp_in_beat",
```

Adjust the generated alternate list so `awidat.flash_white` is still visible as a fallback for beat-hit cuts if the local planner response shape has a transition alternate section. If the current shape only returns a hard-cut alternate, keep that shape unchanged and cover the fallback in the transition reason text.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p awidat-proto ramp_in_beat_transition_has_time_remap
cargo test -p awidat-core beat_hit_prefers_speed_ramp_transition
```

Expected: PASS.

## Task 5: Gap Analysis Docs And Full Verification

**Files:**
- Modify: `.reference-research/pro-editing-gap-analysis/04-motion-graphics.md`
- Modify: `.reference-research/pro-editing-gap-analysis/04-motion-graphics.html`
- Modify: `.reference-research/pro-editing-gap-analysis/05-transitions.md`
- Modify: `.reference-research/pro-editing-gap-analysis/05-transitions.html`

- [ ] **Step 1: Update Category 04 wording**

Replace stale “only 12 clip paths” claims with wording that matches the new runtime:

```markdown
Runtime animation now supports direct `title.*` / `overlay.*` paths plus an executable `effects.<effect_id>.params.<param>` namespace for the in-tree `awidat.blur`, `awidat.shake`, and `awidat.warp` effects. The remaining gap is breadth: new effect modules still need to declare animatable params through the same registry as they land.
```

- [ ] **Step 2: Update Category 05 wording**

Replace the “speed-ramp transitions are absent” status with:

```markdown
Speed-ramp transition semantics are now represented by `TransitionPrimitiveOp::TimeRemap` and the named `awidat.ramp_in_beat` / `awidat.ramp_out_chapter` recipes. `plan_transition` can choose a speed-ramp transition for `beat_hit`; render backends currently use the paired visual primitive as the export fallback until transition-local retime execution is implemented.
```

- [ ] **Step 3: Run formatting and focused tests**

Run:

```bash
cargo fmt --all -- --check
cargo test -p awidat-proto runtime_effect_parameter_aliases
cargo test -p awidat-proto time_remap_primitive
cargo test -p awidat-proto ramp_in_beat_transition_has_time_remap
cargo test -p awidat-core beat_hit_prefers_speed_ramp_transition
cargo test -p awidat-render effect_parameter_alias_selects_blur_radius_animation
```

Expected: all PASS. If disk remains full, report exact `df -h` output and the tests that could not run.

- [ ] **Step 4: Run broader checks if disk allows**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: PASS. If these are too expensive or blocked by local disk, run `cargo test -p awidat-proto`, `cargo test -p awidat-core plan_transition`, and `cargo test -p awidat-render animation` as the minimum focused fallback, then document the limitation.

- [ ] **Step 5: Final review**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors; changed files are limited to the files listed in this plan.

## Current Setup Blocker

Initial baseline tests in the isolated worktree failed before compilation because the filesystem had about `116MiB` available and Cargo reported `No space left on device`. Do not delete targets or artifacts from other worktrees because active agents may be using them. Continue once disk space is available, or run with a user-approved shared target/cache location that is known not to disrupt active work.
