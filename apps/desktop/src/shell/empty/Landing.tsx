import { useEffect, useState, type ReactNode } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { Clock3, FolderOpen, Import, MoreVertical, Plus, Search } from "lucide-react";
import mark from "../../brand/montage-mark.svg";
import { useProjectStore } from "../../app/state";
import { MENU_COMMANDS, emitMenuCommand } from "../../app/menuCommands";

type ProjectPreview = {
  kind: "image" | "video";
  src: string;
};

type ProjectPreviewResponse = {
  kind: ProjectPreview["kind"];
  path: string;
};

/**
 * Landing — project-manager surface shown when no project is open.
 * Actions reuse the existing menu command bus so picker logic stays in App.
 */
export function Landing() {
  const recentPaths = useProjectStore((s) => s.recent);
  const recents = recentPaths.map((p) => ({ name: basename(p), path: p }));
  const [previewByPath, setPreviewByPath] = useState<Record<string, ProjectPreview>>({});

  const newProject = () => emitMenuCommand(MENU_COMMANDS.NEW_PROJECT);
  const importMedia = () => emitMenuCommand(MENU_COMMANDS.IMPORT_FILES);
  const openProject = () => emitMenuCommand(MENU_COMMANDS.OPEN_PROJECT);

  useEffect(() => {
    let cancelled = false;
    async function loadThumbnails() {
      const entries = await Promise.all(
        recents.map(async (project) => {
          try {
            const preview = await invoke<ProjectPreviewResponse | null>("project_preview_media", {
              path: project.path,
            });
            return preview
              ? [project.path, { kind: preview.kind, src: convertFileSrc(preview.path) }] as const
              : null;
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

  const openRecent = async (path: string) => {
    try {
      await invoke("set_project_root", { path });
      await useProjectStore.getState().refresh();
    } catch (err) {
      console.warn("openRecent failed", err);
    }
  };

  return (
    <div
      data-testid="project-manager"
      className="pm-shell relative z-10 h-full w-full min-h-0 overflow-hidden text-[var(--color-text-primary)]"
    >
      <div
        className="absolute inset-x-0 top-0 z-20 h-12 border-b border-[rgba(255,255,255,0.07)]"
        data-tauri-drag-region
      />
      <div className="relative z-10 grid h-full grid-cols-[292px_minmax(0,1fr)] pt-12">
        <aside className="pm-glass pm-sidebar mx-4 mb-4 mt-4 px-5 py-5">
          <div className="mb-8 flex items-center gap-3" data-tauri-drag-region={false}>
            <img
              src={mark}
              alt=""
              width={34}
              height={34}
              className="drop-shadow-[0_0_18px_rgba(239,68,68,0.32)]"
            />
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
              <span>Search projects...</span>
            </label>
          </div>

          {recents.length > 0 ? (
            <div
              data-testid="recent-project-grid"
              className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-6"
            >
              {recents.map((project) => (
                <RecentProjectTile
                  key={project.path}
                  name={project.name}
                  path={project.path}
                  preview={previewByPath[project.path]}
                  onOpen={() => openRecent(project.path)}
                />
              ))}
            </div>
          ) : (
            <div
              data-testid="recent-project-grid"
              className="grid max-w-[620px] grid-cols-1 gap-4"
            >
              <div className="pm-glass rounded-xl border border-dashed border-[rgba(255,255,255,0.16)] p-6">
                <div className="mb-4 grid aspect-video place-items-center rounded-lg border border-[rgba(255,255,255,0.08)] bg-[#0B0B0B]">
                  <img src={mark} alt="" width={48} height={48} className="opacity-80" />
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
}: {
  name: string;
  path: string;
  preview?: ProjectPreview;
  onOpen: () => void;
}) {
  return (
    <button
      data-testid="recent-project-tile"
      onClick={onOpen}
      className="pm-project-card group text-left focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--color-brand)]"
    >
      <div className="pm-project-frame">
        <div className="relative flex h-full items-end justify-between bg-[radial-gradient(circle_at_24%_10%,rgba(239,68,68,0.10),transparent_34%),linear-gradient(135deg,#202020,#0A0A0A)] p-4">
          {preview?.kind === "video" ? (
            <video
              src={`${preview.src}#t=1`}
              className="absolute inset-0 h-full w-full object-cover"
              muted
              playsInline
              preload="metadata"
            />
          ) : preview?.kind === "image" ? (
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
          <span className="relative z-10 rounded bg-[rgba(0,0,0,0.38)] p-1 text-[var(--color-text-muted)] opacity-0 transition group-hover:opacity-100">
            <MoreVertical className="h-3.5 w-3.5" />
          </span>
        </div>
      </div>
      <div className="px-1 pb-1 pt-3">
        <div className="truncate text-[14px] font-semibold text-[var(--color-text-primary)]">
          {name}
        </div>
        <div className="mt-1 truncate text-[11px] text-[var(--color-text-muted)]">
          {path}
        </div>
      </div>
    </button>
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
