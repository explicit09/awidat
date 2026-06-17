---
name: drawn-artifacts
description: Generate charts, data visualizations, animated counters, and polished motion-graphic assets with open-source generators (matplotlib, Manim, Lottie, Motion Canvas) when the visual need goes beyond native MotionScene layers — then place them via PiP/B-roll or MotionScene image layers.
version: 0.1.0
tier: editorial
tools_allowlist:
  - plan_visual_support
  - plan_motion_scene
  - import_local
  - list_assets
  - apply_edl
  - view_timeline
  - view_frame
  - inspect_moment
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Drawn Artifacts

Use this when the edit needs a drawn or rendered graphic that native
MotionScene layers can't produce: real charts from real numbers,
math/concept animations, polished decorative motion (confetti,
flourishes, animated icons), or rich custom 2D scenes.

Before reaching for a generator, call `plan_visual_support`. If the
need fits MotionScene's native subset (multi-layer text, panels,
callouts, still image layers with simple animations), use
`plan_motion_scene` instead — it previews and renders natively with no
external toolchain. This skill is the escape hatch for everything past
that subset.

Remotion was deliberately excluded from this toolset (company
licensing terms). All four lanes below are permissively licensed.

## Routing

| Need | Generator | Script |
|---|---|---|
| Chart / data viz from real numbers (bar, line, comparison) | matplotlib | `scripts/chart.py` |
| Concept / math / counter animation (animated stat, growing bars, diagrams in motion) | Manim CE | `scripts/manim_scene.py` |
| Polished decorative motion (confetti, checkmarks, flourishes, animated icons) | Lottie (python-lottie + ffmpeg) | `scripts/lottie_render.py` |
| Rich custom 2D animated scenes beyond the above | Motion Canvas optional template | `motion-canvas/template/` + `motion-canvas/README.md` |

All scripts run through `uv` — no system Python packages needed. `uv`
is on PATH (`/opt/homebrew/bin/uv`) and also bundled with the desktop
app at `binaries/uv-aarch64-apple-darwin`. ffmpeg/ffprobe live at
`/opt/homebrew/bin/`.

## Brand palette

Every artifact uses the house palette unless the user overrides it:
gold `#C8A84E` (primary data/accent), navy `#070D17` (panels,
backgrounds when opacity is wanted), ivory `#F2EDE3` (labels, axes).
Backgrounds are transparent by default.

## 1. Charts — `scripts/chart.py`

```bash
uv run --with matplotlib <skill-root>/scripts/chart.py \
  --spec-json '{"type": "bar", "title": "Quarterly Revenue",
                "labels": ["Q1", "Q2", "Q3", "Q4"],
                "values": [12, 18, 15, 24], "y_label": "$M"}' \
  --out generated/drawn/q-revenue.png
```

- Types: `bar`, `line` (single or multi `series`), `comparison`
  (grouped bars from `series: [{name, values}, ...]`).
- Output is a transparent RGBA PNG (default 1600x900 @ 200 dpi;
  `--width/--height/--dpi` to change). Pass `"panel": true` in the
  spec for an 85%-opacity navy backing panel when the underlying
  footage is too busy for floating text.
- Place as a MotionScene still image layer (preferred for stills —
  you get fades/slides for free) or `Insert PiP`.

VERIFIED: bar/line/comparison specs all render; output probes as
`png / rgba` with genuinely transparent pixels (alpha extrema 0..255).

## 2. Manim animations — `scripts/manim_scene.py`

```bash
uv run --with manim manim render -qh -t --format=mov \
  <skill-root>/scripts/manim_scene.py CounterScene \
  -o counter.mov --media_dir /tmp/manim-media
cp /tmp/manim-media/videos/manim_scene/1080p60/counter.mov \
  generated/drawn/counter.mov
```

- `-t` = transparent background; `--format=mov` writes QuickTime
  Animation (qtrle, `argb` pixel format) whose alpha survives into the
  overlay compositor. Use `-ql` (480p15) for fast iteration, `-qh`
  (1080p60) for the final asset.
- Bundled template scenes, parameterized by env vars (no file edits):
  - `CounterScene` — animated stat counter
    (`DRAWN_COUNTER_START/END/SUFFIX/LABEL/SECONDS`).
  - `BarGrowthScene` — bars growing from the baseline
    (`DRAWN_BARS_JSON='[{"label": "2024", "value": 7}, ...]'`,
    `DRAWN_BARS_TITLE`, `DRAWN_BARS_SECONDS`).
- For custom scenes, copy `manim_scene.py` into the project, add a
  `Scene` subclass with the brand constants, render the same way.
- The bundled scenes use Pango `Text` only — they need NO LaTeX.
  Avoid `MathTex`/`Tex`/`DecimalNumber` in custom scenes unless the
  machine has a TeX distribution (this one does not).
- System deps: Manim needs cairo + pango. Present on this machine
  (`pkg-config --exists cairo pango` passes). On a fresh machine:
  `brew install cairo pango pkg-config`.

VERIFIED: both template scenes render end-to-end via
`uv run --with manim`; outputs probe as `qtrle / argb` (alpha kept);
env-var parametrization confirmed.

## 3. Lottie — `scripts/lottie_render.py`

```bash
uv run --with 'lottie[all]' <skill-root>/scripts/lottie_render.py \
  --input assets/confetti.json \
  --out generated/drawn/confetti.mov
```

- Renders the Lottie JSON to a transparent PNG sequence, then
  assembles a ProRes 4444 .mov (`--codec qtrle` for the lighter
  QuickTime Animation codec). `--fps`, `--scale` to adjust;
  `--frames-dir` keeps the PNG sequence (use `--png-only
  --frames-dir ...` when you only want a single frame or a
  MotionScene still).
- Sourcing: lottiefiles.com has a large free library. Download the
  **Lottie JSON** format (if you get a `.lottie` file, unzip it and
  use `animations/*.json`). Every asset carries its own license —
  surface the asset URL to the USER and have them verify the license
  before it ships in a published edit. Don't bulk-download.

VERIFIED: sample animation rendered end-to-end; .mov probes as
`prores / yuva444p12le` (alpha kept), PNG frames probe as `rgba`.

## 4. Motion Canvas optional template

Use this only when native MotionScene, matplotlib, Manim, or Lottie
cannot express the brief. The repo includes a ready template at
`<skill-root>/motion-canvas/template/`, pinned to the current
Motion Canvas package family. It is opt-in: copy the template into the
active project, run `npm install` there when the user accepts the
dependency cost, author scenes in `src/scenes/*.tsx`, and run
`npm run serve` to open the Motion Canvas editor at
`http://127.0.0.1:9000`.

Export transparent PNG frames from the editor, then run
`npm run export:frames -- --frames output/frame_%05d.png --out ../<slug>.mov`
inside the copied template (cwd is `generated/drawn/motion-canvas`, so
`../<slug>.mov` resolves to the project-level `generated/drawn/<slug>.mov`).
Place the resulting ProRes 4444 `.mov`
through `Insert PiP` or overlay `Insert BRoll`, then verify with
`view_frame` or a short render.

## Alpha support in the render pipeline (verified facts)

The Montage render pipeline DOES composite alpha video overlays. In
`crates/render/src/timeline.rs`, the media-overlay filter chain forces
`format=rgba` after scaling specifically so "transparent overlays
(e.g. VP9 yuva420p WebM or ProRes 4444) keep their alpha channel into
the `overlay=` compositor", and every transform path
(rotation/mask/matte/opacity) re-asserts `format=rgba`. MotionScene
still-image layers go through the same `format=rgba` treatment.

Practical consequences:

- ProRes 4444 (`yuva444p10le/12le`) and QuickTime Animation (qtrle,
  `argb`) .mov files from the manim/Lottie lanes alpha-composite
  correctly via `Insert PiP` and overlay `Insert BRoll`.
- Transparent PNGs work both as MotionScene image layers and as PiP
  assets.
- Full-frame overlays with an explicit `blend_mode` go through
  `blend=` instead of `overlay=`; for those, alpha is not the
  compositing mechanism — prefer plain PiP/B-roll placement for drawn
  artifacts.
- Still verify visually: alpha decode is necessary, not sufficient
  (wrong-size renders, unreadable text over busy footage).

## Placement workflow

1. Generate the artifact into the active project under
   `generated/drawn/` (create the directory if needed). Assets
   generated inside the project are already project-relative; only
   use `import_local` for files produced OUTSIDE the project root.
2. Match the asset duration to the slot — don't make the timeline
   absorb drift. Probe with
   `ffprobe -show_entries format=duration <asset>`.
3. Place it:
   - Animated .mov → `*** Insert PiP` (corner placement, scale
     0.10–0.60) or overlay `*** Insert BRoll` (full-frame cutaway)
     anchored per `view_timeline` clip anchors.
   - Still PNG → MotionScene image layer via `plan_motion_scene` /
     `Set Motion Scene` (preferred: native fade/slide animations), or
     PiP when it must sit above other graph features.
4. Verify: `view_timeline` around the anchor to confirm track/timing,
   then `view_frame` (or `start_render` of the affected window) at a
   timestamp INSIDE the artifact's window to confirm the alpha
   composite reads correctly over the footage.

## Common failure modes

- **First `uv run` is slow**: uv resolves and caches the
  environment on first use (manim is the heavy one). Subsequent runs
  are fast. Don't kill a first run that looks stalled — give it a
  few minutes.
- **Manim "No such file or directory: 'latex'"**: a custom scene used
  a TeX-based mobject. Rewrite with Pango `Text`/`MarkupText` or have
  the user install a TeX distribution.
- **Lottie parse failure**: the file is probably dotLottie (a zip) or
  uses unsupported plugin features. Unzip `.lottie` files; for
  unsupported features, pick a simpler asset.
- **Overlay renders as a solid rectangle**: the asset has no alpha
  channel — probe `pix_fmt`. Re-export as ProRes 4444 / qtrle, or
  re-render the PNG without `panel`.
- **Chart text unreadable over footage**: regenerate with
  `"panel": true` or place over a calmer section.

## Don't

- Don't reach for a generator when MotionScene's native layers
  already cover the need — native scenes preview live and re-render
  with the project.
- Don't hardcode numbers into chart visuals that the transcript
  doesn't support — charts assert facts; verify the values first.
- Don't ship a LottieFiles asset without the user confirming its
  license.
- Don't run `npm install` for Motion Canvas during ordinary edits; it
  is only appropriate after the user has chosen this optional lane.
- Don't paste raw filter graphs into EDL — artifacts enter through
  `Insert PiP`, `Insert BRoll`, or `Set Motion Scene` only.

## You are done when...

- [ ] Every drawn artifact exists under `generated/drawn/` and probes
      with the expected codec/pix_fmt (alpha where intended).
- [ ] Each placement went through `Insert PiP`, overlay
      `Insert BRoll`, or a MotionScene image layer — no bespoke
      render paths.
- [ ] `view_timeline` confirms track + timing for each placement.
- [ ] A rendered frame (`view_frame` or preview render) inside each
      artifact's window confirms the composite reads correctly.
- [ ] Chart numbers trace back to a source the user can check.
- [ ] Any Lottie asset's license was surfaced to the user.
