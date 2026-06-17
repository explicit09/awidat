// WebAudio gain stage for the preview monitor.
//
// HTMLMediaElement.volume clamps at 1.0, so clip gains above unity
// (the Inspector's volume slider goes to 4×) were inaudible in
// preview. Routing each <video> element through a GainNode removes
// the ceiling and gives the driver a place to apply audio fades.
//
// Rules:
// - One MediaElementSource per element, ever (creating a second one
//   throws) — tracked in a WeakMap.
// - The AudioContext starts suspended until a user gesture;
//   `resumePreviewAudio()` is called from the play toggle.
// - Any failure (no WebAudio, tainted media) permanently disables the
//   graph for the session and the caller falls back to the clamped
//   `element.volume` path — playback must never break over gain.

let ctx: AudioContext | null = null;
let dead = false;
const gains = new WeakMap<HTMLMediaElement, GainNode>();

/** Hard ceiling for preview gain — matches the Inspector's 4× slider
 *  range; protects ears and speakers from runaway values. */
export const PREVIEW_GAIN_MAX = 4;

/** Resume the context after a user gesture (WebKit autoplay policy
 *  keeps it suspended until then). Safe to call repeatedly. */
export function resumePreviewAudio(): void {
  if (ctx && ctx.state === "suspended") {
    void ctx.resume().catch(() => {});
  }
}

/** Route `v` through the gain graph and set its gain. Returns false
 *  when WebAudio is unavailable — caller falls back to
 *  `element.volume` (clamped at 1). */
export function setPreviewElementGain(
  v: HTMLMediaElement,
  gain: number,
): boolean {
  if (dead) return false;
  try {
    if (!ctx) {
      ctx = new AudioContext();
    }
    let node = gains.get(v);
    if (!node) {
      const source = ctx.createMediaElementSource(v);
      node = ctx.createGain();
      source.connect(node);
      node.connect(ctx.destination);
      gains.set(v, node);
    }
    const clamped = Number.isFinite(gain)
      ? Math.max(0, Math.min(PREVIEW_GAIN_MAX, gain))
      : 1;
    if (Math.abs(node.gain.value - clamped) > 0.001) {
      // Short ramp instead of a step — avoids zipper noise when the
      // driver updates gain every animation frame during fades.
      node.gain.setTargetAtTime(clamped, ctx.currentTime, 0.015);
    }
    return true;
  } catch (e) {
    // eslint-disable-next-line no-console
    console.warn("preview audio graph unavailable; falling back to element volume", e);
    dead = true;
    return false;
  }
}
