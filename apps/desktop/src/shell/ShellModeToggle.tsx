import { useShellMode } from "../state/shellMode";

/**
 * ShellModeToggle — a small Stage|Cockpit segmented pill. Lives in both
 * shells' chrome so the build can be switched live; the choice persists.
 *
 * `tone="glass"` matches the Stage's floating chrome; `tone="flat"` matches
 * the cockpit's solid IdentityRow.
 */
export function ShellModeToggle({ tone = "glass" }: { tone?: "glass" | "flat" }) {
  const mode = useShellMode((s) => s.mode);
  const setMode = useShellMode((s) => s.setMode);
  const wrap =
    tone === "glass"
      ? "glass-ghost flex items-center gap-0.5 rounded-full p-0.5 text-[11px]"
      : "flex items-center gap-0.5 rounded-full border border-[var(--color-border)] bg-[var(--color-surface-input)] p-0.5 text-[11px]";
  return (
    <div className={wrap} title="Switch UI build">
      {(["stage", "cockpit"] as const).map((m) => {
        const on = mode === m;
        return (
          <button
            key={m}
            onClick={() => setMode(m)}
            className="rounded-full px-2.5 py-0.5 font-medium capitalize transition"
            style={{
              background: on ? "linear-gradient(180deg,#FB7185,#EF4444)" : "transparent",
              color: on ? "#1A0E04" : "var(--color-text-muted)",
            }}
          >
            {m}
          </button>
        );
      })}
    </div>
  );
}
