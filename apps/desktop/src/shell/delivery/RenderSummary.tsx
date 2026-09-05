import type { ReactNode } from "react";
import { Inline } from "../../ui";

/** Shared KV row — label uppercase muted, value mono primary. */
export function KV({ label, value }: { label: string; value: string | ReactNode }) {
  return (
    <Inline justify="between" align="baseline">
      <span className="text-[var(--text-caption)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
        {label}
      </span>
      <span className="font-mono text-[var(--text-body-sm)] text-[var(--color-text-primary)]">{value}</span>
    </Inline>
  );
}
