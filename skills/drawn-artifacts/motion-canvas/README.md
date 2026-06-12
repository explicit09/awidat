# Motion Canvas optional template

Motion Canvas (https://motioncanvas.io, MIT license) is a Node/Vite
TypeScript framework for rich custom 2D animated scenes. It is the
right tool when a brief outgrows matplotlib/manim/Lottie — bespoke
choreographed infographics, multi-element animated diagrams, kinetic
typography systems.

It is optional, not part of the default Montage build. The template in
`template/` gives a ready starting point without adding Node
dependencies to normal editing workflows.

## Optional setup

Copy the template into the active project when the user chooses this
lane:

```bash
mkdir -p generated/drawn/motion-canvas
cp -R <skill-root>/motion-canvas/template/. generated/drawn/motion-canvas/
cd generated/drawn/motion-canvas
npm install
npm run serve
```

The editor opens at `http://127.0.0.1:9000`. The template includes a
brand-card scene using the Montage palette: gold `#C8A84E`, navy
`#070D17`, ivory `#F2EDE3`. Author new scenes in `src/scenes/*.tsx`
and register them from `src/project.ts` with the required `?scene`
import suffix.

## Handoff back to Montage

In the Motion Canvas editor, export a transparent PNG sequence. Then
assemble it into a transparent ProRes 4444 asset:

```bash
# cwd is generated/drawn/motion-canvas (the copied template), so `../<slug>.mov`
# lands at the project-level generated/drawn/<slug>.mov the placement step expects.
npm run export:frames -- \
  --frames output/frame_%05d.png \
  --fps 30 \
  --out ../<slug>.mov
```

Place the `.mov` with `Insert PiP` or overlay `Insert BRoll`, then
verify with `view_frame` or a short render inside the asset's timeline
window.

Do not run `npm install` for Motion Canvas during ordinary edits. This
lane is for briefs that genuinely need custom 2D choreography beyond
native MotionScene, matplotlib, Manim, and Lottie.

Note: Remotion (the other popular React video framework) was
deliberately excluded — its company license terms don't fit this
project's distribution model. Motion Canvas is MIT.
