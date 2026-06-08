import { Inline } from "../../ui";
import { MediaFramePlaceholder } from "../MediaFramePlaceholder";

/**
 * Center column bottom — Safe-area preview rectangle with the
 * legend embedded beneath it as a single block. The legend used to
 * be a separate tiny side-card; pulling it inside the same wrapper
 * makes "preview + safe areas + what those colors mean" a single
 * visual block, not three scattered ones.
 */
export function SafeAreaPreview() {
  const legend: Array<{ label: string; value: string; color: string }> = [
    { label: "YouTube 16:9", value: "1920×1080", color: "var(--color-brand-secondary)" },
    { label: "TikTok 9:16", value: "1080×1920", color: "var(--color-brand-purple)" },
    { label: "Caption safe", value: "mobile / TV", color: "var(--color-warning)" },
    { label: "Title safe", value: "1546×874", color: "var(--color-success)" },
  ];
  return (
    <section className="border-t border-[var(--color-border-subtle)] p-2">
      <div className="rounded-[var(--radius-md)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] overflow-hidden">
        <div className="relative aspect-[16/6] w-full overflow-hidden bg-black">
          <MediaFramePlaceholder tone="wide" className="absolute inset-0 opacity-55" />
          <div className="absolute inset-0 bg-black/30" />
          {/* YouTube 16:9 safe area */}
          <div className="absolute inset-[10%] border border-[rgba(239,68,68,0.55)]" />
          {/* TikTok 9:16 safe area (vertical strip) */}
          <div className="absolute inset-y-[6%] left-[36%] right-[36%] border border-[rgba(168,85,247,0.65)] bg-[rgba(168,85,247,0.08)]" />
          {/* Caption safe band */}
          <div className="absolute inset-x-[10%] bottom-[16%] h-10 border border-[rgba(245,158,11,0.7)] bg-[rgba(245,158,11,0.08)]" />
          <div className="absolute bottom-2 left-2 rounded-[var(--radius-xs)] border border-white/10 bg-black/65 px-2 py-1 text-[var(--text-caption)] text-white/75">
            Preview · safe areas
          </div>
        </div>
        <div className="border-t border-[var(--color-border-subtle)] px-3 py-2">
          <Inline gap="4" wrap="wrap" align="center" justify="between">
            {legend.map(({ label, value, color }) => (
              <Inline key={label} gap="2" align="center">
                <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: color }} />
                <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">
                  {label}
                </span>
                <span className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)]">
                  {value}
                </span>
              </Inline>
            ))}
          </Inline>
        </div>
      </div>
    </section>
  );
}
