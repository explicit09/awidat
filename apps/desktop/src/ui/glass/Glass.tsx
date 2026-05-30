import type { CSSProperties, ReactNode } from "react";
import { useCursorGlass } from "./useCursorGlass";

type Elevation = "panel" | "strong" | "soft";

const ELEVATION_CLASS: Record<Elevation, string> = {
  panel: "glass",
  strong: "glass glass-strong",
  soft: "glass glass-soft",
};

/**
 * GlassPanel — a z1 frosted surface. Use for rails, chrome, modals.
 * `reactive` adds the cursor-tracking specular sheen.
 *
 * Text should NOT sit directly on a GlassPanel over a busy backdrop —
 * nest a <GlassContent> for anything users read. (2026 legibility rule.)
 */
export function GlassPanel({
  children,
  elevation = "panel",
  reactive = false,
  radius = 18,
  className = "",
  style,
}: {
  children?: ReactNode;
  elevation?: Elevation;
  reactive?: boolean;
  radius?: number;
  className?: string;
  style?: CSSProperties;
}) {
  const { ref, onMouseMove } = useCursorGlass<HTMLDivElement>();
  const reactiveProps = reactive
    ? { ref, onMouseMove, "data-reactive": true as const }
    : {};
  return (
    <div
      {...reactiveProps}
      className={`${ELEVATION_CLASS[elevation]}${reactive ? " glass-reactive" : ""} ${className}`}
      style={{ borderRadius: radius, ...style }}
    >
      {children}
    </div>
  );
}

/**
 * GlassContent — the z2 near-opaque card that holds text inside glass.
 * Optional accent glow (brand / cyan / violet) for active/selected states.
 */
export function GlassContent({
  children,
  glow,
  className = "",
  style,
  onClick,
}: {
  children?: ReactNode;
  glow?: "brand" | "cyan" | "violet";
  className?: string;
  style?: CSSProperties;
  onClick?: () => void;
}) {
  const glowClass = glow ? ` glow-${glow}` : "";
  return (
    <div
      className={`glass-content${glowClass} ${className}`}
      style={style}
      onClick={onClick}
    >
      {children}
    </div>
  );
}
