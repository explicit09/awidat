export function transitionLabel(effectName: string): string {
  switch (effectName) {
    case "SMPTE_Dissolve":
    case "montage.cross_dissolve":
    case "fade":
      return "Dissolve";
    case "montage.fade_black":
    case "fadeblack":
      return "Fade Black";
    case "montage.flash_white":
    case "fadewhite":
      return "Flash";
    case "montage.slide_left":
    case "slideleft":
      return "Slide L";
    case "montage.slide_right":
    case "slideright":
      return "Slide R";
    case "montage.smooth_push_left":
    case "smoothleft":
      return "Push L";
    case "montage.wipe_left":
    case "wipeleft":
      return "Wipe L";
    case "montage.wipe_right":
    case "wiperight":
      return "Wipe R";
    case "montage.zoom_in":
    case "zoomin":
      return "Zoom In";
    case "montage.pixelize":
    case "pixelize":
      return "Pixelize";
    case "montage.radial":
    case "radial":
      return "Radial";
    default:
      return effectName.replace(/^montage\./, "").replace(/_/g, " ");
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
