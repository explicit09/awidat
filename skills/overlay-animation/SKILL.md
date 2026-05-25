---
name: overlay-animation
description: Plan generated motion-graphic overlay assets as Awidat media overlays, with per-slot briefs, exact durations, and EDL insertion hints.
version: 0.1.0
tier: creative
tools_allowlist:
  - plan_visual_support
  - plan_motion_scene
  - view_timeline
  - inspect_moment
  - apply_edl
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Overlay Animation

Use this when the edit needs a generated motion graphic, callout,
animated stat card, visual explainer, lower-third treatment, or other
bespoke overlay asset.

Before generating an overlay asset, call `plan_visual_support` for the
visual need and read its `needs`, `intents`, `primary_lane`,
`supporting_lanes`, and `plan_steps`. If it returns `motion_scene`,
prefer `plan_motion_scene` and its `Set Motion Scene` EDL for native,
previewable/renderable layered motion. MotionScene supports multi-layer
text, rectangle/solid panels, callout rectangles, and project-relative
still image layers, with shared transforms and layer-local animations
for opacity, position, scale, and rotation; keep actual footage in
B-roll/PiP. Use the generated asset workflow below only when the scene
needs custom motion beyond that subset, subject-aware compositing, video
media inside the graphic, or a renderer path that MotionScene does not
yet provide.

## Principle

Generated animation is an asset workflow, not a custom render path. Each
animation slot produces a project file under `generated/overlays/<slug>/`
and then enters the timeline with normal Awidat overlay operations.

Prefer existing graph-native titles, captions, transitions, and
broadcast overlays when they solve the job. Use generated overlays only
when the graphic itself needs custom motion or visual design.

## Slot Planning

Create a slots JSON file with:

```json
{
  "slots": [
    {
      "name": "Launch Callout",
      "anchor": "clip_uuid=clip-a",
      "start_s": 2.0,
      "duration_s": 1.8,
      "engine": "canvas",
      "mode": "full_frame",
      "prompt": "Animated product callout synced to the word launch",
      "subject_aware": false,
      "text_layers": [
        {
          "text": "LAUNCH",
          "font_family": "Inter",
          "font_size": 120,
          "font_color": "#FFFFFF",
          "opacity": 1.0,
          "rotation": 0,
          "x": 0.5,
          "y": 0.42,
          "weight": "black"
        }
      ]
    }
  ]
}
```

Then generate a manifest:

```bash
python3 <skill-root>/scripts/overlay_animation_plan.py \
  --slots-json overlay-slots.json \
  --output-root generated/overlays
```

For each returned slot:

- Write or generate the asset at `asset_path`.
- Use the generated `brief` as the exact production contract.
- Insert the asset with the provided `edl_hint`, adjusting anchor,
  corner, scale, or margin only if the timeline needs it.
- Render and verify the slot in context.

Before insertion, verify generated assets:

```bash
python3 <skill-root>/scripts/overlay_asset_verify.py \
  --manifest overlay-manifest.json \
  --project-root .
```

The verifier must return `status: "ready"`. A `blocked` report means
the generated overlay, duration contract, project-relative paths, or
subject matte/cutout artifacts are not ready for timeline insertion.

For text-behind-subject preview evidence, generate a deterministic
before/after preview scorecard:

```bash
python3 <skill-root>/scripts/overlay_preview_evidence.py \
  --manifest overlay-manifest.json \
  --fixture-json overlay-preview-fixture.json \
  --output-root generated/previews \
  --project-root .
```

The fixture supplies frame dimensions and the subject bounds for each
subject-aware slot. The report writes an ordinary overlay preview, a
subject-safe preview, and a scorecard. Treat the scorecard as ready only
when ordinary overlay pixels intersect the subject and the subject-safe
preview has zero overlay pixels inside the subject bounds.

The same evidence helper can validate background treatment slots when a
slot includes `background_treatment`. Supported deterministic evidence
modes are `color` and `transparent`; heavier blur, image, or video
replacement belongs in a renderer or sidecar artifact with separate
render evidence.

For text-behind-subject or subject-aware overlay work, set
`subject_aware: true`, provide `subject_prompt`, and provide or generate
the `matte_path` artifact named in the manifest. The production contract
is base video -> overlay asset -> subject matte/cutout. If the matte is
unavailable, use the manifest fallback instead of pretending the effect
rendered correctly.

For subject-aware text, describe each visible text element in
`text_layers` instead of hiding styling in prose. Include text, font
family, font size, `#RRGGBB` color, opacity, rotation, normalized `x`
and `y` placement, and weight. When detection or segmentation still needs
to be produced, include a `detection` object with `object_classes`,
`confidence_threshold`, `iou_threshold`, `mask_threshold`, and any
available `preview_frame_path` or `occlusion_preview_path` evidence.

When a subject matte is not already available, create or request a
segmentation prompt package in `metadata.awidat.tracking_package`.
Include the target clip/range, target object id, intended output
(`subject_matte`, `text_behind_subject`, or `background_treatment`),
and reviewed positive/negative points, boxes, or mask references. Do
not treat a prose `subject_prompt` as enough evidence for subject-aware
compositing.

For background treatment planning, keep the creator intent explicit:

```json
{
  "background_treatment": {
    "mode": "color",
    "color": "#101820"
  }
}
```

Use `mode: "transparent"` only when downstream compositing expects a
transparent or keyed plate. Do not claim blur, image, or video
replacement readiness from the deterministic fixture helper alone.

## Delivery Rules

- Match the exact `duration_s`. Do not make the timeline absorb drift.
- Prefer transparent or keyed backgrounds for overlays.
- Keep important text inside mobile safe areas for vertical output.
- For subject-aware overlays, verify the matte exists and the subject
  remains visually in front of the overlay in the affected timing window.
  Include overlay preview evidence or rendered clip evidence before
  claiming a text-behind-subject effect is ready.
- Keep generated assets project-relative.
- Do not paste raw backend filter graphs into EDL. The asset enters
  through `Insert PiP` or overlay `Insert BRoll`.

## Done When

- [ ] Every requested animation has a slot in the manifest.
- [ ] Every slot has a brief, generated asset path, and EDL insertion
      hint.
- [ ] Generated files exist under `generated/overlays/`.
- [ ] `overlay_asset_verify.py` returns `status: "ready"` for the
      manifest before timeline insertion.
- [ ] Subject-aware slots include before/after preview evidence or
      rendered clip evidence with a passing occlusion scorecard.
- [ ] The overlay assets were inserted as timeline graph nodes.
- [ ] Render verification includes the affected timing windows.
