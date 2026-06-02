// Per-asset waveform cache. Step 11.3.
//
// Each entry is keyed by the absolute `waveform_path` we get on
// TimelineItem::Clip. Values hold the f32 bucket array plus the audio
// duration those buckets span, read from the JSON sidecar via the
// `read_waveform` Tauri command. The duration lets the timeline map a
// trimmed clip's source range onto the bucket array correctly.
//
// The canvas calls `getWaveform(path)` during paint and gets back the
// cached entry, kicking off an async fetch on the first call (returns
// `null` while the read is in flight). Once the read resolves,
// subscribers via `onWaveformDecoded` re-paint.
//
// Mirrors `thumbnailCache.ts` deliberately — same lifecycle, same
// "fire async, paint on resolve" rhythm.

import { invoke } from "@tauri-apps/api/core";

/** A decoded waveform: peak buckets plus the audio span they cover. */
export interface Waveform {
  buckets: Float32Array;
  /** Audio duration (seconds) the buckets span. `0` ⇒ unknown (legacy
   *  sidecar); callers fall back to an approximation. */
  durationS: number;
}

const cache = new Map<string, Waveform | "pending">();
const onLoadedHooks = new Set<() => void>();

/** Subscribe to "a new waveform finished loading." */
export function onWaveformDecoded(cb: () => void): () => void {
  onLoadedHooks.add(cb);
  return () => {
    onLoadedHooks.delete(cb);
  };
}

function notifyDecoded() {
  for (const cb of onLoadedHooks) cb();
}

/** Returns the decoded waveform for `path`, or null while the initial
 *  fetch is still in flight. The canvas calls this on every paint;
 *  the first call fires the async read, subsequent calls hit the
 *  cache. */
export function getWaveform(path: string): Waveform | null {
  const cached = cache.get(path);
  if (cached === "pending") return null;
  if (cached) return cached;

  cache.set(path, "pending");
  void (async () => {
    try {
      const data = await invoke<{ buckets: number[]; duration_s: number }>(
        "read_waveform",
        { path },
      );
      // Float32Array beats Array<number> for canvas drawing — tighter
      // memory, faster numeric iteration on V8.
      const wave: Waveform = {
        buckets: Float32Array.from(data.buckets),
        durationS: data.duration_s ?? 0,
      };
      cache.set(path, wave);
      notifyDecoded();
    } catch {
      // Failed read — drop the pending sentinel so a future paint
      // can retry (e.g. if the sidecar lands later).
      cache.delete(path);
    }
  })();
  return null;
}

/** Visible for tests. */
export function clearWaveformCache(): void {
  cache.clear();
}
