// ScopeDock — human-visible surface for the `color_scopes` evidence
// the agent uses to reason about exposure / channel balance / chroma.
//
// Renders four canvases from the JSON snapshot returned by the
// `get_color_scopes` Tauri command:
//
//   1. Luma waveform — luminance distribution per horizontal slice.
//   2. RGB parade   — R/G/B distributions side-by-side per slice.
//   3. Vectorscope  — Cb/Cr 2D density on a chroma plane.
//   4. Histograms   — luma + RGB stacked overlays as line plots.
//
// Pure canvas rendering: no WebGL, no third-party libs. Each scope
// reads the snapshot tensors directly and rasterises rectangles or
// polylines into a fixed-size canvas. Resolution is governed by the
// snapshot's `bins` field (server-side default: 64).
//
// The dock fetches scopes on demand — when the panel is open AND the
// selected proxy and playhead second have changed. Coarsening to the
// nearest second matches the `media/MediaPane` second-tick throttle
// and keeps ffmpeg invocations cheap.

import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useMediaStore } from "../media/store";

/// Mirrors `montage_core::montage_mcp::tools::color_scopes::ColorScopeSnapshot`.
export type RgbHistogram = {
  red: number[];
  green: number[];
  blue: number[];
};

export type RgbParade = {
  red: number[][];
  green: number[][];
  blue: number[][];
};

export type ColorScopeSnapshot = {
  width: number;
  height: number;
  bins: number;
  sample_count: number;
  luma_histogram: number[];
  rgb_histogram: RgbHistogram;
  waveform: number[][];
  rgb_parade: RgbParade;
  vectorscope: number[][];
};

/** Canvas pixel size for each scope. Picked to fit a ~25vw side
 *  panel without forcing the layout to scroll. */
const CANVAS_W = 256;
const CANVAS_H = 144;

export type ScopeDockProps = {
  /** Absolute disk path to the currently-loaded proxy (or asset). */
  proxyPath: string | null;
  /** Optional callback when the user dismisses the dock. */
  onClose?: () => void;
};

export function ScopeDock({ proxyPath, onClose }: ScopeDockProps) {
  const currentTime = useMediaStore((s) => s.currentTime);
  const [collapsed, setCollapsed] = useState(false);
  const [snapshot, setSnapshot] = useState<ColorScopeSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Quantise to whole seconds so we don't re-fetch on every
  // microscopic timeupdate. ffmpeg extraction is the costly bit.
  const tBucket = Math.max(0, Math.floor(currentTime));

  useEffect(() => {
    if (collapsed || proxyPath === null) {
      return;
    }
    let cancelled = false;
    setLoading(true);
    invoke<ColorScopeSnapshot>("get_color_scopes", {
      args: { path: proxyPath, t_s: tBucket },
    })
      .then((scopes) => {
        if (cancelled) return;
        setSnapshot(scopes);
        setError(null);
      })
      .catch((e) => {
        if (cancelled) return;
        const msg = typeof e === "string" ? e : (e?.message ?? String(e));
        setError(msg);
        setSnapshot(null);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [proxyPath, tBucket, collapsed]);

  if (proxyPath === null) {
    return null;
  }

  return (
    <section
      className={`scope-dock ${collapsed ? "scope-dock-collapsed" : ""}`}
      aria-label="Color scopes"
    >
      <header className="scope-dock-header">
        <button
          type="button"
          className="scope-dock-toggle"
          aria-expanded={!collapsed}
          onClick={() => setCollapsed((v) => !v)}
        >
          <span aria-hidden="true">{collapsed ? "+" : "-"}</span>
          <span>Scopes</span>
          {loading && <span className="scope-dock-status">refreshing…</span>}
          {!loading && snapshot && (
            <span className="scope-dock-status">
              {snapshot.width}x{snapshot.height} - t={tBucket}s
            </span>
          )}
        </button>
        {onClose && (
          <button
            type="button"
            className="scope-dock-close"
            aria-label="Close scope dock"
            onClick={onClose}
          >
            ×
          </button>
        )}
      </header>
      {!collapsed && (
        <div className="scope-dock-body">
          {error && <div className="scope-dock-error">{error}</div>}
          {!error && (
            <div className="scope-dock-grid">
              <ScopeCanvas label="Waveform (luma)" snapshot={snapshot} draw={drawWaveform} />
              <ScopeCanvas label="RGB parade" snapshot={snapshot} draw={drawRgbParade} />
              <ScopeCanvas label="Vectorscope" snapshot={snapshot} draw={drawVectorscope} />
              <ScopeCanvas label="Histograms" snapshot={snapshot} draw={drawHistograms} />
            </div>
          )}
        </div>
      )}
    </section>
  );
}

/// Per-scope canvas. Renders the supplied snapshot via `draw` when
/// available; otherwise paints an "empty" placeholder so the grid
/// keeps a stable footprint while the first fetch is in flight.
type DrawFn = (ctx: CanvasRenderingContext2D, snapshot: ColorScopeSnapshot) => void;

function ScopeCanvas({
  label,
  snapshot,
  draw,
}: {
  label: string;
  snapshot: ColorScopeSnapshot | null;
  draw: DrawFn;
}) {
  const ref = useRef<HTMLCanvasElement | null>(null);
  // Memoise the draw call so React doesn't redraw on parent re-render
  // when neither input has changed.
  const stableSnapshot = useMemo(() => snapshot, [snapshot]);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    clearBackground(ctx, canvas.width, canvas.height);
    if (stableSnapshot) {
      draw(ctx, stableSnapshot);
    } else {
      drawPlaceholder(ctx, canvas.width, canvas.height);
    }
  }, [stableSnapshot, draw]);

  return (
    <figure className="scope-figure">
      <canvas
        ref={ref}
        width={CANVAS_W}
        height={CANVAS_H}
        className="scope-canvas"
        aria-label={label}
      />
      <figcaption>{label}</figcaption>
    </figure>
  );
}

/// Clear-to-near-black. The dock sits in a dark workspace; pure black
/// reads as a hole in the layout, so we use a very dark grey.
function clearBackground(ctx: CanvasRenderingContext2D, w: number, h: number) {
  ctx.fillStyle = "#0a0c10";
  ctx.fillRect(0, 0, w, h);
}

function drawPlaceholder(ctx: CanvasRenderingContext2D, w: number, h: number) {
  ctx.fillStyle = "#1a1d24";
  ctx.fillRect(0, 0, w, h);
  ctx.fillStyle = "#4a5160";
  ctx.font = "11px system-ui, sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText("no data", w / 2, h / 2);
}

/// Render a single-channel 2D grid (cols=x, rows=intensity-inverted)
/// to the canvas, mapping density to alpha on the supplied colour.
function drawGrid(
  ctx: CanvasRenderingContext2D,
  grid: number[][],
  xOffset: number,
  cellW: number,
  fillStyle: string,
) {
  const bins = grid.length;
  if (bins === 0) return;
  const maxCount = peakInGrid(grid);
  if (maxCount === 0) return;
  const cellH = CANVAS_H / bins;
  for (let xi = 0; xi < bins; xi += 1) {
    const col = grid[xi];
    for (let yi = 0; yi < bins; yi += 1) {
      const v = col[yi];
      if (v === 0) continue;
      // Square-root scaling gives the long tail visibility without
      // washing out the bright clusters — same idea as vectorscope
      // gamma in colour-grading tools.
      const alpha = Math.min(1, Math.sqrt(v / maxCount));
      ctx.globalAlpha = alpha;
      ctx.fillStyle = fillStyle;
      ctx.fillRect(xOffset + xi * cellW, yi * cellH, cellW + 0.5, cellH + 0.5);
    }
  }
  ctx.globalAlpha = 1;
}

function peakInGrid(grid: number[][]): number {
  let m = 0;
  for (const col of grid) {
    for (const v of col) {
      if (v > m) m = v;
    }
  }
  return m;
}

function peakInArray(arr: number[]): number {
  let m = 0;
  for (const v of arr) {
    if (v > m) m = v;
  }
  return m;
}

function drawWaveform(ctx: CanvasRenderingContext2D, snap: ColorScopeSnapshot) {
  drawAxisLines(ctx);
  drawGrid(ctx, snap.waveform, 0, CANVAS_W / snap.bins, "#9ee0ff");
}

function drawRgbParade(ctx: CanvasRenderingContext2D, snap: ColorScopeSnapshot) {
  drawAxisLines(ctx);
  // Three side-by-side panels: R | G | B.
  const panelW = CANVAS_W / 3;
  const bins = snap.bins;
  const cellW = panelW / bins;
  drawGrid(ctx, snap.rgb_parade.red, 0, cellW, "#ff6b6b");
  drawGrid(ctx, snap.rgb_parade.green, panelW, cellW, "#6bff8d");
  drawGrid(ctx, snap.rgb_parade.blue, panelW * 2, cellW, "#6b8dff");
  // Light separators between panels.
  ctx.strokeStyle = "rgba(255,255,255,0.18)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(panelW, 0);
  ctx.lineTo(panelW, CANVAS_H);
  ctx.moveTo(panelW * 2, 0);
  ctx.lineTo(panelW * 2, CANVAS_H);
  ctx.stroke();
}

function drawVectorscope(ctx: CanvasRenderingContext2D, snap: ColorScopeSnapshot) {
  // Vectorscope is square — center it horizontally in our 256x144
  // canvas and use the full height as the diameter.
  const size = CANVAS_H;
  const ox = (CANVAS_W - size) / 2;
  // Subtle background ring so users perceive it as a circular scope
  // even when scatter is dense.
  ctx.strokeStyle = "rgba(255,255,255,0.12)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.arc(ox + size / 2, size / 2, size / 2 - 1, 0, Math.PI * 2);
  ctx.stroke();
  // Reticle.
  ctx.beginPath();
  ctx.moveTo(ox, size / 2);
  ctx.lineTo(ox + size, size / 2);
  ctx.moveTo(ox + size / 2, 0);
  ctx.lineTo(ox + size / 2, size);
  ctx.stroke();

  const bins = snap.vectorscope.length;
  if (bins === 0) return;
  const maxCount = peakInGrid(snap.vectorscope);
  if (maxCount === 0) return;
  const cell = size / bins;
  for (let yi = 0; yi < bins; yi += 1) {
    const row = snap.vectorscope[yi];
    for (let xi = 0; xi < bins; xi += 1) {
      const v = row[xi];
      if (v === 0) continue;
      const alpha = Math.min(1, Math.sqrt(v / maxCount));
      ctx.globalAlpha = alpha;
      // Vectorscope dots blend toward chroma yellow — visually
      // distinct from waveform's blue and parade's RGB.
      ctx.fillStyle = "#ffd66b";
      ctx.fillRect(ox + xi * cell, yi * cell, cell + 0.5, cell + 0.5);
    }
  }
  ctx.globalAlpha = 1;
}

function drawHistograms(ctx: CanvasRenderingContext2D, snap: ColorScopeSnapshot) {
  drawAxisLines(ctx);
  // Overlay four polylines: luma (white), R, G, B.
  // Normalise to the joint peak so the per-channel relationships are
  // preserved (otherwise a flat-blue frame would look identical to a
  // flat-red one).
  const peak = Math.max(
    peakInArray(snap.luma_histogram),
    peakInArray(snap.rgb_histogram.red),
    peakInArray(snap.rgb_histogram.green),
    peakInArray(snap.rgb_histogram.blue),
  );
  if (peak === 0) return;
  drawPolyline(ctx, snap.luma_histogram, peak, "#e8edf5", 1.5);
  drawPolyline(ctx, snap.rgb_histogram.red, peak, "#ff6b6b", 1.0);
  drawPolyline(ctx, snap.rgb_histogram.green, peak, "#6bff8d", 1.0);
  drawPolyline(ctx, snap.rgb_histogram.blue, peak, "#6b8dff", 1.0);
}

function drawPolyline(
  ctx: CanvasRenderingContext2D,
  series: number[],
  peak: number,
  stroke: string,
  width: number,
) {
  if (series.length === 0) return;
  ctx.strokeStyle = stroke;
  ctx.lineWidth = width;
  ctx.beginPath();
  const dx = CANVAS_W / (series.length - 1 || 1);
  for (let i = 0; i < series.length; i += 1) {
    const x = i * dx;
    const y = CANVAS_H - (series[i] / peak) * CANVAS_H;
    if (i === 0) {
      ctx.moveTo(x, y);
    } else {
      ctx.lineTo(x, y);
    }
  }
  ctx.stroke();
}

/// Faint mid-tone reticle — quarters horizontally and vertically so
/// readers can eyeball "this peak is around 50 IRE" without precision
/// labels. Keeps the dock light on annotation chrome.
function drawAxisLines(ctx: CanvasRenderingContext2D) {
  ctx.strokeStyle = "rgba(255,255,255,0.08)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let i = 1; i < 4; i += 1) {
    const y = (CANVAS_H / 4) * i;
    ctx.moveTo(0, y);
    ctx.lineTo(CANVAS_W, y);
  }
  for (let i = 1; i < 4; i += 1) {
    const x = (CANVAS_W / 4) * i;
    ctx.moveTo(x, 0);
    ctx.lineTo(x, CANVAS_H);
  }
  ctx.stroke();
}
