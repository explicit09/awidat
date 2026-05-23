export function transitionLabel(effectName: string): string {
  switch (effectName) {
    case "SMPTE_Dissolve":
    case "awidat.cross_dissolve":
    case "fade":
      return "Dissolve";
    case "awidat.fade_black":
    case "fadeblack":
      return "Fade Black";
    case "awidat.flash_white":
    case "fadewhite":
      return "Flash";
    case "awidat.slide_left":
    case "slideleft":
      return "Slide L";
    case "awidat.slide_right":
    case "slideright":
      return "Slide R";
    case "awidat.smooth_push_left":
    case "smoothleft":
      return "Push L";
    case "awidat.wipe_left":
    case "wipeleft":
      return "Wipe L";
    case "awidat.wipe_right":
    case "wiperight":
      return "Wipe R";
    case "awidat.zoom_in":
    case "zoomin":
      return "Zoom In";
    case "awidat.pixelize":
    case "pixelize":
      return "Pixelize";
    case "awidat.radial":
    case "radial":
      return "Radial";
    default:
      return effectName.replace(/^awidat\./, "").replace(/_/g, " ");
  }
}

/** Format a multiplier for badges: trailing-zero-trim, max 2 decimals. */
export function formatBadgeNumber(n: number): string {
  const fixed = n.toFixed(2);
  return fixed.replace(/\.?0+$/, "");
}

export function formatTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}
