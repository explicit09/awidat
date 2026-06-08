import { AudioWaveform, Captions, Clapperboard, Mic2 } from "lucide-react";

import { cn } from "../ui";

type MediaFrameTone = "before" | "after" | "wide";

type MediaFramePlaceholderProps = {
  label?: string;
  tone?: MediaFrameTone;
  muted?: boolean;
  className?: string;
};

const toneStyles: Record<MediaFrameTone, { bg: string; accent: string; icon: typeof Clapperboard }> = {
  before: {
    bg: "bg-[linear-gradient(135deg,#172033_0%,#24344d_48%,#0e1420_100%)]",
    accent: "bg-[rgba(148,163,184,0.48)]",
    icon: Mic2,
  },
  after: {
    bg: "bg-[linear-gradient(135deg,#10261d_0%,#214335_52%,#0d1715_100%)]",
    accent: "bg-[rgba(34,197,94,0.48)]",
    icon: Captions,
  },
  wide: {
    bg: "bg-[linear-gradient(135deg,#111827_0%,#243244_45%,#171312_100%)]",
    accent: "bg-[rgba(239,68,68,0.44)]",
    icon: Clapperboard,
  },
};

export function MediaFramePlaceholder({
  label,
  tone = "wide",
  muted = false,
  className,
}: MediaFramePlaceholderProps) {
  const styles = toneStyles[tone];
  const Icon = styles.icon;

  return (
    <div
      className={cn(
        "relative h-full w-full overflow-hidden bg-black",
        styles.bg,
        muted && "opacity-80",
        className,
      )}
    >
      <div className="absolute inset-0 opacity-55">
        <div className="absolute left-[12%] top-[14%] h-[64%] w-[28%] rounded-[var(--radius-sm)] border border-white/12 bg-black/18" />
        <div className="absolute right-[12%] top-[18%] h-[56%] w-[24%] rounded-[var(--radius-sm)] border border-white/10 bg-black/20" />
        <div className="absolute inset-x-[9%] bottom-[14%] flex h-8 items-end gap-1.5">
          {Array.from({ length: 18 }, (_, index) => (
            <span
              key={index}
              className={cn("w-full rounded-t-[2px]", styles.accent)}
              style={{ height: `${28 + ((index * 17) % 58)}%` }}
            />
          ))}
        </div>
      </div>
      <div className="absolute inset-0 bg-[radial-gradient(circle_at_center,transparent_0%,rgba(0,0,0,0.24)_62%,rgba(0,0,0,0.64)_100%)]" />
      <div className="absolute left-4 top-4 grid h-9 w-9 place-items-center rounded-[var(--radius-sm)] border border-white/12 bg-black/36 text-white/82">
        <Icon className="h-4 w-4 stroke-[1.7]" />
      </div>
      <AudioWaveform className="absolute bottom-4 right-4 h-4 w-4 text-white/60" />
      {label ? (
        <div className="absolute bottom-4 left-4 rounded-[var(--radius-xs)] border border-white/10 bg-black/50 px-2 py-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-white/76">
          {label}
        </div>
      ) : null}
    </div>
  );
}
