// Top-of-window project bar. Shows the active project path and a
// "Change" affordance that opens a popover with: native folder picker,
// "New project…" button, and a recents list.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Trash2 } from "lucide-react";
import { useProjectStore } from "./state";
import { NewProjectForm } from "./NewProjectForm";
import { DeleteProjectConfirm } from "./DeleteProjectConfirm";
import { ManageProjectsDialog } from "./ManageProjectsDialog";
import type { ProjectType } from "../protocol";
import { MENU_COMMANDS, onMenuCommand } from "./menuCommands";

type Props = {
  /** Called whenever the active project changes (open/new/clear). */
  onChange: (path: string | null) => void;
};

export function ProjectBanner({ onChange }: Props) {
  const current = useProjectStore((s) => s.current);
  const recent = useProjectStore((s) => s.recent);
  const refresh = useProjectStore((s) => s.refresh);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showNew, setShowNew] = useState(false);
  const [showManage, setShowManage] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [projectType, setProjectType] = useState<ProjectType | null>(null);
  const popRef = useRef<HTMLDivElement | null>(null);

  // Reload project type whenever the active project changes — get_
  // project_type returns Other{description:""} when no project is
  // loaded, which we render as no-badge.
  useEffect(() => {
    if (!current) {
      setProjectType(null);
      return;
    }
    invoke<ProjectType>("get_project_type")
      .then((pt) => setProjectType(pt))
      .catch(() => setProjectType(null));
  }, [current]);

  // Initial load + close popover on outside click.
  useEffect(() => {
    refresh().catch((e) => setError(String(e)));
    function onClick(e: MouseEvent) {
      if (popRef.current && !popRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [refresh]);

  async function pickAndOpen() {
    setError(null);
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Open Awidat project",
      });
      if (typeof picked === "string") {
        await invoke("set_project_root", { path: picked });
        await refresh();
        onChange(picked);
        setOpen(false);
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function openRecent(path: string) {
    setError(null);
    try {
      await invoke("set_project_root", { path });
      await refresh();
      onChange(path);
      setOpen(false);
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    return onMenuCommand((id) => {
      if (id === MENU_COMMANDS.NEW_PROJECT) {
        setShowNew(true);
        setOpen(false);
      } else if (id === MENU_COMMANDS.OPEN_PROJECT) {
        void pickAndOpen();
      } else if (id === MENU_COMMANDS.OPEN_RECENT) {
        setOpen(true);
      }
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="project-banner" ref={popRef}>
      <span className="project-label">Project</span>
      <div className="project-path-group">
        <strong className="project-name">{current ? basename(current) : "No project"}</strong>
        <code className="project-path">
          {current ?? "Open or create a Montage project"}
        </code>
      </div>
      {projectType && current && (
        <span className="project-type-badge" title={projectTypeTitle(projectType)}>
          {projectTypeLabel(projectType)}
        </span>
      )}
      <button className="project-switch-button" onClick={() => setOpen((v) => !v)}>
        {current ? "Switch" : "Open"}
      </button>
      {open && (
        <div className="project-popover" role="menu">
          <button className="popover-action" onClick={pickAndOpen}>
            <strong>Open existing project…</strong>
            <span className="popover-hint">pick a folder</span>
          </button>
          <button
            className="popover-action"
            onClick={() => {
              setShowNew(true);
              setOpen(false);
            }}
          >
            <strong>New project…</strong>
            <span className="popover-hint">init in a chosen folder</span>
          </button>
          {recent.length > 0 && (
            <>
              <div className="popover-divider">Recent</div>
              {recent.map((p) => (
                <div key={p} className="popover-recent-row">
                  <button
                    className="popover-recent"
                    onClick={() => openRecent(p)}
                    title={p}
                  >
                    <span className="recent-name">{basename(p)}</span>
                    <span className="recent-path">{p}</span>
                  </button>
                  <button
                    className="popover-recent-delete"
                    onClick={() => setPendingDelete(p)}
                    title={`Delete "${basename(p)}" permanently`}
                    aria-label={`Delete ${basename(p)} permanently`}
                  >
                    <Trash2 size={14} strokeWidth={1.75} />
                  </button>
                </div>
              ))}
              <button
                className="popover-action popover-manage"
                onClick={() => {
                  setShowManage(true);
                  setOpen(false);
                }}
              >
                <strong>Manage projects…</strong>
                <span className="popover-hint">delete from disk</span>
              </button>
            </>
          )}
          {error && <div className="popover-error">{error}</div>}
        </div>
      )}
      {showNew && (
        <NewProjectForm
          onClose={() => setShowNew(false)}
          onCreated={(path) => {
            setShowNew(false);
            onChange(path);
            refresh().catch(() => {});
          }}
        />
      )}
      {pendingDelete && (
        <DeleteProjectConfirm
          path={pendingDelete}
          isActive={pendingDelete === current}
          onCancel={() => setPendingDelete(null)}
          onDeleted={() => {
            const wasActive = pendingDelete === current;
            setPendingDelete(null);
            refresh().catch(() => {});
            if (wasActive) onChange(null);
          }}
        />
      )}
      {showManage && (
        <ManageProjectsDialog
          onClose={() => setShowManage(false)}
          onDeletedActive={() => onChange(null)}
        />
      )}
    </div>
  );
}

function basename(p: string): string {
  const i = p.lastIndexOf("/");
  return i === -1 ? p : p.slice(i + 1);
}

/** Short label for the banner badge. */
function projectTypeLabel(pt: ProjectType): string {
  switch (pt.kind) {
    case "podcast":
      return "podcast";
    case "shorts":
      return "shorts";
    case "tutorial":
      return "tutorial";
    case "other":
      return "custom";
  }
}

/** Hover-title carrying the long form (description for "other"). */
function projectTypeTitle(pt: ProjectType): string {
  switch (pt.kind) {
    case "podcast":
      return "Podcast — long-form cleanup defaults";
    case "shorts":
      return "Shorts — vertical, fast-cut defaults";
    case "tutorial":
      return "Tutorial — hold key frames, never cut over code";
    case "other":
      return pt.description ? `Custom: ${pt.description}` : "Custom (no description)";
  }
}
