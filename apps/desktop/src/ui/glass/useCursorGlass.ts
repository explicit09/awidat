import { useCallback, useRef } from "react";

/**
 * useCursorGlass — drives the cursor-reactive specular sheen on a glass
 * surface (the "Liquid Glass reacts to movement" behavior).
 *
 * Returns a ref to attach to the glass element and an onMouseMove handler.
 * On move it writes the pointer position into the `--glass-mx` / `--glass-my`
 * custom properties (as percentages of the element box), which the
 * `.glass-reactive::after` radial-gradient reads to position the highlight.
 *
 * Writes go straight to the style attribute inside a rAF, so there's no
 * React re-render per mousemove — cheap enough for many simultaneous cards.
 *
 * Usage:
 *   const { ref, onMouseMove } = useCursorGlass<HTMLDivElement>();
 *   <div ref={ref} onMouseMove={onMouseMove} className="glass glass-reactive" />
 */
export function useCursorGlass<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const frame = useRef<number | null>(null);

  const onMouseMove = useCallback((e: React.MouseEvent<T>) => {
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const mx = ((e.clientX - rect.left) / rect.width) * 100;
    const my = ((e.clientY - rect.top) / rect.height) * 100;
    if (frame.current !== null) return; // coalesce to one write per frame
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      el.style.setProperty("--glass-mx", `${mx}%`);
      el.style.setProperty("--glass-my", `${my}%`);
    });
  }, []);

  return { ref, onMouseMove };
}
