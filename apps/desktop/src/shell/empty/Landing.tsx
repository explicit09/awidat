import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import mark from "../../brand/montage-mark.svg";
import { useProjectStore } from "../../app/state";
import { MENU_COMMANDS, emitMenuCommand } from "../../app/menuCommands";

/**
 * Landing — full-workspace surface shown when no project is open.
 *
 * Replaces the prior two-card "No project open yet / What happens next /
 * System state" panel in `App.tsx#NoProjectWorkspace`. Rendered by the
 * branch in `App.tsx` that previously returned `<NoProjectWorkspace />`.
 *
 * State adaptation notes:
 * - The plan referenced `useRecentProjects` and `useProjectActions` hooks.
 *   Those don't exist; the canonical project state is `useProjectStore`
 *   (apps/desktop/src/app/state.ts) which exposes `recent: string[]`
 *   (absolute paths). We derive `{ name, path }` from each.
 * - The project lifecycle actions (New / Open / Import) are dispatched
 *   through the existing global menu-command bus (`MENU_COMMANDS` +
 *   `emitMenuCommand`) so the same handlers wired in `App.tsx` and
 *   `ProjectBanner.tsx` fire — no duplicated picker logic here.
 * - "Try example" has no real backend target yet, so it's a stub. See
 *   the TODO on its `onClick`.
 */
export function Landing() {
  const recentPaths = useProjectStore((s) => s.recent);
  const recents = recentPaths.map((p) => ({ name: basename(p), path: p }));

  const newProject = () => emitMenuCommand(MENU_COMMANDS.NEW_PROJECT);
  const importMedia = () => emitMenuCommand(MENU_COMMANDS.IMPORT_FILES);
  const openProject = () => emitMenuCommand(MENU_COMMANDS.OPEN_PROJECT);
  // TODO(redesign): wire to backend "open example" command when one exists.
  const tryExample = () => emitMenuCommand(MENU_COMMANDS.NEW_PROJECT);
  // Recents open the picked project directly via `set_project_root`;
  // useProjectStore.refresh() then pulls the new current/recent state
  // from the backend so the workspace mounts.
  const openRecent = async (path: string) => {
    try {
      await invoke("set_project_root", { path });
      await useProjectStore.getState().refresh();
    } catch (err) {
      console.warn("openRecent failed", err);
    }
  };

  // TODO(redesign): drop-zone treatment when drag-over. Tauri-level
  // drop wiring is out of scope for this task.

  return (
    <div
      className="relative z-10 flex flex-col items-center justify-center text-center px-6 py-12 h-full w-full min-h-0 bg-transparent"
    >
      <img
        src={mark}
        alt=""
        width={64}
        height={64}
        className="drop-shadow-[0_0_24px_rgba(255,122,24,0.30)] mb-3"
      />
      <div className="font-mono text-[13px] tracking-[0.16em] text-[var(--color-text-primary)] mb-5">
        MONTAGE
      </div>
      <h1 className="text-[16px] font-bold text-[var(--color-text-primary)] mb-1">
        Open a project to start editing
      </h1>
      <p className="text-[12px] text-[var(--color-text-muted)] max-w-[38ch] mb-5">
        Montage needs a project to index your media and run the agent. Drop a
        file anywhere in the window, or pick one of these.
      </p>
      <div className="flex flex-wrap gap-2 justify-center">
        <LandingBtn primary kbd="⌘N" onClick={newProject}>
          ＋ New project
        </LandingBtn>
        <LandingBtn kbd="⌘I" onClick={importMedia}>
          Import media
        </LandingBtn>
        <LandingBtn kbd="⌘O" onClick={openProject}>
          Open…
        </LandingBtn>
        <LandingBtn muted onClick={tryExample}>
          Try example
        </LandingBtn>
      </div>
      {recents.length > 0 && (
        <div className="mt-5 font-mono text-[10px] text-[var(--color-text-disabled)]">
          recent ·{" "}
          {recents.slice(0, 3).map((r, i) => (
            <span key={r.path}>
              <button
                onClick={() => openRecent(r.path)}
                className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              >
                {r.name}
              </button>
              {i < Math.min(recents.length, 3) - 1 && " · "}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * LandingBtn — local button styling for the landing surface.
 *
 * Defined inline (not via `ui/primitives/Button`) because these actions
 * have visually distinctive treatments — primary orange filled, secondary
 * outlined ghost, tertiary muted — and the keyboard-shortcut chip baked
 * into each variant. The shared `Button` doesn't model kbd hints.
 */
function LandingBtn({
  children,
  primary,
  muted,
  kbd,
  onClick,
}: {
  children: ReactNode;
  primary?: boolean;
  muted?: boolean;
  kbd?: string;
  onClick?: () => void;
}) {
  const base =
    "h-7 inline-flex items-center gap-2 px-3 rounded-md text-[11px] font-semibold";
  const variant = primary
    ? "border border-[var(--color-brand)] bg-[var(--color-brand)] text-[var(--color-text-inverse)] hover:bg-[var(--color-brand-hover)]"
    : muted
    ? "text-[var(--color-text-muted)]"
    : "border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--color-text-primary)] hover:bg-[var(--color-surface-card-hover)]";

  const kbdClass = primary
    ? "font-mono text-[9px] px-1 py-px rounded border border-[rgba(10,11,11,0.25)] text-[rgba(10,11,11,0.55)]"
    : "font-mono text-[9px] px-1 py-px rounded border border-[var(--color-border-subtle)] text-[var(--color-text-disabled)]";

  return (
    <button onClick={onClick} className={`${base} ${variant}`}>
      {children}
      {kbd && <span className={kbdClass}>{kbd}</span>}
    </button>
  );
}

/** Display name for a recent project — last path segment, OS-agnostic. */
function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
}
