# Montage glass styling

The application imports `src/ui/glass.css` and renders
`src/ui/glass/AmbientBackground.tsx` in its root. The stylesheet provides
surface depth, ambient color, and reduced-motion/transparency fallbacks.

Use the actual workspace for UI review. `tests/ui-harness.html?project=1`
loads the application with the shared Tauri IPC fixture under Vite;
`node tests/desktop-ui-smoke.mjs` checks landing and workspace rendering.

The standalone design galleries and unused glass component wrappers have
been retired. The application is the source of truth for its appearance.
