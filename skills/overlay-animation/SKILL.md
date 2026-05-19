---
name: overlay-animation
description: Plan generated motion-graphic overlay assets as Awidat media overlays, with per-slot briefs, exact durations, and EDL insertion hints.
version: 0.1.0
tier: creative
tools_allowlist:
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
      "subject_aware": false
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

For text-behind-subject or subject-aware overlay work, set
`subject_aware: true`, provide `subject_prompt`, and provide or generate
the `matte_path` artifact named in the manifest. The production contract
is base video -> overlay asset -> subject matte/cutout. If the matte is
unavailable, use the manifest fallback instead of pretending the effect
rendered correctly.

## Delivery Rules

- Match the exact `duration_s`. Do not make the timeline absorb drift.
- Prefer transparent or keyed backgrounds for overlays.
- Keep important text inside mobile safe areas for vertical output.
- For subject-aware overlays, verify the matte exists and the subject
  remains visually in front of the overlay in the affected timing window.
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
- [ ] The overlay assets were inserted as timeline graph nodes.
- [ ] Render verification includes the affected timing windows.
