// Per-asset filmstrip cache. Step 10.4.
//
// Each entry is keyed by the absolute `thumbnail_dir` we get on
// TimelineItem::Clip. Values hold:
//   - the list of absolute jpeg paths (one per source-second, at
//     density 1/sec — see crates/render/src/ffmpeg.rs::generate_thumbnails)
//   - one HTMLImageElement per path, lazily decoded on first request
//
// The canvas calls `getStrip(dir)` during paint and gets back a
// possibly-empty array of decoded HTMLImageElements (or `null` when
// the dir is still being listed). Frames that haven't decoded yet
// kick off async load + register a one-shot `onLoaded` callback so
// the canvas can re-paint when more frames are ready.

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

type StripEntry = {
  /** Absolute jpeg paths in numeric (frame-0001 → frame-NNNN) order. */
  paths: string[];
  /** Decoded images, parallel to `paths`. `null` until loaded. */
  images: Array<HTMLImageElement | null>;
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

function notifyDecoded() {
  for (const cb of onLoadedHooks) cb();
}

/** Returns the strip entry for `dir`, kicking off a list + async
 *  decode of all frames on first call. Returns `null` while the
 *  initial list is in flight. */
export function getStrip(dir: string): StripEntry | null {
  const cached = cache.get(dir);
  if (cached === "pending") return null;
  if (cached) return cached;

  // Mark pending and fire the list-then-decode pipeline.
  cache.set(dir, "pending");
  void (async () => {
    try {
      const paths = await invoke<string[]>("list_thumbnail_frames", { dir });
      const entry: StripEntry = {
        paths,
        images: paths.map(() => null),
      };
      cache.set(dir, entry);
      // Kick off async decode for every frame. We don't gate this on
      // viewport visibility — the typical strip is dozens of files
      // (one per second), and the browser handles concurrent decodes
      // fine. A 60-min source is 3600 frames at ~3KB each = ~10 MB,
      // still within the same order as one mp4 segment.
      paths.forEach((p, i) => {
        const img = new Image();
        img.onload = () => {
          entry.images[i] = img;
          notifyDecoded();
        };
        img.onerror = () => {
          // Ignore — frame just won't render. The fall-back
          // coloured rect is already visible.
        };
        img.src = convertFileSrc(p);
      });
      // Fire one immediate notify so the canvas re-paints with the
      // (still-empty) strip ready, in case all decodes are slow.
      notifyDecoded();
    } catch {
      // Failed list — wipe the pending sentinel so a future paint
      // can retry.
      cache.delete(dir);
    }
  })();
  return null;
}

/** Visible for tests + the rare "the dir got regenerated, drop
 *  cached frames" path (no current callers — included for parity). */
export function clearStripCache(): void {
  cache.clear();
}
