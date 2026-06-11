# Motion Canvas (deferred — manual setup required)

Motion Canvas (https://motioncanvas.io, MIT license) is a Node/Vite
TypeScript framework for rich custom 2D animated scenes. It is the
right tool when a brief outgrows matplotlib/manim/Lottie — bespoke
choreographed infographics, multi-element animated diagrams, kinetic
typography systems.

It is NOT scaffolded as a runnable project here, deliberately:

- Rendering requires a full `npm install` of `@motion-canvas/core`,
  `@motion-canvas/2d`, `@motion-canvas/ui`, and Vite — hundreds of MB
  and minutes of install time the agent should not burn mid-edit.
- There is no first-class headless render CLI: the supported render
  path runs the Vite dev server and drives the editor's Render button
  in a browser (manually or via Puppeteer automation). That browser
  dependency makes unattended agent-time rendering fragile.

## Manual setup (one-time, done by the user)

```bash
npm init @motion-canvas@latest   # scaffold a project (pick 2D)
cd <project>
npm install
npm run serve                    # opens the editor at localhost:9000
```

Author scenes in `src/scenes/*.tsx` using the brand palette
(gold `#C8A84E`, navy `#070D17`, ivory `#F2EDE3`). In the editor,
choose **Render**, set the output to image sequence (PNG, transparent
background) or use `@motion-canvas/ffmpeg` for direct video export.

## Handoff back to Montage

Export a transparent PNG sequence, then assemble it with the same
ffmpeg recipe the Lottie lane uses:

```bash
ffmpeg -framerate 30 -i frame_%05d.png \
  -c:v prores_ks -profile:v 4444 -pix_fmt yuva444p10le \
  generated/drawn/<slug>.mov
```

Drop the .mov in the active project under `generated/drawn/` and place
it with `Insert PiP` / `Insert BRoll` like any other drawn artifact.

Note: Remotion (the other popular React video framework) was
deliberately excluded — its company license terms don't fit this
project's distribution model. Motion Canvas is MIT.
