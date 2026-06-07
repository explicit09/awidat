import type { TitleStyling } from "../protocol";
import { snapMoveDeltaS, type UserMoveDrag } from "./editMath.ts";
import { formatBadgeNumber, formatTime, transitionLabel } from "./formatting.ts";
import type { GhostRange } from "./ghostClipRanges.ts";
import { CLIP_PADDING_X, LANE_HEIGHT, RULER_HEIGHT, TRACK_HEADER_WIDTH, timeToX } from "./layout.ts";
import {
  fillRoundedRect,
  pathRoundedRect,
  strokeRoundedRect,
  truncateToWidth,
} from "./canvasPrimitives.ts";
import type { PendingProposal } from "./pendingProposals.ts";
import type { TimelineItem, TimelineSnapshot } from "./store.ts";
import { getStrip } from "./thumbnailCache.ts";
import { getWaveform } from "./waveformCache.ts";

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
  borderSubtle: "#1F1F1F",
  /** --color-border */
  border: "#2A2A2A",
  /** --color-text-primary */
  textPrimary: "#FFFFFF",
  /** --color-text-secondary */
  textSecondary: "#D1D5DB",
  /** --color-text-muted */
  textMuted: "#8F949E",
  /** --color-brand-secondary (red accents: selection, playhead, highlight) */
  brandSecondary: "#EF4444",
  /** --color-brand-secondary-hover (lighter red) */
  brandSecondaryHover: "#FCA5A5",
  /** --color-brand-purple (transitions, motion family) */
  brandPurple: "#FB7185",
  /** --color-viz-audio — slate-grey baseline for inactive audio */
  vizAudio: "#64748B",
  /** --color-job-ready-dot — green reserved for the active audio clip */
  jobReadyDot: "#22C55E",
  /** --color-danger / deleted state */
  danger: "#EF4444",
  /** --color-warning / title-clip border */
  warning: "#F59E0B",
} as const;

/// DaVinci-style per-track clip palette, tuned for the Obsidian Glass
/// shell: clip bodies are *translucent* (rgba alpha) so the frosted
/// stage reads through them, but each track type carries a distinct hue
/// the way Resolve colours video / audio / title clips. `accent` is the
/// 2px top stripe (Resolve's clip-colour bar); `body` is the fill.
const CLIP_STYLE = {
  /** Video clips — cool blue-grey, Resolve's default video colour. */
  video: { body: "rgba(56, 78, 104, 0.55)", accent: "rgba(125, 170, 222, 0.90)" },
  /** Audio clips — teal, matching the teal waveform envelope. */
  audio: { body: "rgba(28, 74, 70, 0.55)", accent: "rgba(45, 196, 170, 0.90)" },
  /** Title / generator clips — amber. */
  title: { body: "rgba(92, 68, 24, 0.55)", accent: "rgba(245, 179, 64, 0.90)" },
} as const;

/// Selection accent — DaVinci's signature red-orange clip outline,
/// replacing the old cyan ring. Kept with a soft outer glow so it still
/// reads on the translucent glass surfaces.
const SELECTION = {
  ring: "rgba(255, 96, 64, 0.95)",
  glow: "rgba(255, 96, 64, 0.20)",
} as const;

const FONT_SANS = '"Inter", ui-sans-serif, system-ui, sans-serif';
const FONT_MONO = '"JetBrains Mono", ui-monospace, SFMono-Regular, "SF Mono", Menlo, monospace';

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
    const x = Math.round(timeToX(item.track_start_s, pps) + dx);
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
  ctx.moveTo(TRACK_HEADER_WIDTH - 0.5, 0);
  ctx.lineTo(TRACK_HEADER_WIDTH - 0.5, RULER_HEIGHT);
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
      const x = Math.round(timeToX(t, pps)) + 0.5;
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
    const x = Math.round(timeToX(t, pps)) + 0.5;
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
    /**
     * One-sentence agent rationale to surface as a 9px italic muted
     * label below the clip name on highlighted (proposed-change)
     * clips. Empty / undefined → no extra label. Truncated to the
     * clip width; the full text still lives in the Inspector +
     * Brief row. Wave 3 B5 — lightweight surface, Wave 4 C2 ghost-
     * clip review will own the heavy layout work.
     */
    proposalRationale?: string;
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
      const x = Math.round(timeToX(item.track_start_s, pps));
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
        flag === "highlight" ? opts.proposalRationale : undefined,
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
  rationale?: string,
) {
  const radius = 4;
  if (item.kind === "clip") {
    const isTitleClip = isTitlesRow && item.title !== null && item.title !== undefined;
    // Card fill — DaVinci-style per-track clip colour (translucent so the
    // glass stage reads through), then the title/audio/video overlay
    // (text, waveform, filmstrip) layers on top.
    const clipStyle = isTitleClip
      ? CLIP_STYLE.title
      : trackKind === "audio"
        ? CLIP_STYLE.audio
        : CLIP_STYLE.video;
    ctx.fillStyle = clipStyle.body;
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
    // DaVinci clip-colour stripe — a 2px accent bar along the clip's top
    // edge, clipped to the rounded corners. The per-track hue that reads
    // even when the body is heavily translucent over the glass stage.
    if (w > 6) {
      ctx.save();
      pathRoundedRect(ctx, x, y, w, h, radius);
      ctx.clip();
      ctx.fillStyle = clipStyle.accent;
      ctx.fillRect(x, y, w, 2);
      ctx.restore();
    }
    // Border — inactive clips use the subtle hairline; highlighted
    // (diff) clips lift to the cyan brand; deleted use danger red.
    // The selected ring is drawn separately *outside* the card so it
    // sits above the border edges below.
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
      // Proposed-change rationale — 9px italic muted, drawn directly
      // below the clip name. Only painted when the rationale is non-
      // empty, the clip is wide enough to fit a useful first chunk
      // (~80px), and there's vertical room below the name + badges
      // strip (~h >= 28). Wave 4 ghost-clip review (task C2) will
      // upgrade this into a fuller treatment.
      if (rationale && w > 80 && h >= 28) {
        ctx.font = `italic 9px ${FONT_SANS}`;
        ctx.textBaseline = "top";
        const rationaleLabel = truncateToWidth(
          ctx,
          rationale,
          w - 2 * CLIP_PADDING_X,
        );
        ctx.fillStyle = TOKEN.textMuted;
        ctx.fillText(rationaleLabel, x + CLIP_PADDING_X, y + 16);
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
      // DaVinci's red-orange selection outline + soft outer glow. The
      // glow is a wider, semi-transparent stroke just outside the card,
      // then the crisp 2px ring on top — both glass-friendly.
      ctx.strokeStyle = SELECTION.glow;
      ctx.lineWidth = 6;
      strokeRoundedRect(ctx, x - 2.5, y - 2.5, w + 5, h + 5, radius + 3);
      ctx.strokeStyle = SELECTION.ring;
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
      // Same red-orange selection ring as clips, for a uniform
      // "this is selected" signal across item kinds.
      ctx.strokeStyle = SELECTION.ring;
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
  const wave = getWaveform(item.waveform_path);
  if (!wave || wave.buckets.length === 0) return false;
  const buckets = wave.buckets;

  // The bucket array spans the WHOLE asset [0, assetDuration]. Slice out
  // only the source span this clip uses. The clip plays source seconds
  // [sourceStart, sourceStart + sourceSpan), where sourceSpan is the
  // timeline duration scaled by playback speed (a 2x clip consumes 2s of
  // source per 1s on the timeline). assetDuration comes from the sidecar
  // (decoded sample count / sample rate); fall back to the old
  // approximation only when it's unknown (legacy sidecar).
  const sourceStart = item.source_start_s ?? 0;
  const speed = item.speed && item.speed > 0 ? item.speed : 1;
  const sourceSpan = item.duration_s * speed;
  const assetDuration =
    wave.durationS > 1e-3 ? wave.durationS : sourceStart + sourceSpan;
  const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
  const startFrac = clamp01(sourceStart / assetDuration);
  const endFrac = clamp01((sourceStart + sourceSpan) / assetDuration);

  const startBucket = Math.max(0, Math.floor(buckets.length * startFrac));
  const endBucket = Math.min(buckets.length, Math.ceil(buckets.length * endFrac));
  if (endBucket <= startBucket) return false;

  ctx.save();
  pathRoundedRect(ctx, x, y, w, h, radius);
  ctx.clip();

  const centerY = y + h / 2;
  const ampMax = Math.max(1, h / 2 - 3);

  // DaVinci-style audio: a bright teal envelope filled and mirrored about
  // the centre line (the "tape" look), brightening to cyan when selected.
  // Top edge left→right, bottom edge right→left, close, fill.
  ctx.fillStyle = selected
    ? "rgba(94, 234, 212, 0.95)" // cyan-300 — active/inspecting
    : "rgba(45, 196, 170, 0.80)"; // teal — DaVinci audio
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
  for (let i = w - 1; i >= 0; i--) {
    const ampPx = waveformPeakForColumn(buckets, startBucket, endBucket, i, w) * ampMax;
    const colX = x + i + 0.5;
    ctx.lineTo(colX, centerY + ampPx);
  }
  ctx.closePath();
  ctx.fill();

  // Faint centre baseline anchors quiet passages so silence still reads.
  ctx.strokeStyle = selected
    ? "rgba(94, 234, 212, 0.45)"
    : "rgba(45, 196, 170, 0.30)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(x, centerY + 0.5);
  ctx.lineTo(x + w, centerY + 0.5);
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

/**
 * Wave 3 C2 — inline ghost-clip review for cut-medium proposals.
 *
 * Painted as a final additive pass on top of the normal track render.
 * Each range becomes:
 *   - 1.5px dashed cyan border + 8% cyan fill — the "this region is
 *     pending review" affordance.
 *   - Title in the top-left, rationale at the bottom (when there's
 *     room) so the canvas tells you what + why without a tooltip.
 *   - Focused proposal: solid 2px border + outer glow (parallel
 *     language to the selected-clip ring).
 *
 * Vertical fan-out: when multiple proposals overlap on the same track,
 * each gets a 4px vertical offset so they're all visible — the caller
 * passes pre-fanned ranges, so this function only consumes y-offset
 * metadata.
 *
 * No buttons drawn here — the DOM overlay above the canvas owns the
 * Accept / Reject controls (easier hit-testing + accessibility).
 */
export function drawGhostClips(
  ctx: CanvasRenderingContext2D,
  ranges: ReadonlyArray<GhostRange>,
  proposalsById: Map<string, PendingProposal>,
  focusedId: string | null,
  pps: number,
  laneHeight: number,
) {
  if (ranges.length === 0) return;

  // Compute per-track stacks so overlapping ghosts fan out vertically.
  // Stable order — earlier ranges sit higher.
  const offsetForRange = computeFanOffsets(ranges);

  for (let i = 0; i < ranges.length; i++) {
    const range = ranges[i]!;
    const proposal = proposalsById.get(range.proposalId);
    if (!proposal) continue;
    const offsetY = offsetForRange[i] ?? 0;
    drawSingleGhost(
      ctx,
      range,
      proposal,
      focusedId === range.proposalId,
      pps,
      laneHeight,
      offsetY,
    );
  }
}

/** Greedy fan-out — every overlapping ghost on the same track gets a
 *  4px stride. Tracks fan independently. Returns one y-offset per
 *  range, parallel to the input array. */
function computeFanOffsets(ranges: ReadonlyArray<GhostRange>): number[] {
  const STRIDE_PX = 4;
  const offsets: number[] = new Array(ranges.length).fill(0);
  // Per-track running stacks of (rangeIndex, endS).
  const stacksByTrack = new Map<number, { idx: number; endS: number }[]>();
  for (let i = 0; i < ranges.length; i++) {
    const range = ranges[i]!;
    const stack = stacksByTrack.get(range.trackIndex) ?? [];
    // Drop entries that ended before this range starts — they no
    // longer collide so they don't push us down.
    const active = stack.filter((entry) => entry.endS > range.startS);
    offsets[i] = active.length * STRIDE_PX;
    active.push({ idx: i, endS: range.endS });
    stacksByTrack.set(range.trackIndex, active);
  }
  return offsets;
}

function drawSingleGhost(
  ctx: CanvasRenderingContext2D,
  range: GhostRange,
  proposal: PendingProposal,
  focused: boolean,
  pps: number,
  laneHeight: number,
  offsetY: number,
) {
  const x = Math.round(timeToX(range.startS, pps));
  const w = Math.max(8, Math.round((range.endS - range.startS) * pps));
  const y = RULER_HEIGHT + range.trackIndex * laneHeight + 4 + offsetY;
  const h = laneHeight - 8 - offsetY;
  if (h < 12) return; // ran out of vertical room
  const radius = 4;

  // 8% cyan fill — sits over the lane tint so the band reads as
  // "pending" without obliterating the underlying clip imagery.
  ctx.save();
  ctx.fillStyle = "rgba(56, 189, 248, 0.08)";
  fillRoundedRect(ctx, x, y, w, h, radius);

  if (focused) {
    // Outer glow + 2px solid ring. Matches the selected-clip language
    // so the focused-ghost reads as "the one ⌘↵ targets."
    ctx.strokeStyle = "rgba(56, 189, 248, 0.28)";
    ctx.lineWidth = 6;
    strokeRoundedRect(ctx, x - 2.5, y - 2.5, w + 5, h + 5, radius + 3);
    ctx.strokeStyle = TOKEN.brandSecondary;
    ctx.lineWidth = 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
  } else {
    // Default — dashed 1.5px border. The dash pattern signals
    // "proposal, not committed" without a separate label.
    ctx.setLineDash([5, 4]);
    ctx.strokeStyle = TOKEN.brandSecondary;
    ctx.lineWidth = 1.5;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.setLineDash([]);
  }
  ctx.lineWidth = 1;

  // Top-left title: 10px SemiBold in cyan-hover so it carries the
  // "proposal" tone the dashed border establishes.
  if (w > 60) {
    ctx.font = `600 10px ${FONT_SANS}`;
    ctx.textBaseline = "top";
    const titleText = proposal.summary && proposal.summary.trim().length > 0
      ? proposal.summary
      : "Proposed edit";
    const titleLabel = truncateToWidth(ctx, titleText, w - 2 * CLIP_PADDING_X);
    ctx.fillStyle = TOKEN.brandSecondaryHover;
    ctx.fillText(titleLabel, x + CLIP_PADDING_X, y + 4);
  }

  // Bottom-left rationale: 9px italic muted, truncated. Only painted
  // when there's vertical room below the title.
  if (proposal.rationale && w > 80 && h >= 30) {
    ctx.font = `italic 9px ${FONT_SANS}`;
    ctx.textBaseline = "bottom";
    const rationaleLabel = truncateToWidth(
      ctx,
      proposal.rationale,
      w - 2 * CLIP_PADDING_X,
    );
    ctx.fillStyle = TOKEN.textMuted;
    ctx.fillText(rationaleLabel, x + CLIP_PADDING_X, y + h - 4);
  }

  ctx.restore();
}

export function drawPlayhead(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  currentTime: number,
  pps: number,
) {
  const x = Math.round(timeToX(currentTime, pps)) + 0.5;
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
