import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Clock3, FolderOpen, Import, MoreVertical, Plus, Search } from "lucide-react";
import { BrandMark } from "../../brand/BrandMark";
import { useProjectStore } from "../../app/state";
import { MENU_COMMANDS, emitMenuCommand } from "../../app/menuCommands";
import {
  loadRecentProjectPreview,
  type RecentProjectPreview,
} from "./recentProjectPreview";

type PendingDelete = {
  name: string;
  path: string;
  size: number | null;
};

/**
 * Landing — project-manager surface shown when no project is open.
 * Actions reuse the existing menu command bus so picker logic stays in App.
 */
export function Landing() {
  const recentPaths = useProjectStore((s) => s.recent);
  const refreshProjects = useProjectStore((s) => s.refresh);
  const recents = recentPaths.map((p) => ({ name: basename(p), path: p }));
  const [previewByPath, setPreviewByPath] = useState<Record<string, RecentProjectPreview>>({});
  const [query, setQuery] = useState("");
  const [openMenuPath, setOpenMenuPath] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<PendingDelete | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const deleteRequestIdRef = useRef(0);
  const previousFocusRef = useRef<HTMLElement | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const deleteDialogRef = useRef<HTMLElement>(null);
  const cancelDeleteButtonRef = useRef<HTMLButtonElement>(null);
  const confirmDeleteButtonRef = useRef<HTMLButtonElement>(null);
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visibleRecents = normalizedQuery
    ? recents.filter((project) =>
        `${project.name}\n${project.path}`.toLocaleLowerCase().includes(normalizedQuery),
      )
    : recents;

  const newProject = () => emitMenuCommand(MENU_COMMANDS.NEW_PROJECT);
  const importMedia = () => emitMenuCommand(MENU_COMMANDS.IMPORT_FILES);
  const openProject = () => emitMenuCommand(MENU_COMMANDS.OPEN_PROJECT);

  useEffect(() => {
    let cancelled = false;
    async function loadThumbnails() {
      const entries = await Promise.all(
        recents.map(async (project) => {
          try {
            const preview = await loadRecentProjectPreview(project.path);
            if (!preview) return null;
            return [project.path, preview] as const;
          } catch (err) {
            console.warn("project_preview_media failed", err);
            return null;
          }
        }),
      );
      if (cancelled) return;
      setPreviewByPath(Object.fromEntries(entries.filter((entry) => entry !== null)));
    }

    void loadThumbnails();
    return () => {
      cancelled = true;
    };
  }, [recentPaths.join("\n")]);

  const pendingDeletePath = pendingDelete?.path;
  useEffect(() => {
    if (!pendingDeletePath) return;
    const frame = window.requestAnimationFrame(() => cancelDeleteButtonRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [pendingDeletePath]);

  useEffect(() => {
    if (!deleteBusy) return;
    const frame = window.requestAnimationFrame(() => deleteDialogRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [deleteBusy]);

  const openRecent = async (path: string) => {
    try {
      await invoke("set_project_root", { path });
      await useProjectStore.getState().refresh();
    } catch (err) {
      console.warn("openRecent failed", err);
    }
  };

  const removeRecent = async (path: string) => {
    setActionError(null);
    try {
      await invoke("remove_recent_project", { path });
      setOpenMenuPath(null);
      await refreshProjects();
    } catch (err) {
      setActionError(String(err));
    }
  };

  const requestDeleteProject = async (project: { name: string; path: string }) => {
    if (pendingDelete || deleteBusy) return;
    setActionError(null);
    setOpenMenuPath(null);
    previousFocusRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement
      : null;
    const requestId = ++deleteRequestIdRef.current;
    setPendingDelete({ ...project, size: null });
    try {
      const size = await invoke<number>("project_size_bytes", { path: project.path });
      if (deleteRequestIdRef.current !== requestId) return;
      setPendingDelete((current) =>
        current?.path === project.path ? { ...current, size } : current
      );
    } catch (err) {
      if (deleteRequestIdRef.current !== requestId) return;
      setActionError(String(err));
    }
  };

  const restoreDeleteFocus = () => {
    window.requestAnimationFrame(() => {
      const previous = previousFocusRef.current;
      if (previous?.isConnected) previous.focus();
      else searchInputRef.current?.focus();
    });
  };

  const cancelDeleteProject = () => {
    if (deleteBusy) return;
    deleteRequestIdRef.current += 1;
    setPendingDelete(null);
    setActionError(null);
    restoreDeleteFocus();
  };

  const confirmDeleteProject = async () => {
    if (!pendingDelete || pendingDelete.size === null || deleteBusy) return;
    const project = pendingDelete;
    setActionError(null);
    setDeleteBusy(true);
    try {
      await invoke("delete_project", {
        path: project.path,
        expectedBasename: project.name,
      });
    } catch (err) {
      setActionError(String(err));
      setDeleteBusy(false);
      return;
    }

    deleteRequestIdRef.current += 1;
    setPendingDelete(null);
    setDeleteBusy(false);
    restoreDeleteFocus();
    try {
      await refreshProjects();
    } catch (err) {
      setActionError(`Project was deleted, but recent projects could not refresh: ${String(err)}`);
    }
  };

  const handleDeleteDialogKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape" && !deleteBusy) {
      event.preventDefault();
      cancelDeleteProject();
      return;
    }
    if (event.key !== "Tab") return;
    const controls = [cancelDeleteButtonRef.current, confirmDeleteButtonRef.current]
      .filter((control): control is HTMLButtonElement => control !== null && !control.disabled);
    if (controls.length === 0) {
      event.preventDefault();
      deleteDialogRef.current?.focus();
      return;
    }
    const first = controls[0];
    const last = controls[controls.length - 1];
    const active = document.activeElement;
    if (!controls.includes(active as HTMLButtonElement)) {
      event.preventDefault();
      first.focus();
    } else if (controls.length === 1 || (event.shiftKey && active === first)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      data-testid="project-manager"
      className="pm-shell relative z-10 h-full w-full min-h-0 overflow-hidden text-[var(--color-text-primary)]"
    >
      <div
        inert={pendingDelete !== null}
        className="absolute inset-x-0 top-0 z-20 h-12 border-b border-[rgba(255,255,255,0.07)]"
        data-tauri-drag-region
      />
      <div
        inert={pendingDelete !== null}
        className="relative z-10 grid h-full grid-cols-[292px_minmax(0,1fr)] pt-12"
      >
        <aside className="pm-glass pm-sidebar mx-4 mb-4 mt-4 px-5 py-5">
          <div className="mb-8 flex items-center gap-3" data-tauri-drag-region={false}>
            <BrandMark size={34} className="rounded-[9px] drop-shadow-[0_0_18px_rgba(239,68,68,0.32)]" />
            <div>
              <div className="font-mono text-[11px] font-bold tracking-[0.18em] text-[var(--color-text-primary)]">
                MONTAGE
              </div>
              <div className="text-[11px] text-[var(--color-text-muted)]">
                Project Manager
              </div>
            </div>
          </div>

          <nav className="flex flex-col gap-2" data-tauri-drag-region={false}>
            <LandingBtn primary icon={<Plus className="h-4 w-4" />} kbd="⌘N" onClick={newProject}>
              New Project
            </LandingBtn>
            <LandingBtn icon={<FolderOpen className="h-4 w-4" />} kbd="⌘O" onClick={openProject}>
              Open Project
            </LandingBtn>
            <LandingBtn icon={<Import className="h-4 w-4" />} kbd="⌘I" onClick={importMedia}>
              Import Media
            </LandingBtn>
          </nav>

          <div className="mt-8 rounded-lg border border-[rgba(255,255,255,0.08)] bg-[rgba(255,255,255,0.035)] p-4">
            <div className="mb-2 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.12em] text-[var(--color-text-muted)]">
              <Clock3 className="h-3.5 w-3.5 text-[var(--color-brand)]" />
              Local Projects
            </div>
            <p className="text-[12px] leading-5 text-[var(--color-text-disabled)]">
              Recent tiles use frames already generated in each project cache.
            </p>
          </div>
        </aside>

        <main className="min-w-0 overflow-y-auto px-8 py-9" data-tauri-drag-region={false}>
          <div className="mb-8 flex items-start justify-between gap-4">
            <div>
              <div className="mb-2 font-mono text-[10px] uppercase tracking-[0.18em] text-[var(--color-brand)]">
                Continue editing
              </div>
              <h1 className="text-[26px] font-semibold text-[var(--color-text-primary)]">
                Recent Projects
              </h1>
              <p className="mt-2 max-w-[58ch] text-[13px] leading-5 text-[var(--color-text-muted)]">
                Continue from a local Montage project or open another folder.
              </p>
            </div>
            <label className="pm-glass flex h-9 w-[250px] shrink-0 items-center gap-2 rounded-lg px-3 text-[12px] text-[var(--color-text-disabled)]">
              <Search className="h-3.5 w-3.5" />
              <input
                ref={searchInputRef}
                aria-label="Search projects"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
                placeholder="Search projects..."
                className="min-w-0 flex-1 bg-transparent text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-disabled)]"
              />
            </label>
          </div>

          {actionError ? (
            <div role="alert" className="mb-4 rounded-lg border border-red-500/35 bg-red-500/10 px-3 py-2 text-[12px] text-red-200">
              {actionError}
            </div>
          ) : null}

          {visibleRecents.length > 0 ? (
            <div
              data-testid="recent-project-grid"
              className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-6"
            >
              {visibleRecents.map((project) => (
                <RecentProjectTile
                  key={project.path}
                  name={project.name}
                  path={project.path}
                  preview={previewByPath[project.path]}
                  onOpen={() => openRecent(project.path)}
                  menuOpen={openMenuPath === project.path}
                  onToggleMenu={() =>
                    setOpenMenuPath((current) => current === project.path ? null : project.path)
                  }
                  onRemove={() => void removeRecent(project.path)}
                  onDelete={() => void requestDeleteProject(project)}
                />
              ))}
            </div>
          ) : recents.length > 0 ? (
            <div className="pm-glass rounded-xl border border-dashed border-[rgba(255,255,255,0.16)] p-6 text-[13px] text-[var(--color-text-muted)]">
              No recent projects match “{query.trim()}”.
            </div>
          ) : (
            <div
              data-testid="recent-project-grid"
              className="grid max-w-[620px] grid-cols-1 gap-4"
            >
              <div className="pm-glass rounded-xl border border-dashed border-[rgba(255,255,255,0.16)] p-6">
                <div className="mb-4 grid aspect-video place-items-center rounded-lg border border-[rgba(255,255,255,0.08)] bg-[#0B0B0B]">
                  <BrandMark size={48} className="rounded-[12px] opacity-80" />
                </div>
                <h2 className="text-[14px] font-semibold">No recent projects yet</h2>
                <p className="mt-1 text-[12px] leading-5 text-[var(--color-text-muted)]">
                  Create a Montage project or open an existing local project to see it here next time.
                </p>
              </div>
            </div>
          )}
        </main>
      </div>
      {pendingDelete ? (
        <div className="absolute inset-0 z-50 grid place-items-center bg-black/70 px-4">
          <section
            ref={deleteDialogRef}
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-project-title"
            tabIndex={-1}
            onKeyDown={handleDeleteDialogKeyDown}
            className="w-full max-w-md rounded-xl border border-red-500/30 bg-[#171717] p-5 shadow-2xl"
          >
            <h2 id="delete-project-title" className="text-[16px] font-semibold text-white">
              Delete “{pendingDelete.name}” permanently?
            </h2>
            <p className="mt-3 break-all text-[12px] leading-5 text-[var(--color-text-muted)]">
              {pendingDelete.path}
            </p>
            <p className="mt-1 text-[12px] text-[var(--color-text-muted)]">
              {pendingDelete.size === null ? "Calculating size…" : formatBytes(pendingDelete.size)} · This cannot be undone.
            </p>
            {actionError ? (
              <p
                role="alert"
                data-testid="delete-project-error"
                className="mt-3 rounded border border-red-500/35 bg-red-500/10 px-3 py-2 text-[12px] text-red-200"
              >
                {actionError}
              </p>
            ) : null}
            <div className="mt-5 flex justify-end gap-2">
              <button
                ref={cancelDeleteButtonRef}
                type="button"
                disabled={deleteBusy}
                onClick={cancelDeleteProject}
                className="glass-ghost rounded px-3 py-2 text-[12px] disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                ref={confirmDeleteButtonRef}
                type="button"
                disabled={deleteBusy || pendingDelete.size === null}
                onClick={() => void confirmDeleteProject()}
                className="rounded bg-red-600 px-3 py-2 text-[12px] font-semibold text-white hover:bg-red-500 disabled:opacity-50"
              >
                {deleteBusy ? "Deleting…" : "Delete permanently"}
              </button>
            </div>
          </section>
        </div>
      ) : null}
    </div>
  );
}

function LandingBtn({
  children,
  icon,
  primary,
  kbd,
  onClick,
}: {
  children: ReactNode;
  icon: ReactNode;
  primary?: boolean;
  kbd?: string;
  onClick?: () => void;
}) {
  const base =
    "h-10 inline-flex w-full items-center gap-3 rounded-lg px-4 text-[13px] font-semibold transition";
  const variant = primary
    ? "border border-[rgba(255,255,255,0.82)] bg-white text-[#0D0D0D] shadow-[0_12px_34px_rgba(0,0,0,0.20)] hover:bg-[#F4F4F5]"
    : "pm-glass text-[var(--color-text-primary)] hover:border-[rgba(239,68,68,0.40)] hover:text-white";

  const kbdClass = primary
    ? "ml-auto font-mono text-[9px] rounded border border-[rgba(10,11,11,0.18)] px-1.5 py-px text-[rgba(10,11,11,0.56)]"
    : "ml-auto font-mono text-[9px] rounded border border-[rgba(255,255,255,0.10)] px-1.5 py-px text-[var(--color-text-disabled)]";

  return (
    <button onClick={onClick} className={`${base} ${variant}`}>
      {icon}
      {children}
      {kbd && <span className={kbdClass}>{kbd}</span>}
    </button>
  );
}

function RecentProjectTile({
  name,
  path,
  preview,
  onOpen,
  menuOpen,
  onToggleMenu,
  onRemove,
  onDelete,
}: {
  name: string;
  path: string;
  preview?: RecentProjectPreview;
  onOpen: () => void;
  menuOpen: boolean;
  onToggleMenu: () => void;
  onRemove: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      data-testid="recent-project-tile"
      tabIndex={0}
      onContextMenu={(event) => {
        event.preventDefault();
        if (!menuOpen) onToggleMenu();
      }}
      onKeyDown={(event) => {
        if (event.shiftKey && event.key === "F10") {
          event.preventDefault();
          if (!menuOpen) onToggleMenu();
        }
      }}
      className="pm-project-card group relative text-left focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-brand)]"
    >
      <button type="button" onClick={onOpen} className="block w-full text-left">
        <div className="pm-project-frame">
        <div className="relative flex h-full items-end justify-between bg-[radial-gradient(circle_at_24%_10%,rgba(239,68,68,0.10),transparent_34%),linear-gradient(135deg,#202020,#0A0A0A)] p-4">
          {preview ? (
            <img
              src={preview.src}
              alt=""
              className="absolute inset-0 h-full w-full object-cover"
              loading="lazy"
            />
          ) : (
            <span className="relative z-10 grid h-12 w-12 place-items-center rounded-lg bg-[rgba(255,255,255,0.08)] font-mono text-[14px] font-semibold text-white">
              {projectInitials(name)}
            </span>
          )}
          <span className="absolute inset-x-0 bottom-0 h-16 bg-gradient-to-t from-[rgba(0,0,0,0.72)] to-transparent" />
        </div>
        </div>
        <div className="px-1 pb-1 pt-3">
          <div className="truncate pr-8 text-[14px] font-semibold text-[var(--color-text-primary)]">
            {name}
          </div>
          <div className="mt-1 truncate text-[11px] text-[var(--color-text-muted)]">
            {path}
          </div>
        </div>
      </button>
      <button
        type="button"
        aria-label={`Project actions for ${name}`}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={(event) => {
          event.stopPropagation();
          onToggleMenu();
        }}
        className="absolute bottom-11 right-3 z-20 rounded bg-[rgba(0,0,0,0.38)] p-1.5 text-[var(--color-text-muted)] opacity-0 transition hover:text-white focus:opacity-100 group-hover:opacity-100"
      >
        <MoreVertical className="h-3.5 w-3.5" />
      </button>
      {menuOpen ? (
        <div
          role="menu"
          className="absolute bottom-10 right-2 z-30 w-48 rounded-lg border border-[rgba(255,255,255,0.12)] bg-[#171717] p-1.5 shadow-2xl"
        >
          <button type="button" role="menuitem" onClick={onRemove} className="block w-full rounded px-3 py-2 text-left text-[12px] text-[var(--color-text-primary)] hover:bg-white/10">
            Remove from Recents
          </button>
          <button type="button" role="menuitem" onClick={onDelete} className="block w-full rounded px-3 py-2 text-left text-[12px] text-red-300 hover:bg-red-500/15">
            Delete Project…
          </button>
        </div>
      ) : null}
    </div>
  );
}

/** Display name for a recent project — last path segment, OS-agnostic. */
function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const idx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return idx >= 0 ? trimmed.slice(idx + 1) : trimmed;
}

function projectInitials(name: string): string {
  const parts = name
    .split(/[\s._-]+/)
    .map((part) => part.trim())
    .filter(Boolean);
  const initials = parts.slice(0, 2).map((part) => part[0]?.toUpperCase()).join("");
  return initials || "M";
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** unit;
  return `${value >= 10 || unit === 0 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}
