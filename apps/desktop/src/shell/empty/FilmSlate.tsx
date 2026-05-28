/**
 * FilmSlate — loading-state placeholder for the source review preview.
 *
 * Renders the classic "clapper-board" film slate metaphor in the viewer
 * stage while a source media file is still being prepared (proxy build,
 * first decoded frame, indexers warming up). Replaces the prior raw
 * black void in `PreviewSurface` so the user gets affordance + progress
 * instead of a dead rectangle.
 *
 * Visual notes:
 * - The card is intentionally ~60% width of the stage. The point is to
 *   *acknowledge* the missing preview, not to pretend to be one — a
 *   full-bleed loading shimmer would read as a broken player.
 * - The diagonal stripe background is the slate's "leader" texture. The
 *   progress bar pinned to the bottom edge mirrors a clapper closing.
 * - All text uses tokenized colors so the slate participates in any
 *   future theming pass.
 *
 * Props are all caller-supplied — this component holds no state. The
 * caller (`PreviewSurface`) decides what to feed in based on the media
 * model. Missing optional fields just collapse silently.
 */
interface FilmSlateProps {
  filename: string;
  resolution?: string;         // "1920×1080"
  codec?: string;              // "h264"
  audio?: string;              // "aac 48k"
  sizeBytes?: number;
  durationSec?: number;
  ready: number;               // 0..1
  status: string;              // "Building proxy…"
  detail: string;              // "5 of 9 indexers ready · transcript at 67%"
  eta?: string;                // "~2 min"
}

export function FilmSlate({
  filename,
  resolution,
  codec,
  audio,
  sizeBytes,
  durationSec,
  ready,
  status,
  detail,
  eta,
}: FilmSlateProps) {
  return (
    <div className="flex items-center justify-center w-full h-full bg-black">
      <div
        className="relative w-[60%] aspect-video border border-[var(--color-border-subtle)] rounded
                   flex flex-col justify-between p-3 text-[10px] text-[var(--color-text-muted)] font-mono
                   bg-[repeating-linear-gradient(45deg,#0F1112_0_6px,#0A0B0B_6px_12px)]"
      >
        <div className="flex justify-between">
          <span>{filename}</span>
          {resolution && (
            <span>
              {resolution}
              {codec ? ` · ${codec}` : ""}
            </span>
          )}
        </div>
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-1.5 font-sans">
          <div className="text-[14px] font-bold text-[var(--color-brand)]">{status}</div>
          <div className="text-[11px] text-[var(--color-text-secondary)]">{detail}</div>
          {eta && (
            <div className="text-[10px] text-[var(--color-text-muted)] font-mono">
              {eta} · runs locally
            </div>
          )}
        </div>
        <div className="flex justify-between">
          {sizeBytes !== undefined && durationSec !== undefined && (
            <span>
              {Math.round(sizeBytes / 1e6)} MB · {durationSec.toFixed(1)}s
            </span>
          )}
          {audio && <span>{audio}</span>}
        </div>
        <div className="absolute inset-x-0 bottom-0 h-[3px] bg-[var(--color-surface-input)] overflow-hidden">
          <div
            className="h-full bg-gradient-to-r from-[var(--color-brand)] to-[#FCA67A] transition-[width] duration-300"
            style={{ width: `${Math.max(0, Math.min(1, ready)) * 100}%` }}
          />
        </div>
      </div>
    </div>
  );
}
