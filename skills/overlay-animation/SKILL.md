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
      "prompt": "Animated product callout synced to the word launch"
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

## Delivery Rules

- Match the exact `duration_s`. Do not make the timeline absorb drift.
- Prefer transparent or keyed backgrounds for overlays.
- Keep important text inside mobile safe areas for vertical output.
- Keep generated assets project-relative.
- Do not paste raw backend filter graphs into EDL. The asset enters
  through `Insert PiP` or overlay `Insert BRoll`.

## Done When

- [ ] Every requested animation has a slot in the manifest.
- [ ] Every slot has a brief, generated asset path, and EDL insertion
      hint.
- [ ] Generated files exist under `generated/overlays/`.
- [ ] The overlay assets were inserted as timeline graph nodes.
- [ ] Render verification includes the affected timing windows.
