---
name: color-corrector
description: Analyze footage color/exposure problems and apply clip-level graph-native color correction or LUTs through EDL ops.
version: 0.1.0
tier: finishing
tools_allowlist:
  - read_index
  - view_frame
  - view_program_frame
  - verify_render
  - color_scopes
  - view_timeline
  - inspect_clip
  - start_look_region_pass
  - plan_look_regions
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - review_look_regions
  - update_plan
  - bash
---

# Color corrector

Use this when the user asks to fix exposure, contrast, saturation,
white balance, camera matching, or LUT/look application. This is a
graph-native finishing pass: color intent must land as `Set Color
Correction` or `Apply LUT` EDL ops, never as a detached FFmpeg command
that bypasses the edit graph.

## Workflow

### 1. Inspect color signals

Start from the edit graph and the color index:

```
view_timeline
read_index(channel="color", asset_id=<asset>)
```

Start from `summary.policy` and, when present, `scenes[].policy`. The
policy is the routing layer:

- `recommended_action: no_op` means leave the graph untouched.
- `recommended_action: auto_correct` means a graph-native correction can
  be proposed from `recommended_correction`, then rendered and verified.
- `recommended_action: review_only` means inspect frames/contact sheets
  before applying anything.

Use `policy.edit_types` to decide the pass: `lighting_correction`,
`white_balance`, or `contrast_correction`. Then use
`summary.recommended_correction` and, when present,
`scenes[].recommended_correction` as the parameter source. Check
`issue_tags`, `confidence`, and `auto_correct_safe` before applying
anything. If the index marks a clip or scene as `unsafe_to_auto_correct`,
show the issue and inspect frames instead of blindly applying the value.

For multi-camera podcasts/interviews, compare each camera's brightness
percentiles, contrast range, RGB means, estimated color cast, clipping
fractions, and scene-level recommendations. Camera matching in v1 is
recommendation-driven: compute the per-camera deltas and emit normal
`Set Color Correction` ops on each clip.

Use the policy-to-EDL helper when a color index should be converted into
an edit proposal:

```bash
python3 <skill-root>/scripts/color_apply_plan.py \
  --color-index /path/to/color-index.json \
  --clip-uuid <clip_uuid>
```

Use the camera matching helper when multiple cameras need to be brought
into the same baseline:

```bash
python3 <skill-root>/scripts/camera_match_plan.py \
  --color-index cam-a.json \
  --color-index cam-b.json \
  --camera-label A \
  --camera-label B
```

For offline benchmark runs or dataset checks, use the bundled helper:

```bash
python3 <skill-root>/scripts/color_benchmark.py \
  --color-index /path/to/color-index.json \
  --dataset-dir /optional/external/dataset/cache
```

Datasets are only benchmark inputs. Do not make them runtime
dependencies and do not treat the benchmark as an alternate editor.

### 1.5. Plan look regions and generated LUTs

When the user asks for an end-to-end look pass, per-timeframe LUTs, or
agent-generated looks, produce a look-region plan before applying edits.
Prefer the first-class execution tool when the user wants the pass run:

```json
start_look_region_pass({"style":"cinematic"})
```

This tool runs the plan -> `apply_edl` -> `start_render(scope="timeline")`
sequence and returns the render `job_id`, render path, plan paths, and
the exact `review_look_regions` call to use after `poll_render` reports
`state: done`.

Use the planning-only tool when drafting or when the user wants to review
the EDL before applying it:

```json
plan_look_regions({"style":"cinematic"})
```

Both tools auto-discover color-analysis sidecars under
`index/color-analysis/`, writes `renders/look-plan.edl`,
`renders/look-plan.json`, `renders/look-plan.md`, and generated `.cube`
files under `luts/generated/`.

When running the helper directly for debugging, use:

```bash
python3 <skill-root>/scripts/look_region_plan.py \
  --project /path/to/project \
  --color-index /path/to/index/color-analysis/raw/source.mp4.json \
  --style cinematic \
  --output-edl /path/to/project/renders/look-plan.edl \
  --output-json /path/to/project/renders/look-plan.json \
  --report-md /path/to/project/renders/look-plan.md
```

Use multiple `--color-index` arguments, or the tool's `color_indexes`
array, when the timeline uses multiple assets and auto-discovery is not
enough. The planner reads `project.otio.json`, intersects timeline clips
with color-analysis scenes, records look-region consistency groups,
writes generated `.cube` files under `luts/generated/`, and emits
graph-native EDL using `Split Clip`, `Set Color Correction`, and
`Apply LUT`. The emitted EDL is still a proposal: apply it through
`apply_edl`, inspect `vedit_diff`, render, then verify visually.

Planner output fields to review:

- `regions[].timeline_start_s/end_s`: where the look lands in the edit.
- `regions[].sample_times_s`: timeline frames the agent should inspect
  in the rendered output with `view_frame` or contact sheets.
- `regions[].source_sample_times_s`: source-frame samples for checking
  the underlying media before the timeline render.
- `regions[].issue_tags`, `policy`, and `correction`: objective color
  reasoning from color-analysis.
- `regions[].consistency_group`: regions with the same asset/look/tag
  signature that should be reviewed for matching.
- `regions[].look_id`, `lut_path`, `score`, and `rationale`: selected
  generated look and why it was chosen.

Style presets are `natural`, `cinematic`, `warm`, `cool`, and `punchy`.
Use `natural` when the user wants correction/matching more than a
creative grade. Use a style preset only when the user has asked for a
look or when a finishing pass clearly calls for one.

The set of named looks the planner can emit lives in
`looks.toml` next to this file. Each entry carries the look's
display name, description, default input/output color space,
default cube size, recommended `strength` range, and tags. Call
the `list_looks` tool (no arguments) to read it as structured
JSON before reasoning about which look to plan — the catalog
is the source of truth, the planner script validates against it,
and the agent-facing `montage.color_pipeline` effect accepts the
same color-space ids.

Camera Log encodings (ARRI LogC3/LogC4, Sony S-Log3, Panasonic
V-Log, Blackmagic Film Gen 5) come with bundled 1D shaper LUTs
under `shapers/`. When a clip's `clip_input_space` is one of
these and the look LUT is authored in `rec709_g24`, set
`shaper_lut = "skills/color-corrector/shapers/<space>_to_rec709_g24.csp"`
on the `montage.color_pipeline` effect. The shaper converts the
log-encoded source into Rec.709 g2.4 *before* the look LUT runs,
so the LUT sees the pixel values it was authored for. To
regenerate the shapers (e.g. after updating a vendor EOTF
formula), run `python3 scripts/generate_shaper_luts.py`.

### 2. Apply corrections through the graph

For each corrected clip, emit one anchored op:

```text
*** Begin EDL
*** Set Color Correction
@@ anchor: clip_uuid=<clip_uuid>
+ exposure_ev: 0.150
+ contrast: 1.100
+ saturation: 1.050
+ temperature: -0.120
+ tint: 0.030
+ shadows: 0.100
+ highlights: -0.080
*** End EDL
```

Only include fields you intend to set. Re-applying replaces the existing
`montage.color_correction` effect on that clip, so do not stack multiple
correction ops to simulate one grade.

### 3. Apply LUTs through the graph

If the user asks for a LUT/look and the LUT file exists in the project,
use:

```text
*** Begin EDL
*** Apply LUT
@@ anchor: clip_uuid=<clip_uuid>
+ lut_path: luts/show-look.cube
+ interpolation: tetrahedral
*** End EDL
```

LUT paths are project-relative. Do not pass absolute paths or paths with
`.`, `..`, or backslashes. Supported render formats are `.3dl`,
`.cube`, `.dat`, `.m3d`, and `.csp`. Re-applying `Apply LUT` replaces
the clip's prior LUT. Use `Remove LUT` to clear the look from a clip
without touching color correction or audio effects. If the LUT is
missing, report the missing project-relative path rather than applying an
external render script.

Parser validation is intentionally strongest for `.cube` and `.3dl`.
The graph validator parses those two before accepting the EDL. `.dat`,
`.m3d`, and `.csp` are extension-accepted render paths that rely on the
FFmpeg LUT filters rather than in-tree parsers; do not describe them as
parser-validated.

### 4. Verify graph and render

After applying a batch:

```
vedit_diff
view_timeline
start_render(scope="timeline")
poll_render
```

Confirm the diff shows `montage.color_correction` or `montage.lut` effects
on the intended clips. Render at least a timeline preview for user-facing
finishing work. If render fails, fix the graph or report the exact
filter/LUT blocker.

Use `color_scopes` on representative frames when the decision depends on
objective color evidence. It returns luma histogram, RGB histogram,
luma waveform, RGB parade, and Cb/Cr vectorscope data for a single
frame. Pair it with `view_frame` when summarizing exposure, channel
balance, clipping risk, or chroma spread.

When comparing an uncorrected render against a corrected render, generate
a rendered contact sheet from the actual render outputs:

```bash
python3 <skill-root>/scripts/rendered_contact_sheet.py \
  --before-render /path/to/before.mp4 \
  --after-render /path/to/after.mp4 \
  --output /path/to/color-contact-sheet.ppm \
  --times 0,15,30
```

The contact sheet is visual evidence only. The project graph remains the
source of truth.

For look-region plans, the minimum verification package is:

1. Apply the generated EDL via `apply_edl`.
2. Review `vedit_diff` and confirm expected `montage.lut` effects and any
   split boundaries.
3. Render `scope="timeline"` or a relevant segment.
4. Generate a contact sheet at each timeline `regions[].sample_times_s`.
5. Summarize chosen regions, generated LUT paths, render path, and any
   low-confidence or `review_only` regions.

Use the first-class review tool for this final package after rendering:

```json
review_look_regions({
  "look_plan": "renders/look-plan.json",
  "after_render": "renders/timeline.mp4"
})
```

When running the helper directly for debugging, use:

```bash
python3 <skill-root>/scripts/look_region_review_package.py \
  --look-plan /path/to/project/renders/look-plan.json \
  --after-render /path/to/project/renders/timeline.mp4 \
  --contact-sheet /path/to/project/renders/look-review.ppm \
  --report-md /path/to/project/renders/look-review.md \
  --package-json /path/to/project/renders/look-review.json
```

Add `--before-render /path/to/baseline.mp4` when a before/after render is
available. Without it, the helper produces an after-only contact sheet
from the rendered look pass.

For risky clips, low-confidence clips, or user-facing color decisions,
generate a full review package:

```bash
python3 <skill-root>/scripts/color_review_package.py \
  --color-index /path/to/color-index.json \
  --before-render /path/to/before.mp4 \
  --after-render /path/to/after.mp4 \
  --contact-sheet /path/to/color-contact-sheet.ppm \
  --report-md /path/to/color-review.md
```

The report must summarize affected clips, issue tags, policy action,
correction values, render paths, and residual risk.

## Rules

- Color analysis proposes values; frame inspection confirms them.
- Do not overcorrect a camera to match a bad reference camera.
- Prioritize skin tone and exposure consistency over stylized saturation.
- Preserve clipped highlights if they are already gone; do not promise
  recovery the footage cannot support.
- Never create a separate corrected media file as the primary output
  path. The edit graph is the source of truth.
- Keep objective correction separate from creative grading. Correction
  fixes exposure, contrast, clipping risk, and color cast; LUTs or
  warmer/punchier looks are stylistic and require user/style intent.

## You are done when...

- [ ] `read_index(channel="color")` was consulted for every corrected
      source asset.
- [ ] `summary.policy` or `scenes[].policy` routed the decision.
- [ ] Representative frames were inspected before applying a batch.
- [ ] `issue_tags`, `confidence`, and `auto_correct_safe` were checked.
- [ ] Every correction landed through `apply_edl`.
- [ ] `vedit_diff` was reviewed.
- [ ] A render and, when useful, a rendered contact sheet were verified,
      or the exact blocker was reported.
