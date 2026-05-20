// Hook that renders one GPU-blended frame for the active transition.
//
// Called from SegmentedVideoView when the playhead enters a
// transition window whose `transitionId` maps to a GPU shader CSS
// cannot fake (today: `awidat.match_dissolve` with its keyframed
// Blur curve; in the future: shake / chromatic_split / agent
// composites). Returns a `data:image/png;base64,…` URL the consumer
// drops into an `<img>` overlay.
//
// Two failure modes the hook handles itself:
//   - The transition has no GPU mapping. Backend returns `""`, we
//     return null, and SegmentedVideoView falls through to the CSS
//     overlay branch as before.
//   - Stale results during fast scrubbing. Each call gets an
//     incrementing request id; results whose id is no longer the
//     latest are ignored so a slow response doesn't briefly flash an
//     older frame over the timeline.
//
// Throttling: we only fire a fresh request when timelineTime moves
// by more than ~33ms (≈30fps), which is more than enough for preview
// since the IPC + extract + GPU + PNG round-trip is itself in the
// 50-150ms range on typical hardware.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { PreviewTransition } from "../timeline/usePlaySegments";

/** Set of awidat transition ids that should render via the GPU
 *  command instead of the CSS-based overlay. Kept in sync with
 *  `resolve_shader_for_id` in the Rust preview command — adding a
 *  named GPU preset to the registry means adding it here too. */
const GPU_ROUTED_TRANSITION_IDS = new Set<string>([
  // Phase 4 demonstrator: keyframed Blur curve renders correctly
  // only on the GPU; the CSS opacity-only blend loses the animation.
  "awidat.match_dissolve",
  // Luma-mask transitions — procedural reveals (clock, blinds,
  // checkerboard, spiral, burst, jaws) have no FFmpeg xfade
  // equivalent and no CSS approximation, so the only correct
  // preview is the GPU path.
  "awidat.clock_wipe",
  "awidat.venetian_blinds_h",
  "awidat.checkerboard_dissolve",
  "awidat.spiral_wipe",
  "awidat.burst_wipe",
  "awidat.jaws_wipe",
  // Hyperframes-port GPU shaders. Each has a distinct visual that
  // neither CSS overlays nor xfade can fake.
  "awidat.light_leak",
  "awidat.swirl_vortex",
  "awidat.cinematic_pan_left",
  "awidat.cinematic_pan_right",
]);

export function shouldRenderTransitionOnGpu(transition: PreviewTransition | null): boolean {
  if (!transition || !transition.transitionId) return false;
  return GPU_ROUTED_TRANSITION_IDS.has(transition.transitionId);
}

interface PreviewRequest {
  fromProxyPath: string;
  fromSourceS: number;
  toProxyPath: string;
  toSourceS: number;
  transitionId: string;
  progress: number;
  width: number;
  height: number;
}

/**
 * Returns a `data:image/png;base64,…` URL for the current preview
 * frame, or null when no GPU frame is available (no active
 * transition, no GPU mapping, or still loading the first frame).
 *
 * `width` and `height` come from the preview pane's actual pixel
 * size so we don't waste GPU cycles rendering at 4K when the pane
 * is 480px wide. Pass the pane's `clientWidth`/`clientHeight`.
 */
export function useGpuTransitionPreview(
  transition: PreviewTransition | null,
  timelineTime: number,
  width: number,
  height: number,
): string | null {
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const lastRequestedTimeRef = useRef<number>(Number.NEGATIVE_INFINITY);
  const requestIdRef = useRef<number>(0);
  const latestSettledRef = useRef<number>(0);

  useEffect(() => {
    if (!shouldRenderTransitionOnGpu(transition) || !transition) {
      // Clear any stale preview when leaving a GPU-routed window so
      // the underlying video doesn't show through a transparent PNG.
      if (dataUrl !== null) {
        setDataUrl(null);
      }
      lastRequestedTimeRef.current = Number.NEGATIVE_INFINITY;
      return;
    }
    if (width <= 0 || height <= 0) return;

    // Throttle: only re-fire when timeline-time moves by ~33ms+.
    const delta = Math.abs(timelineTime - lastRequestedTimeRef.current);
    if (delta < 0.033) return;
    lastRequestedTimeRef.current = timelineTime;

    const reqId = ++requestIdRef.current;
    const progress = transitionProgress(transition, timelineTime);
    const fromSourceS = clampToSegment(
      transition.from.sourceEnd - transition.inOffset + (timelineTime - transition.timelineStart),
      transition.from.sourceStart,
      transition.from.sourceEnd,
    );
    const toSourceS = clampToSegment(
      transition.to.sourceStart - transition.inOffset + (timelineTime - transition.timelineStart),
      transition.to.sourceStart,
      transition.to.sourceEnd,
    );
    const args: PreviewRequest = {
      fromProxyPath: transition.from.proxyPath,
      fromSourceS,
      toProxyPath: transition.to.proxyPath,
      toSourceS,
      transitionId: transition.transitionId ?? "",
      progress,
      width: Math.max(8, Math.round(width)),
      height: Math.max(8, Math.round(height)),
    };

    invoke<string>("render_transition_preview_frame", { args })
      .then((url) => {
        // Discard if a newer request has already settled.
        if (reqId < latestSettledRef.current) return;
        latestSettledRef.current = reqId;
        // Empty string is the backend's "no GPU mapping" signal —
        // we honor it by clearing rather than treating it as a load.
        setDataUrl(url || null);
      })
      .catch((err) => {
        // The frontend's responsibility on error is to fall back to
        // the CSS overlay rather than show a broken state. Log it
        // for now — the hook just stays at its last known data URL.
        // eslint-disable-next-line no-console
        console.warn("[gpu-preview] render failed", err);
      });
  }, [transition, timelineTime, width, height, dataUrl]);

  return dataUrl;
}

function transitionProgress(
  transition: PreviewTransition,
  timelineTime: number,
): number {
  if (transition.duration <= 0) return 0;
  const raw = (timelineTime - transition.timelineStart) / transition.duration;
  return Math.max(0, Math.min(1, raw));
}

function clampToSegment(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(max, value));
}
