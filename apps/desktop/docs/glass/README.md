# Obsidian Glass — Montage 2026 UI experiment

A dark-first liquid-glass design layer, built from 2026 dark-glassmorphism
and Apple Liquid Glass research. Branch: `feat/ui-2026-glass`.

## See it (no Tauri / Rust build needed)

```bash
cd apps/desktop
pnpm dev
# open http://localhost:1420/glass.html
```

Three scenes via the floating switch at the bottom:

1. **Landing** (`01-landing.png`) — no-project hero on the ambient orb mesh.
2. **Editor** (`02-editor.png`) — frosted chrome, glass agent rail, the Brief
   with cursor-reactive proposal cards (medium-colored glows), glass inspector.
3. **Components** (`03-components.png`) — the glass primitive gallery.

Move your cursor across any reactive panel/card — the specular sheen follows.

## What's in it

| File | Role |
|---|---|
| `src/ui/glass.css` | The system: 3-plane depth model (atmosphere / glass / content), specular edges, cursor sheen, brand-as-ambient-light orbs, grain, a11y fallbacks |
| `src/ui/glass/AmbientBackground.tsx` | Drifting orange + amber + teal + violet orb mesh + film grain + vignette |
| `src/ui/glass/useCursorGlass.ts` | rAF pointer-tracking sheen (no React re-render) |
| `src/ui/glass/Glass.tsx` | `GlassPanel` (z1 frosted) + `GlassContent` (z2 opaque, holds text) |
| `src/ui/glass/GlassButton.tsx` | `cta` (radiating orange) + `ghost` (frosted) |
| `src/glassShowcase.tsx` + `glass.html` | The browser showcase |

## Design rules (the ones that matter)

- **Text never sits on busy glass.** It lives on `GlassContent` (z2, near-opaque).
- **Orange is light, not paint.** The brand radiates from behind glass as an
  orb; CTAs glow rather than just fill.
- **Honors the environment.** `prefers-reduced-transparency` → solid surfaces;
  `prefers-reduced-motion` → orbs freeze; no `backdrop-filter` → opaque fallback.

## Status

This is the **design layer + showcase**, verified (`tsc` clean, `vite build`
bundles, all three scenes render with zero console errors). It is NOT yet wired
into the real `App.tsx` surfaces — that's the next step once the direction is
approved. Adopting it means: import `glass.css`, drop `<AmbientBackground />` at
the root, and swap the real chrome/rails/Brief to `GlassPanel` / `GlassContent`.
