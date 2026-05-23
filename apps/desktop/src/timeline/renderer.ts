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
    ctx.strokeStyle = "#78b8ff";
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
  ctx.fillStyle = "#151711";
  ctx.fillRect(0, 0, width, RULER_HEIGHT);
  ctx.strokeStyle = "#30352d";
  ctx.beginPath();
  ctx.moveTo(0, RULER_HEIGHT - 0.5);
  ctx.lineTo(width, RULER_HEIGHT - 0.5);
  ctx.stroke();

  const desiredPx = 64;
  const candidates = [0.5, 1, 2, 5, 10, 30, 60, 120, 300];
  const interval =
    candidates.find((candidate) => candidate * pps >= desiredPx) ??
    candidates[candidates.length - 1];

  ctx.fillStyle = "#a49f91";
  ctx.font = "11px ui-monospace, SFMono-Regular, 'SF Mono', Menlo, monospace";
  ctx.textBaseline = "middle";

  for (let t = 0; t <= duration + interval; t += interval) {
    const x = Math.round(t * pps) + 0.5;
    if (x > width) break;
    ctx.strokeStyle = "#30352d";
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
  tracks: { kind: string; role: string | null; items: TimelineItem[] }[],
  pps: number,
  opts: {
    deletedKeys?: Set<string>;
    highlightKeys?: Set<string>;
    selectedKey?: string;
  },
  laneHeight: number = LANE_HEIGHT,
) {
  for (let row = 0; row < tracks.length; row++) {
    const track = tracks[row];
    const y = RULER_HEIGHT + row * laneHeight;
    const isTitlesRow = track.role === "titles";

    if (isTitlesRow) {
      ctx.fillStyle = "#070b10";
    } else if (track.kind === "audio") {
      ctx.fillStyle = "#0b100d";
    } else {
      ctx.fillStyle = "#0d0f0d";
    }
    ctx.fillRect(0, y, width, laneHeight);
    ctx.strokeStyle = "#30352d";
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
  }
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
    if (isTitleClip) {
      ctx.fillStyle = "#0a1622";
    } else {
      ctx.fillStyle = trackKind === "audio" ? "#1b4a39" : "#263b48";
    }
    fillRoundedRect(ctx, x, y, w, h, radius);
    let drewOverlay = false;
    if (isTitleClip) {
      drawClipTitleText(ctx, item.title!, x, y, w, h);
      drewOverlay = true;
    } else if (trackKind !== "audio" && item.thumbnail_dir && w > 24) {
      drewOverlay = drawClipFilmstrip(ctx, item, x, y, w, h, radius);
    } else if (trackKind === "audio" && item.waveform_path && w > 24) {
      drewOverlay = drawClipWaveform(ctx, item, x, y, w, h, radius);
    }
    const stroke =
      flag === "deleted"
        ? "#ef7168"
        : flag === "highlight"
          ? "#78b8ff"
          : isTitleClip
            ? "#e4ae52"
            : trackKind === "audio"
              ? "#71c587"
              : "#71b7a6";
    ctx.strokeStyle = stroke;
    ctx.lineWidth = flag === "normal" ? 1 : 2;
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.lineWidth = 1;
    if (w > 24 && !isTitleClip) {
      ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      const label = truncateToWidth(ctx, item.name, w - 2 * CLIP_PADDING_X);
      const labelY = y + h / 2;
      if (drewOverlay) {
        const metrics = ctx.measureText(label);
        ctx.fillStyle = "rgba(5, 6, 5, 0.74)";
        ctx.fillRect(
          x + CLIP_PADDING_X - 2,
          labelY - 7,
          Math.min(w - 2 * CLIP_PADDING_X + 4, metrics.width + 4),
          14,
        );
      }
      ctx.fillStyle = "#eee8d7";
      ctx.fillText(label, x + CLIP_PADDING_X, labelY);
    }
    if (w > 36 && !isTitleClip) {
      drawClipBadges(ctx, item, x, y, w);
    }
    if (flag === "deleted") {
      ctx.strokeStyle = "#ef7168";
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x + 2, y + h / 2);
      ctx.lineTo(x + w - 2, y + h / 2);
      ctx.stroke();
      ctx.lineWidth = 1;
    }
    if (selected) {
      ctx.strokeStyle = "#91d7ff";
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
      ctx.lineWidth = 1;
    }
  } else if (item.kind === "gap") {
    ctx.fillStyle = "rgba(164, 159, 145, 0.12)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    ctx.strokeStyle = "#30352d";
    ctx.setLineDash([3, 3]);
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    ctx.setLineDash([]);
  } else {
    ctx.fillStyle = "rgba(120, 184, 255, 0.18)";
    fillRoundedRect(ctx, x, y, w, h, radius);
    ctx.strokeStyle = "#78b8ff";
    strokeRoundedRect(ctx, x + 0.5, y + 0.5, w - 1, h - 1, radius);
    if (w > 30) {
      ctx.font = "10px ui-sans-serif, system-ui, sans-serif";
      ctx.textBaseline = "middle";
      ctx.fillStyle = "#c9e7ff";
      const label = truncateToWidth(
        ctx,
        `${transitionLabel(item.effect_name)} ${item.duration_s.toFixed(2)}s`,
        w - 2 * CLIP_PADDING_X,
      );
      ctx.fillText(label, x + CLIP_PADDING_X, y + h / 2);
    }
    if (selected) {
      ctx.strokeStyle = "#91d7ff";
      ctx.lineWidth = 2;
      strokeRoundedRect(ctx, x - 0.5, y - 0.5, w + 1, h + 1, radius + 1);
      ctx.lineWidth = 1;
    }
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

  ctx.beginPath();
  ctx.strokeStyle = "rgba(113, 197, 135, 0.86)";
  ctx.lineWidth = 1;
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
  ctx.font = "11px ui-sans-serif, system-ui, sans-serif";
  ctx.textBaseline = "middle";
  const label = truncateToWidth(ctx, styling.text, w - 2 * CLIP_PADDING_X);
  ctx.fillStyle = "#91d7ff";
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
  ctx.font = "10px ui-sans-serif, system-ui, sans-serif";
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
    ctx.fillStyle = "rgba(5, 6, 5, 0.78)";
    ctx.fillRect(boxX, boxY, boxW, boxH);
    ctx.fillStyle = "#91d7ff";
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
  ctx.strokeStyle = "#ef7168";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.moveTo(x, 0);
  ctx.lineTo(x, height);
  ctx.stroke();
  ctx.fillStyle = "#ef7168";
  ctx.beginPath();
  ctx.moveTo(x - 5, 0);
  ctx.lineTo(x + 5, 0);
  ctx.lineTo(x, 6);
  ctx.closePath();
  ctx.fill();
  ctx.lineWidth = 1;
}
