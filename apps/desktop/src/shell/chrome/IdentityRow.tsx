import { Share2, Settings } from "lucide-react";
import mark from "../../brand/montage-icon.png";
import { useMode } from "../../state/mode";
import { useProjectStore } from "../../app/state";
import { useSettings } from "../../state/settings";

/**
 * IdentityRow — row 1 of the redesigned top chrome.
 *
 * Layout:
 *   [traffic-light gutter] [mark] MONTAGE · <project name> <project path>      [Pro|Creator] [Share] [Settings]
 *
 * The project store exposes only `current: string | null` (an absolute path),
 * so the project name is derived from the path's basename. When no project is
 * loaded, only the brand half on the left renders.
 *
 * `data-tauri-drag-region` makes the row a draggable window region; interactive
 * children opt out via `data-tauri-drag-region={false}`.
 */
export function IdentityRow() {
  const mode = useMode((s) => s.mode);
  const toggle = useMode((s) => s.toggle);
  const projectPath = useProjectStore((s) => s.current);
  const openSettings = useSettings((s) => s.open);
  const project = projectPath ? { name: basename(projectPath), path: projectPath } : null;

  return (
    <div
      data-tauri-drag-region
      className="flex items-center justify-between gap-3 h-9 pl-[88px] pr-3 select-none"
      // pl-[88px] reserves space for the macOS traffic lights at x:12 + ~70px gap
    >
      <div className="flex items-center gap-2 min-w-0" data-tauri-drag-region={false}>
        <img src={mark} alt="" width={18} height={18} />
        <span className="font-mono font-semibold tracking-[0.08em] text-[11px] text-[var(--color-text-primary)]">
          MONTAGE
        </span>
        {project && (
          <>
            <span className="text-[var(--color-text-disabled)]">·</span>
            <span className="font-semibold text-[13px] text-[var(--color-text-primary)] truncate">
              {project.name}
            </span>
            <span className="font-mono text-[11px] text-[var(--color-text-muted)] truncate">
              {project.path}
            </span>
          </>
        )}
      </div>
      <div className="flex items-center gap-2" data-tauri-drag-region={false}>
        <ModePill mode={mode} onToggle={toggle} />
        <IconBtn label="Share">
          <Share2 size={14} />
        </IconBtn>
        <IconBtn label="Settings" onClick={openSettings}>
          <Settings size={14} />
        </IconBtn>
      </div>
    </div>
  );
}

function ModePill({ mode, onToggle }: { mode: "pro" | "creator"; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className="inline-flex items-center gap-0.5 px-1 py-0.5 rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] text-[11px] font-semibold text-[var(--color-text-muted)]"
      aria-label={`Switch to ${mode === "pro" ? "Creator" : "Pro"} mode`}
    >
      <span className={pillSegment(mode === "pro")}>Pro</span>
      <span className={pillSegment(mode === "creator")}>Creator</span>
    </button>
  );
}

const pillSegment = (active: boolean) =>
  active
    ? "px-2 py-0.5 rounded-full bg-[var(--color-surface-selected)] text-[var(--color-text-primary)] shadow-[inset_0_0_0_1px_var(--color-border)]"
    : "px-2 py-0.5 rounded-full";

function IconBtn({
  children,
  label,
  onClick,
}: {
  children: React.ReactNode;
  label: string;
  onClick?: () => void;
}) {
  return (
    <button
      aria-label={label}
      onClick={onClick}
      className="grid place-items-center w-6 h-6 rounded border border-transparent text-[var(--color-text-muted)] hover:border-[var(--color-border-subtle)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]"
    >
      {children}
    </button>
  );
}

/** Derive a project's display name from its absolute path's basename. */
function basename(path: string): string {
  // Strip trailing slashes, then take the segment after the last separator.
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
}
