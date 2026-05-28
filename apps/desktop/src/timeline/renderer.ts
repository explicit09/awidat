import type { TitleStyling } from "../protocol";
import { snapMoveDeltaS, type UserMoveDrag } from "./editMath.ts";
import { formatBadgeNumber, formatTime, transitionLabel } from "./formatting.ts";
import { CLIP_PADDING_X, LANE_HEIGHT, RULER_HEIGHT } from "./layout.ts";
import {
  fillRoundedRect,
  pathRoundedRect,
  strokeRoundedRect,
  truncateToWidth,
} from "./canvasPrimitives.ts";
import type { TimelineItem, TimelineSnapshot } from "./store.ts";
import { getStrip } from "./thumbnailCache.ts";
import { getBuckets } from "./waveformCache.ts";

// Canvas needs literal colors but every value mirrors a token from
// `ui/tokens.css`. Keep them in sync — if a token shifts, update its
// twin here. Listed once so individual draw fns can pull from one
// place instead of repeating hex literals.
const TOKEN = {
  /** --color-surface-panel */
  surfacePanel: "#0F0F11",
  /** --color-surface-card */
  surfaceCard: "#18181A",
  /** lane background tints — derived from panel with a subtle channel
   *  shift so video / audio / titles tracks read as distinct rows. */
  laneVideo: "#0F0F11",
  laneAudio: "#0E1110",
  laneTitle: "#0E1219",
  /** --color-border-subtle */
  borderSubtle: "#1F1F22",
  /** --color-border */
  border: "#2D2D30",
  /** --color-text-primary */
  textPrimary: "#E6E6E6",
  /** --color-text-secondary */
  textSecondary: "#B4B4B8",
  /** --color-text-muted */
  textMuted: "#7A7A82",
  /** --color-brand-secondary (cyan accents: selection, playhead, highlight) */
  brandSecondary: "#38BDF8",
  /** --color-brand-secondary-hover (lighter cyan) */
  brandSecondaryHover: "#7DD3FC",
  /** --color-brand-purple (transitions, motion family) */
  brandPurple: "#A855F7",
  /** --color-viz-audio — slate-grey baseline for inactive audio */
  vizAudio: "#64748B",
  /** --color-job-ready-dot — green reserved for the active audio clip */
  jobReadyDot: "#22C55E",
  /** --color-danger / deleted state */
  danger: "#EF4444",
  /** --color-warning / title-clip border */
  warning: "#F59E0B",
} as const;

const FONT_SANS = '"Inter", ui-sans-serif, system-ui, sans-serif';
const FONT_MONO = '"JetBrains Mono", ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace';

/** Width of the inline track-header pill drawn at the left edge of
 *  every lane (V1 / A1 / …). Sized so 3-character labels fit comfortably
 *  while leaving the rest of the lane free for clip content. */
const TRACK_HEADER_WIDTH = 44;

export function drawMoveGhost(
  ctx: CanvasRenderingContext2D,
  snapshot: TimelineSnapshot,
  currentTime: number,
  drag: UserMoveDrag,
  pps: number,
  laneHeight: number = LANE_HEIGHT,
) {
  const dx = snapMoveDeltaS(snapshot, currentTime, drag, pps) * pps;
  const drawClip = (trackIndex: number, item: Extract<TimelineItem, { kind: "clip" }>) => {
    const x = Math.round(item.track_start_s * pps + dx);
    const y = RULER_HEIGHT + trackIndex * laneHeight + 4;
    const w = Math.max(2, Math.round(item.duration_s * pps));
    const h = laneHeight - 8;
    ctx.save();
    ctx.setLineDash([5, 4]);
    ctx.strokeStyle = TOKEN.brandSecondary;
    ctx.lineWidth = 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, 4);
    ctx.restore();
  };

  if (drag.linkGroupId !== null) {
    for (let trackIndex = 0; trackIndex < snapshot.tracks.length; trackIndex += 1) {
      for (const item of snapshot.tracks[trackIndex].items) {
        if (item.kind === "clip" && item.link_group_id === drag.linkGroupId) {
          drawClip(trackIndex, item);
        }
      }
    }
    return;
  }

  const item = snapshot.tracks[drag.trackIndex]?.items.find(
    (candidate): candidate is Extract<TimelineItem, { kind: "clip" }> =>
      candidate.kind === "clip" && candidate.index === drag.clipIndex,
  );
  if (item) {
    drawClip(drag.trackIndex, item);
  }
}

/** Draw the time ruler with tick marks every 1, 5, or 10 seconds depending on zoom. */
export function drawRuler(
  ctx: CanvasRenderingContext2D,
  width: number,
  duration: number,
  pps: number,
) {
  // Slightly darker than the panel surface so the ruler reads as
  // dedicated chrome separate from the lane backgrounds below.
  ctx.fillStyle = TOKEN.surfacePanel;
  ctx.fillRect(0, 0, width, RULER_HEIGHT);
  ctx.strokeStyle = TOKEN.borderSubtle;
  ctx.beginPath();
  ctx.moveTo(0, RULER_HEIGHT - 0.5);
  ctx.lineTo(width, RULER_HEIGHT - 0.5);
  ctx.stroke();

  const desiredPx = 64;
  const candidates = [0.5, 1, 2, 5, 10, 30, 60, 120, 300];
  const majorInterval =
    candidates.find((candidate) => candidate * pps >= desiredPx) ??
    candidates[candidates.length - 1];
  // Minor interval: 1/5 of the major when wide enough to read; else
  // fall back to one tier finer from the candidates list. Skipped when
  // the minor would be sub-pixel.
  const majorIndex = candidates.indexOf(majorInterval);
  const minorInterval =
    majorIndex > 0 ? candidates[majorIndex - 1] : majorInterval / 5;
  const minorPxGap = minorInterval * pps;

  // Minor ticks first so the major ticks paint on top.
  if (minorPxGap >= 4) {
    ctx.strokeStyle = TOKEN.borderSubtle;
    ctx.beginPath();
    for (let t = 0; t <= duration + majorInterval; t += minorInterval) {
      const x = Math.round(t * pps) + 0.5;
      if (x > width) break;
      ctx.moveTo(x, RULER_HEIGHT - 4);
      ctx.lineTo(x, RULER_HEIGHT);
    }
    ctx.stroke();
  }

  ctx.fillStyle = TOKEN.textMuted;
  ctx.font = `11px ${FONT_MONO}`;
  ctx.textBaseline = "middle";

  for (let t = 0; t <= duration + majorInterval; t += majorInterval) {
    const x = Math.round(t * pps) + 0.5;
    if (x > width) break;
    ctx.strokeStyle = TOKEN.border;
    ctx.beginPath();
    ctx.moveTo(x, RULER_HEIGHT - 8);
    ctx.lineTo(x, RULER_HEIGHT);
    ctx.stroke();
    ctx.fillText(formatTime(t), x + 4, RULER_HEIGHT / 2 - 1);
  }
}

export function drawTracks(
  ctx: CanvasRenderingContext2D,
  width: number,
  tracks: { name: string; kind: string; role: string | null; items: TimelineItem[] }[],
  pps: number,
  opts: {
    deletedKeys?: Set<string>;
    highlightKeys?: Set<string>;
    selectedKey?: string;
  },
  laneHeight: number = LANE_HEIGHT,
) {
  // Selection-track index drives the cyan left-rail accent and a
  // brighter header pill so the user can see which lane owns the
  // currently-inspected clip even when scrolled away horizontally.
  const selectedTrackIndex = opts.selectedKey
    ? Number.parseInt(opts.selectedKey.split(":")[0] ?? "", 10)
    : Number.NaN;

  for (let row = 0; row < tracks.length; row++) {
    const track = tracks[row];
    const y = RULER_HEIGHT + row * laneHeight;
    const isTitlesRow = track.role === "titles";
    const isSelected = row === selectedTrackIndex;

    // Lane background — tonally close so the rows feel like a single
    // surface but still distinguishable.
    ctx.fillStyle = isTitlesRow
      ? TOKEN.laneTitle
      : track.kind === "audio"
        ? TOKEN.laneAudio
        : TOKEN.laneVideo;
    ctx.fillRect(0, y, width, laneHeight);
    // Lane separator — 1px subtle border between lanes.
    ctx.strokeStyle = TOKEN.borderSubtle;
    ctx.beginPath();
    ctx.moveTo(0, y + laneHeight - 0.5);
    ctx.lineTo(width, y + laneHeight - 0.5);
    ctx.stroke();

    for (const item of track.items) {
      const x = Math.round(item.track_start_s * pps);
      const w = Math.max(2, Math.round(item.duration_s * pps));
      const key = `${row}:${item.index}`;
      const flag =
        opts.deletedKeys?.has(key)
          ? "deleted"
          : opts.highlightKeys?.has(key)
            ? "highlight"
            : "normal";
      const selected = opts.selectedKey === key;
      drawItem(
        ctx,
        item,
        x,
        y + 4,
        w,
        laneHeight - 8,
        track.kind,
        flag,
        selected,
        isTitlesRow,
      );
    }

    drawTrackHeader(ctx, track, y, laneHeight, isSelected);
  }
}

/** Inline track-header pill drawn at the very left of every lane.
 *  Mirrors the V1 / A1 / T1 labels Premiere & Resolve use on their
 *  left rail; selected track gets a 2px cyan accent so the active row
 *  is unambiguous even across long timelines. */
function drawTrackHeader(
  ctx: CanvasRenderingContext2D,
  track: { name: string; kind: string; role: string | null },
  y: number,
  laneHeight: number,
  selected: boolean,
) {
  const headerWidth = TRACK_HEADER_WIDTH;
  // Slightly-elevated chip background so the label reads on top of the
  // lane tint without obscuring whatever clip sits underneath.
  ctx.fillStyle = TOKEN.surfaceCard;
  ctx.fillRect(0, y + 1, headerWidth, laneHeight - 2);
  // Right edge of the chip — separates it from clip content.
  ctx.strokeStyle = TOKEN.borderSubtle;
  ctx.beginPath();
  ctx.moveTo(headerWidth - 0.5, y + 1);
  ctx.lineTo(headerWidth - 0.5, y + laneHeight - 1);
  ctx.stroke();

  // Selection accent — 2px cyan rail at the left edge marks the lane
  // owning the inspected clip.
  if (selected) {
    ctx.fillStyle = TOKEN.brandSecondary;
    ctx.fillRect(0, y + 2, 2, laneHeight - 4);
  }

  const label = track.name || (track.kind === "audio" ? "A" : "V");
  ctx.font = `600 10px ${FONT_MONO}`;
  ctx.textBaseline = "middle";
  ctx.fillStyle = selected ? TOKEN.textPrimary : TOKEN.textMuted;
  ctx.fillText(label, 6, y + laneHeight / 2);
}

type ItemFlag = "normal" | "deleted" | "highlight";

function drawItem(
  ctx: CanvasRenderingContext2D,
  item: TimelineItem,
  x: number,
  y: number,
  w: number,
  h: number,
  trackKind: string,
  flag: ItemFlag,
  selected: boolean,
  isTitlesRow: boolean,
) {
  const radius = 4;
  if (item.kind === "clip") {
    const isTitleClip = isTitlesRow && item.title !== null && item.title !== undefined;
    // Card fill — Studio Pro spec calls for `--color-surface-card`
    // on every clip; the title/audio/video variants then layer their
    // overlay (text, waveform, filmstrip) on top.
    ctx.fillStyle = TOKEN.surfaceCard;
    fillRoundedRect(ctx, x, y, w, h, radius);
    let drewOverlay = false;
    if (isTitleClip) {
      drawClipTitleText(ctx, item.title!, x, y, w, h);
      drewOverlay = true;
    } else if (trackKind !== "audio" && item.thumbnail_dir && w > 24) {
      drewOverlay = drawClipFilmstrip(ctx, item, x, y, w, h, radius);
    } else if (trackKind === "audio" && item.waveform_path && w > 24) {
      drewOverlay = drawClipWaveform(ctx, item, x, y, w, h, radius, selected);
    }
    // Bottom-edge vignette — only over a filmstrip, so the white label
    // sitting in the middle of the card still has a dark strip behind
    // it instead of fighting the brightest mid-frame thumbnail.
    if (drewOverlay && trackKind !== "audio" && !isTitleClip) {
      drawClipBottomVignette(ctx, x, y, w, h, radius);
    }
    // Border — inactive clips use the subtle hairline; highlighted
    // (diff) clips lift to the cyan brand; deleted use danger red.
    // The selected ring is drawn separately *outside* the card so it
    // sits above the cyan/border edges below.
    const stroke =
      flag === "deleted"
        ? TOKEN.danger
        : flag === "highlight"
          ? TOKEN.brandSecondary
          : isTitleClip
            ? TOKEN.warning
            : TOKEN.borderSubtle;
    ctx.strokeStyle = stroke;
    ctx.lineWidth = flag === "normal" ? 1 : 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.lineWidth = 1;
    if (w > 24 && !isTitleClip) {
      // Top-left clip label — 10px SemiBold, near-white. Sits above
      // the vignette band so it never collides with bright thumbnails.
      ctx.font = `600 10px ${FONT_SANS}`;
      ctx.textBaseline = "top";
      const linkPad = item.link_group_id ? 10 : 0; // room for chain glyph
      const label = truncateToWidth(
        ctx,
        item.name,
        w - 2 * CLIP_PADDING_X - linkPad,
      );
      ctx.fillStyle = TOKEN.textPrimary;
      ctx.fillText(label, x + CLIP_PADDING_X + linkPad, y + 4);
      if (item.link_group_id) {
        drawLinkChainIcon(ctx, x + CLIP_PADDING_X, y + 5);
      }
    }
    if (w > 36 && !isTitleClip) {
      drawClipBadges(ctx, item, x, y, w);
    }
    if (flag === "deleted") {
      ctx.strokeStyle = TOKEN.danger;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x + 2, y + h / 2);
      ctx.lineTo(x + w - 2, y + h / 2);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
    if (selected) {
      // 2px cyan border + soft outer glow per Studio Pro spec. The
      // glow is faked by drawing a wider, semi-transparent stroke just
      // outside the card, then the crisp 2px ring on top.
      ctx.strokeStyle = "rgba(56, 189, 248, 0.18)";
      ctx.lineWidth = 6;
      strokeRoundedRect(ctx, x - 2.5, y - 2.5, w + 5, h + 5, radius + 3);
      ctx.strokeStyle = TOKEN.brandSecondary;
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
      ctx.lineWidth = 1;
    }
  } else if (item.kind === "gap") {
    ctx.fillStyle = "rgba(122, 122, 130, 0.10)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    ctx.strokeStyle = TOKEN.borderSubtle;
    ctx.setLineDash([3, 3]);
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.setLineDash([]);
  } else {
    // Transition card — purple per the brand convention (purple =
    // transitions / motion). Faint cross-fade gradient fills the
    // boundary, with a small ◇ glyph centered on cards too narrow to
    // hold the label.
    drawTransitionCard(ctx, item, x, y, w, h, radius);
    if (selected) {
      ctx.strokeStyle = TOKEN.brandSecondary;
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
      ctx.lineWidth = 1;
    }
  }
}

/** Tiny chain-link glyph (5x6 px) drawn in muted text colour to mark
 *  clips that share a `link_group_id` with a sibling video/audio clip
 *  imported from the same source. */
function drawLinkChainIcon(ctx: CanvasRenderingContext2D, x: number, y: number) {
  ctx.save();
  ctx.strokeStyle = TOKEN.textSecondary;
  ctx.lineWidth = 1;
  // Two interlocked ovals — 4x3 each, offset by ~2px on the X axis.
  ctx.beginPath();
  ctx.ellipse(x + 2, y + 3, 2, 1.5, 0, 0, Math.PI * 2);
  ctx.ellipse(x + 5, y + 3, 2, 1.5, 0, 0, Math.PI * 2);
  ctx.stroke();
  ctx.restore();
}

/** Bottom-of-card vignette so labels sitting over the brightest part
 *  of a thumbnail still read at full contrast. Only painted when an
 *  overlay (filmstrip / title) actually filled the card. */
function drawClipBottomVignette(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
) {
  ctx.save();
  pathRoundedRect(ctx, x, y, w, h, radius);
  ctx.clip();
  const gradient = ctx.createLinearGradient(0, y, 0, y + h);
  // Three-stop fade: top stays clear, midway begins darkening, the
  // bottom 30% holds full ~50% black so the label band is legible.
  gradient.addColorStop(0, "rgba(0, 0, 0, 0)");
  gradient.addColorStop(0.55, "rgba(0, 0, 0, 0)");
  gradient.addColorStop(1, "rgba(0, 0, 0, 0.45)");
  ctx.fillStyle = gradient;
  ctx.fillRect(x, y, w, h);
  // Top-edge vignette protects the clip label drawn in the top-left.
  const topGradient = ctx.createLinearGradient(0, y, 0, y + 18);
  topGradient.addColorStop(0, "rgba(0, 0, 0, 0.55)");
  topGradient.addColorStop(1, "rgba(0, 0, 0, 0)");
  ctx.fillStyle = topGradient;
  ctx.fillRect(x, y, w, 18);
  ctx.restore();
}

/** Cross-fade gradient card with purple border and a centered ◇ glyph
 *  on narrow widths. Wider cards fall back to the standard label. */
function drawTransitionCard(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "transition" }>,
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
) {
  // Two-stop horizontal gradient hints at the cross-fade direction.
  const gradient = ctx.createLinearGradient(x, y, x + w, y);
  gradient.addColorStop(0, "rgba(168, 85, 247, 0.05)");
  gradient.addColorStop(0.5, "rgba(168, 85, 247, 0.22)");
  gradient.addColorStop(1, "rgba(168, 85, 247, 0.05)");
  ctx.fillStyle = gradient;
  fillRoundedRect(ctx, x, y, w, h, radius);
  ctx.strokeStyle = TOKEN.brandPurple;
  strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
  if (w > 60) {
    ctx.font = `10px ${FONT_SANS}`;
    ctx.textBaseline = "middle";
    ctx.fillStyle = TOKEN.brandSecondaryHover;
    const label = truncateToWidth(
      ctx,
      `${transitionLabel(item.effect_name)} ${item.duration_s.toFixed(2)}s`,
      w - 2 * CLIP_PADDING_X,
    );
    ctx.fillText(label, x + CLIP_PADDING_X, y + h / 2);
  } else if (w > 10) {
    // Narrow card → centered ◇ diamond glyph.
    ctx.font = `12px ${FONT_SANS}`;
    ctx.textBaseline = "middle";
    ctx.textAlign = "center";
    ctx.fillStyle = TOKEN.brandPurple;
    ctx.fillText("◇", x + w / 2, y + h / 2);
    ctx.textAlign = "start";
  }
}

function drawClipFilmstrip(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "clip" }>,
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
): boolean {
  if (!item.thumbnail_dir) return false;
  const strip = getStrip(item.thumbnail_dir);
  if (!strip || strip.paths.length === 0) return false;

  const sourceStart = item.source_start_s ?? 0;
  const sourceEnd = sourceStart + item.duration_s;
  const tilesByWidth = Math.max(1, Math.round(w / 50));
  const tilesByDuration = Math.max(1, Math.floor(item.duration_s));
  const tileCount = Math.min(tilesByWidth, tilesByDuration);

  ctx.save();
  pathRoundedRect(ctx, x, y, w, h, radius);
  ctx.clip();

  let drewAny = false;
  const tileWidth = w / tileCount;
  for (let i = 0; i < tileCount; i++) {
    const sourceTime =
      sourceStart + (sourceEnd - sourceStart) * ((i + 0.5) / tileCount);
    const frameIndex = Math.min(
      strip.paths.length - 1,
      Math.max(0, Math.floor(sourceTime)),
    );
    const img = strip.images[frameIndex];
    if (!img) continue;
    const tx = x + i * tileWidth;
    ctx.drawImage(img, tx, y, tileWidth, h);
    drewAny = true;
  }
  ctx.restore();
  return drewAny;
}

function drawClipWaveform(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "clip" }>,
  x: number,
  y: number,
  w: number,
  h: number,
  radius: number,
  selected: boolean,
): boolean {
  if (!item.waveform_path) return false;
  const buckets = getBuckets(item.waveform_path);
  if (!buckets || buckets.length === 0) return false;

  const sourceStart = item.source_start_s ?? 0;
  const approxAssetEnd = sourceStart + item.duration_s;
  const approxAssetDuration = Math.max(1e-3, approxAssetEnd);
  const startFrac = sourceStart / approxAssetDuration;
  const endFrac = approxAssetEnd / approxAssetDuration;

  const startBucket = Math.max(0, Math.floor(buckets.length * startFrac));
  const endBucket = Math.min(buckets.length, Math.ceil(buckets.length * endFrac));
  if (endBucket <= startBucket) return false;

  ctx.save();
  pathRoundedRect(ctx, x, y, w, h, radius);
  ctx.clip();

  const centerY = y + h / 2;
  const ampMax = Math.max(1, h / 2 - 3);

  // Slate by default; lifts to green when the clip is selected so the
  // active waveform reads as "this is the audio you're inspecting"
  // without flooding the timeline with brand-green elsewhere.
  ctx.strokeStyle = selected
    ? "rgba(34, 197, 94, 0.92)" // --color-job-ready-dot
    : "rgba(100, 116, 139, 0.85)"; // --color-viz-audio
  ctx.lineWidth = 1;

  ctx.beginPath();
  for (let i = 0; i < w; i++) {
    const ampPx = waveformPeakForColumn(buckets, startBucket, endBucket, i, w) * ampMax;
    const colX = x + i + 0.5;
    if (i === 0) {
      ctx.moveTo(colX, centerY - ampPx);
    } else {
      ctx.lineTo(colX, centerY - ampPx);
    }
  }
  ctx.stroke();

  ctx.beginPath();
  for (let i = 0; i < w; i++) {
    const ampPx = waveformPeakForColumn(buckets, startBucket, endBucket, i, w) * ampMax;
    const colX = x + i + 0.5;
    if (i === 0) {
      ctx.moveTo(colX, centerY + ampPx);
    } else {
      ctx.lineTo(colX, centerY + ampPx);
    }
  }
  ctx.stroke();

  ctx.restore();
  return true;
}

function waveformPeakForColumn(
  buckets: ArrayLike<number>,
  startBucket: number,
  endBucket: number,
  column: number,
  width: number,
): number {
  const colStart = startBucket + ((endBucket - startBucket) * column) / width;
  const colEnd = startBucket + ((endBucket - startBucket) * (column + 1)) / width;
  const lo = Math.max(0, Math.floor(colStart));
  const hi = Math.min(buckets.length, Math.max(lo + 1, Math.ceil(colEnd)));
  let peak = 0;
  for (let i = lo; i < hi; i++) {
    if (buckets[i] > peak) peak = buckets[i];
  }
  return peak;
}

function drawClipTitleText(
  ctx: CanvasRenderingContext2D,
  styling: TitleStyling,
  x: number,
  y: number,
  w: number,
  h: number,
) {
  if (w < 8) return;
  ctx.font = `11px ${FONT_SANS}`;
  ctx.textBaseline = "middle";
  const label = truncateToWidth(ctx, styling.text, w - 2 * CLIP_PADDING_X);
  ctx.fillStyle = TOKEN.brandSecondaryHover;
  ctx.fillText(label, x + CLIP_PADDING_X, y + h / 2);
}

function drawClipBadges(
  ctx: CanvasRenderingContext2D,
  item: Extract<TimelineItem, { kind: "clip" }>,
  x: number,
  y: number,
  w: number,
) {
  const badges: string[] = [];
  if (
    item.volume !== null &&
    item.volume !== undefined &&
    Math.abs(item.volume - 1.0) > 1e-6
  ) {
    badges.push(`🔉 ${formatBadgeNumber(item.volume)}×`);
  }
  if (
    item.speed !== null &&
    item.speed !== undefined &&
    Math.abs(item.speed - 1.0) > 1e-6
  ) {
    badges.push(`⚡ ${formatBadgeNumber(item.speed)}×`);
  }
  if (badges.length === 0) return;
  ctx.font = `10px ${FONT_SANS}`;
  ctx.textBaseline = "top";
  let cursorX = x + w - 4;
  for (let i = badges.length - 1; i >= 0; i--) {
    const text = badges[i];
    const metrics = ctx.measureText(text);
    const padX = 4;
    const padY = 1;
    const boxW = metrics.width + padX * 2;
    const boxH = 14;
    const boxX = cursorX - boxW;
    const boxY = y + 3;
    if (boxX < x + 4) break;
    ctx.fillStyle = "rgba(0, 0, 0, 0.62)";
    ctx.fillRect(boxX, boxY, boxW, boxH);
    ctx.fillStyle = TOKEN.brandSecondaryHover;
    ctx.fillText(text, boxX + padX, boxY + padY);
    cursorX = boxX - 4;
  }
}

export function drawPlayhead(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  currentTime: number,
  pps: number,
) {
  const x = Math.round(currentTime * pps) + 0.5;
  if (x < 0 || x > width) return;
  // Soft cyan halo so even a thin 1.5px shaft reads across light
  // thumbnail frames — without it the line vanishes on white peaks.
  ctx.save();
  ctx.shadowColor = "rgba(56, 189, 248, 0.55)";
  ctx.shadowBlur = 6;
  ctx.strokeStyle = TOKEN.brandSecondary;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, height);
  ctx.stroke();
  ctx.restore();
  // Top-cap triangle / arrow — gives the playhead a draggable feel
  // and anchors the eye at the ruler row.
  ctx.fillStyle = TOKEN.brandSecondary;
  ctx.beginPath();
  ctx.moveTo(x - 5, 0);
  ctx.lineTo(x + 5, 0);
  ctx.lineTo(x, 7);
  ctx.closePath();
  ctx.fill();
  ctx.lineWidth = 1;
}
