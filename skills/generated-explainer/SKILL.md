---
name: generated-explainer
description: Use when the user wants a generated educational, technical, mathematical, or visual-essay video rather than an edit built primarily from recorded footage.
version: 0.1.0
tier: producer
when_to_use: |
  Activate when the user wants a generated educational, technical, or visual-essay video rather than an edit built primarily from recorded footage.
tools_allowlist:
  - plan_visual_support
  - plan_motion_scene
  - import_local
  - list_assets
  - apply_edl
  - view_timeline
  - view_frame
  - view_program_frame
  - inspect_moment
  - assess_edit_quality
  - vedit_diff
  - vedit_commit
  - start_render
  - poll_render
  - verify_render
  - update_plan
  - bash
---

# Generated explainer

Produce a short narration-led explainer whose visuals are generated scene by
scene. Montage owns the script, timing, review loop, timeline, sound, captions,
and export. MotionScene, Manim, and Motion Canvas are scene backends—not separate
products and not replacements for Montage's edit graph.

Run under the shared producer discipline in
[`skills/_shared/producer-spine.md`](../_shared/producer-spine.md). Unlike a
footage-first producer, preflight checks the script, narration, scene source,
and generated renders rather than requiring footage indexes.

## Source bundle contract

Every explainer lives under:

```text
generated/explainers/<slug>/
  manifest.json
  script.md
  assets/
  scenes/<scene-id>/scene.py              # Manim
  scenes/<scene-id>/motion-scene.json     # native MotionScene
  scenes/<scene-id>/scene.tsx             # Motion Canvas
  renders/<scene-id>.mov
```

The scene source is the editable truth. Never leave a generated scene as only a
flattened movie. A scene may regenerate into the same stable render path so its
timeline placement does not change.

The manifest's `output_profile` is the quality authority for every backend and
the Montage timeline. The default `explainer-1440p60` profile is 2560×1440 at
60 fps with `upscale_policy: reject`. `standard-1080p30` and
`vertical-1080p60` are available when delivery requires them. Preview proxies
may be smaller; stable files under `renders/` are production inputs and must
meet or exceed the profile.

Use `scripts/explainer_bundle.py` to initialize and validate this structure:

```bash
python3 <skill-root>/scripts/explainer_bundle.py init \
  --project-root <project> --slug <slug> --title <title> \
  --script-file <script.md> --narration-file <narration.wav> \
  --profile explainer-1440p60

python3 <skill-root>/scripts/explainer_bundle.py add-scene \
  --bundle <project>/generated/explainers/<slug> \
  --id scene-001 --title <title> --backend manim --start 0 --end 8.5
```

The helper refuses duplicate ids and overlapping narration ranges. Do not
manually bypass those checks.

## Workflow

### 1. Lock the learning job

Read the script and identify the audience, prior knowledge, single takeaway,
target duration, and available narration. If the audience is unspecified, ask
before generating technical scenes; the same concept needs different visual
steps for a beginner and an expert.

Keep the first production slice to 60–90 seconds and 5–8 scenes unless the user
explicitly requests a larger scope. Each scene must advance the explanation,
not merely decorate the narration.

### 2. Make the scene plan

Create a numbered plan containing, for every scene:

- stable id (`scene-001`, `scene-002`, ...)
- narration start/end
- the exact idea the viewer should understand
- objects that persist or transform
- backend and why it is the smallest adequate option

Route each visual through `plan_visual_support`:

- **MotionScene** — text, panels, callouts, simple shapes, still diagrams, and
  lightweight transforms that preview natively.
- **Manim** — equations, graphs, vectors, geometry, simulations, and
  transformations where object continuity carries the explanation.
- **Motion Canvas** — rich custom 2D work that exceeds both earlier lanes; use
  it only after the user accepts the dependency cost.

The user confirms the scene plan before any scene generation. This is the one
structural approval gate; do not interrupt them for mechanical render choices.

### 3. Initialize the editable bundle

Run `explainer_bundle.py init`, passing `--narration-file` when recorded audio
exists, then `add-scene` once per approved scene. The helper copies narration
into project-owned `assets/`; do not time scenes against a movable external
file. Author or replace the scaffold at each manifest `source` path. Keep shared colors,
type, stroke widths, camera conventions, and object identities consistent
across scenes.

For native scenes, call `plan_motion_scene`, persist the resulting scene JSON at
the scaffolded `motion-scene.json` path, and apply its returned `Set Motion
Scene` operation. For Manim, render the `GeneratedScene` class to the manifest's
stable `.mov` path. Use Pango `Text` unless a working TeX toolchain has been
confirmed. For Motion Canvas, use the bundled template from `drawn-artifacts`.

Do not choose generation quality manually after bundle initialization. Read the
manifest profile and render each backend at its width, height, and fps. The
Manim scaffold already pins these values. A 1080p or 1440p filename is not
proof; `verify` probes the actual video stream.

Do not imitate another channel's protected characters, logos, or exact brand
system. Translate the explanatory grammar into the user's own visual identity.

### 4. Assemble against narration

Use the narration as the timing spine. Place rendered Manim/Motion Canvas scenes
as full-frame overlay B-roll at their manifest ranges with `apply_edl`; apply
native MotionScenes through their returned EDL. Every proposal rationale names
the explanatory job, for example: `"Keeps the rotating vector visible while the narration introduces phase."`

Before placing scenes, apply one `Set Output Format` operation using the
manifest's aspect ratio, width, height, and frame rate. This keeps Montage's
conform canvas identical to the generator profile; never rely on a nominal
1080p export to enlarge smaller scene renders. High-resolution or high-frame-
rate profiles use Montage's higher-quality H.264 encode path automatically.

After each placement, run `view_timeline` and inspect a frame inside the scene
with `view_program_frame` to check the composed timeline. A valid file path is not visual verification.

### 5. Revise by scene, not by flattening the film

When feedback targets one beat, edit only that scene source, regenerate the same
render path, and re-inspect its timeline window. Do not rebuild unrelated scenes
or replace the entire timeline. Preserve earlier renders and `vedit` checkpoints
until the replacement is accepted.

### 6. Verify and finish

Run:

```bash
python3 <skill-root>/scripts/explainer_bundle.py verify \
  --bundle <project>/generated/explainers/<slug> --require-renders
```

Native MotionScene entries do not require an external render file. Resolve every
missing, undersized, or low-frame-rate generated render before the final
timeline render. Treat quality failures as production blockers rather than
silently upscaling. Then
run `assess_edit_quality`, show the user the complete scene order, and wait for
their confirmation before `start_render`. Finish with `verify_render` and report
sound-design or music-selection gaps honestly.

## Done when

- [ ] The script and all editable scene sources are preserved in the bundle.
- [ ] The manifest has unique, ordered, non-overlapping narration ranges.
- [ ] Every generated render meets the manifest width, height, and frame rate.
- [ ] The timeline output format matches the manifest profile.
- [ ] Every scene uses the smallest adequate backend.
- [ ] The user approved the scene plan before generation.
- [ ] Every placed scene was checked in the timeline and visually inspected.
- [ ] Targeted feedback can regenerate one scene without disturbing the rest.
- [ ] Bundle verification and final render verification both pass.
