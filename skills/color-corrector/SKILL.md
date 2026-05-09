---
name: color-corrector
description: Analyze footage color/exposure problems and apply clip-level graph-native color correction or LUTs through EDL ops.
version: 0.1.0
tier: finishing
tools_allowlist:
  - read_index
  - view_frame
  - view_timeline
  - inspect_clip
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
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
`awidat.color_correction` effect on that clip, so do not stack multiple
correction ops to simulate one grade.

### 3. Apply LUTs through the graph

If the user asks for a LUT/look and the LUT file exists in the project,
use:

```text
*** Begin EDL
*** Apply LUT
@@ anchor: clip_uuid=<clip_uuid>
+ lut_path: luts/show-look.cube
*** End EDL
```

LUT paths are project-relative. Do not pass absolute paths or paths with
`..`. If the LUT is missing, report the missing project-relative path
rather than applying an external render script.

### 4. Verify graph and render

After applying a batch:

```
vedit_diff
view_timeline
start_render(scope="timeline")
poll_render
```

Confirm the diff shows `awidat.color_correction` or `awidat.lut` effects
on the intended clips. Render at least a timeline preview for user-facing
finishing work. If render fails, fix the graph or report the exact
filter/LUT blocker.

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
