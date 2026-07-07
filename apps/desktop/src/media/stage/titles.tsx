import type { TimelineParameterAnimation } from "../../protocol";
import { clampOpacity, evaluateAnimations } from "../../timeline/animation";
import type { TimelineSnapshot } from "../../timeline/store";

export type PreviewTitleOverlay = {
  key: string;
  startS: number;
  endS: number;
  text: string;
  position: "top" | "center" | "bottom";
  fontSize: number;
  color: string;
  fontWeight: "normal" | "bold";
  animation: "none" | "fade_in" | "fade_out" | "fade_in_out" | "slide_in" | "slide_out";
  reveal: "none" | "typewriter" | "word" | "line";
  animations: TimelineParameterAnimation[];
  /** True for MotionScene text layers (protocol role "motion_scene"). */
  isMotionScene: boolean;
  /**
   * Explicit normalized text box (MotionScene layers with x/y params).
   * `x`/`y` are the box center in program-frame space; `null` falls
   * back to the `position` band layout.
   */
  box: {
    x: number;
    y: number;
    width: number | null;
    align: "left" | "center" | "right";
  } | null;
};

export function activeTitleOverlays(
  snapshot: TimelineSnapshot,
  _durationS: number,
): PreviewTitleOverlay[] {
  // A broadcast overlay owns the regular program titles, but MotionScene
  // text is program content (panels, labels, diagrams) — it must keep
  // rendering alongside the broadcast chrome, exactly as the render
  // plan keeps role "motion_scene" titles.
  const suppressProgramTitles = broadcastOverlayOwnsProgramTitles(
    snapshot.broadcast_overlay,
  );

  const overlays: PreviewTitleOverlay[] = [];
  for (const track of snapshot.tracks) {
    if (track.role !== "titles") continue;
    for (const item of track.items) {
      if (item.kind !== "clip" || item.title === null) continue;
      const isMotionScene = item.title.role === "motion_scene";
      if (isMotionScene) continue;
      if (suppressProgramTitles && !isMotionScene) continue;
      const startS = item.track_start_s;
      const endS = item.track_start_s + item.duration_s;
      if (!Number.isFinite(startS) || !Number.isFinite(endS) || endS <= startS) {
        continue;
      }
      overlays.push({
        key: item.clip_uuid || item.name,
        startS,
        endS,
        text: item.title.text,
        position: titlePosition(item.title.position),
        fontSize: item.title.font_size,
        color: item.title.color || "#FFFFFF",
        fontWeight: item.title.font_weight === "bold" ? "bold" : "normal",
        animation: titleAnimation(item.title.animation),
        reveal: titleReveal(item.title.reveal),
        animations: item.animations ?? [],
        isMotionScene,
        box: titleOverlayBox(item.title),
      });
    }
  }
  return overlays;
}

export function titleOverlayBox(title: {
  x: number | null;
  y: number | null;
  width: number | null;
  align: string | null;
}): PreviewTitleOverlay["box"] {
  if (
    title.x === null ||
    title.y === null ||
    !Number.isFinite(title.x) ||
    !Number.isFinite(title.y)
  ) {
    return null;
  }
  return {
    x: title.x,
    y: title.y,
    width:
      title.width !== null && Number.isFinite(title.width) && title.width > 0
        ? title.width
        : null,
    align: titleAlign(title.align),
  };
}

export function titleAlign(value: string | null): "left" | "center" | "right" {
  return value === "left" || value === "right" ? value : "center";
}

export function broadcastOverlayOwnsProgramTitles(
  overlay: TimelineSnapshot["broadcast_overlay"],
): boolean {
  return Boolean(overlay?.enabled && !overlay.short_form_mode);
}

export function TimelineTitleOverlays({
  overlays,
  timelineTime,
}: {
  overlays: PreviewTitleOverlay[];
  timelineTime: number;
}) {
  const active = overlays.filter(
    (overlay) => timelineTime >= overlay.startS && timelineTime < overlay.endS,
  );
  if (active.length === 0) return null;
  return (
    <div className="timeline-title-layer" aria-hidden="true">
      {active.map((overlay) => (
        <div
          key={overlay.key}
          className={
            overlay.box
              ? "timeline-title-overlay"
              : `timeline-title-overlay title-pos-${overlay.position}`
          }
          style={titleOverlayStyle(overlay, timelineTime)}
        >
          {titleRevealText(overlay, timelineTime)}
        </div>
      ))}
    </div>
  );
}

export function titleOverlayStyle(
  overlay: PreviewTitleOverlay,
  timelineTime: number,
): React.CSSProperties {
  const elapsed = timelineTime - overlay.startS;
  const remaining = overlay.endS - timelineTime;
  const fadeIn = Math.min(1, Math.max(0, elapsed / 0.45));
  const fadeOut = Math.min(1, Math.max(0, remaining / 0.45));
  let opacity = 1;
  if (overlay.animation === "fade_in") opacity = fadeIn;
  if (overlay.animation === "fade_out") opacity = fadeOut;
  if (overlay.animation === "fade_in_out") opacity = Math.min(fadeIn, fadeOut);

  let translateX = "-50%";
  if (overlay.animation === "slide_in" && elapsed < 0.55) {
    const p = Math.min(1, Math.max(0, elapsed / 0.55));
    translateX = `calc(-50% + ${(1 - p) * -18}%)`;
  } else if (overlay.animation === "slide_out" && remaining < 0.55) {
    const p = Math.min(1, Math.max(0, remaining / 0.55));
    translateX = `calc(-50% + ${(1 - p) * 18}%)`;
  }
  // Explicit boxes center on (x, y); band layout centers vertically
  // only for the "center" band.
  const translateY = overlay.box || overlay.position === "center" ? "-50%" : "0";
  const animated = evaluateAnimations(overlay.animations, elapsed);
  if (animated["title.opacity"] !== undefined) {
    opacity = clampOpacity(animated["title.opacity"]);
  }
  if (animated["overlay.opacity"] !== undefined) {
    opacity = clampOpacity(animated["overlay.opacity"]);
  }
  const fontSize = animated["title.font_size"] ?? overlay.fontSize;
  const xOffset = animated["title.x"] ?? 0;
  const yOffset = animated["title.y"] ?? 0;

  const style: React.CSSProperties = {
    color: overlay.color,
    fontSize: `clamp(11px, ${Math.max(1.2, fontSize / 22).toFixed(2)}vw, ${fontSize}px)`,
    fontWeight: overlay.fontWeight === "bold" ? 750 : 500,
    opacity,
    transform: `translate(calc(${translateX} + ${xOffset * 100}vw), calc(${translateY} + ${yOffset * 100}vh))`,
  };
  if (overlay.box) {
    // MotionScene text box: position at the normalized box center
    // (translate(-50%, -50%) recenters the element on that point).
    style.left = `${overlay.box.x * 100}%`;
    style.top = `${overlay.box.y * 100}%`;
    // Without an explicit width, fall back to the widest centered box
    // that stays on screen (mirrors the render's effective_width) so
    // long text wraps instead of overflowing the frame.
    const width =
      overlay.box.width ??
      Math.max(
        0.05,
        Math.min(0.92, 2 * overlay.box.x, 2 * (1 - overlay.box.x)),
      );
    style.width = `${width * 100}%`;
    style.maxWidth = "none";
    style.textAlign = overlay.box.align;
    // Scene text uses explicit line breaks ("1\nCLIENT NEED") and
    // wraps inside its box instead of the single-line band layout.
    style.whiteSpace = "pre-line";
    style.lineHeight = 1.15;
  }
  return style;
}

export function titlePosition(value: string): PreviewTitleOverlay["position"] {
  return value === "top" || value === "bottom" ? value : "center";
}

export function titleAnimation(value: string): PreviewTitleOverlay["animation"] {
  switch (value) {
    case "fade_in":
    case "fade_out":
    case "fade_in_out":
    case "slide_in":
    case "slide_out":
      return value;
    default:
      return "none";
  }
}

export function titleReveal(value: string): PreviewTitleOverlay["reveal"] {
  switch (value) {
    case "typewriter":
    case "word":
    case "line":
      return value;
    default:
      return "none";
  }
}

export function titleRevealText(overlay: PreviewTitleOverlay, timelineTime: number): string {
  if (overlay.reveal === "none") return overlay.text;
  const elapsed = Math.max(0, timelineTime - overlay.startS);
  const duration = Math.max(0.001, overlay.endS - overlay.startS);
  const progress = Math.min(1, elapsed / duration);
  const steps = revealSteps(overlay.text, overlay.reveal);
  if (steps.length === 0) return "";
  const index = Math.min(steps.length - 1, Math.floor(progress * steps.length));
  return steps[index];
}

export function revealSteps(text: string, reveal: PreviewTitleOverlay["reveal"]): string[] {
  if (reveal === "typewriter") {
    return Array.from(text).map((_, index, chars) => chars.slice(0, index + 1).join(""));
  }
  if (reveal === "word") {
    const matches = [...text.matchAll(/\S+/g)];
    return matches.map((match) => text.slice(0, (match.index ?? 0) + match[0].length));
  }
  if (reveal === "line") {
    const lines = text.match(/.*(?:\n|$)/g)?.filter((line) => line.length > 0) ?? [];
    let cursor = 0;
    return lines.map((line) => {
      cursor += line.length;
      return text.slice(0, cursor);
    });
  }
  return [text];
}
