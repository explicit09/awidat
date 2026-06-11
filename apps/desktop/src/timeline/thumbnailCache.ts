// Per-asset filmstrip cache. Step 10.4.
//
// Each entry is keyed by the absolute `thumbnail_dir` we get on
// TimelineItem::Clip. Values hold:
//   - the list of absolute jpeg paths (one per source-second, at
//     density 1/sec — see crates/render/src/ffmpeg.rs::generate_thumbnails)
//   - one HTMLImageElement per path, decoded ON DEMAND
//
// The canvas calls `getStrip(dir)` during paint and gets back a
// possibly-empty array of decoded HTMLImageElements (or `null` when
// the dir is still being listed), then `ensureFrame(dir, i)` for the
// specific indices it actually draws. Decoding is strictly
// demand-driven: a long source has thousands of frames (an 86-minute
// episode ships 6,757 jpegs) and eagerly decoding them all floods the
// asset protocol at project open, starving the transcript/preview IPC
// while the timeline only ever paints ~width/50 tiles.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

type StripEntry = {
  /** Absolute jpeg paths in numeric (frame-0001 → frame-NNNN) order. */
  paths: string[];
  /** Decoded images, parallel to `paths`. `null` until loaded. */
  images: Array<HTMLImageElement | null>;
  /** Indices with a decode in flight or done — never re-request. */
  requested: Set<number>;
};

const cache = new Map<string, StripEntry | "pending">();
const onLoadedHooks = new Set<() => void>();

/** Subscribe to "a new frame just decoded somewhere." Called from
 *  the canvas paint effect so it can `paint()` again. */
export function onThumbnailDecoded(cb: () => void): () => void {
  onLoadedHooks.add(cb);
  return () => {
    onLoadedHooks.delete(cb);
  };
}

// Coalesce decode notifications to one repaint per frame. Without
// this, a burst of decodes (first paint of a long clip) schedules one
// full canvas repaint per jpeg.
let notifyScheduled = false;
function notifyDecoded() {
  if (notifyScheduled) return;
  notifyScheduled = true;
  requestAnimationFrame(() => {
    notifyScheduled = false;
    for (const cb of onLoadedHooks) cb();
  });
}

/** Returns the strip entry for `dir`, kicking off the path listing on
 *  first call. Returns `null` while the initial list is in flight.
 *  Frames are NOT decoded here — callers request the specific indices
 *  they paint via `ensureFrame`. */
export function getStrip(dir: string): StripEntry | null {
  const cached = cache.get(dir);
  if (cached === "pending") return null;
  if (cached) return cached;

  cache.set(dir, "pending");
  void (async () => {
    try {
      const paths = await invoke<string[]>("list_thumbnail_frames", { dir });
      cache.set(dir, {
        paths,
        images: paths.map(() => null),
        requested: new Set(),
      });
      // One notify so the canvas re-paints now that the strip exists
      // and can request the frames it actually needs.
      notifyDecoded();
    } catch {
      // Failed list — wipe the pending sentinel so a future paint
      // can retry.
      cache.delete(dir);
    }
  })();
  return null;
}

/** Start decoding frame `index` of `dir`'s strip if it hasn't been
 *  requested yet. Safe to call on every paint — repeat calls are
 *  no-ops. The canvas repaints via `onThumbnailDecoded` when ready. */
export function ensureFrame(dir: string, index: number): void {
  const entry = cache.get(dir);
  if (!entry || entry === "pending") return;
  if (index < 0 || index >= entry.paths.length) return;
  if (entry.requested.has(index)) return;
  entry.requested.add(index);
  const img = new Image();
  img.onload = () => {
    entry.images[index] = img;
    notifyDecoded();
  };
  img.onerror = () => {
    // Ignore — frame just won't render. The fall-back coloured rect
    // is already visible.
  };
  img.src = convertFileSrc(entry.paths[index]);
}

/** Visible for tests + the rare "the dir got regenerated, drop
 *  cached frames" path (no current callers — included for parity). */
export function clearStripCache(): void {
  cache.clear();
}
