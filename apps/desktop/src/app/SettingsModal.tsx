// SettingsModal - global Settings surface for the redesigned chrome.
//
// Settings is a dense operational surface, not a linear form. The shell uses
// the shared glass language with section navigation on the left and focused
// cards on the right so project, account, publishing, and workspace controls
// can breathe without becoming a long dumped modal.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useState, type ReactNode } from "react";
import { PublishingSettings } from "./PublishingSettings";
import { useProjectStore } from "./state";
import { useAgentsMdEditor } from "../state/agentsMdEditor";
import { useIntroState } from "../state/introState";
import { useMode } from "../state/mode";
import { useSettings } from "../state/settings";
import { useAuth } from "../state/auth";
import { useWelcome } from "../state/welcome";
import { WORKSPACE_SHORTCUTS } from "../state";
import type { IndexerConfigSnapshot } from "../shell";

const APP_VERSION = "0.1.0";
// `Montage Pro 1.2` is the placeholder label Wave 1 used in the footer.
// When the backend exposes a `get_agent_model` invoke we can replace
// this with the real value — see `TODO(redesign):` below.
const AGENT_MODEL = "Montage Pro 1.2";
const AGENT_CONTEXT = "local";
const AGENT_PROVIDER = "Codex bridge";

/** Static shortcut cheatsheet — mirrors the menu-command bindings the
 *  chrome already handles plus the new ⌘, opener wired in App.tsx. */
const SHORTCUTS: ReadonlyArray<{ keys: string; label: string }> = [
  { keys: "⌘N", label: "New project" },
  { keys: "⌘O", label: "Open project" },
  { keys: "⌘I", label: "Import files" },
  ...WORKSPACE_SHORTCUTS.map((shortcut) => ({
    keys: shortcut.keys,
    label: `Open ${shortcut.label}`,
  })),
  { keys: "⌘,", label: "Open settings" },
];

type SettingsSectionId =
  | "project"
  | "account"
  | "publishing"
  | "agent"
  | "workspace"
  | "indexers"
  | "about";

const SETTINGS_SECTIONS: ReadonlyArray<{ id: SettingsSectionId; label: string; detail: string }> = [
  { id: "project", label: "Project", detail: "Folder and brief" },
  { id: "account", label: "Account", detail: "OpenAI access" },
  { id: "publishing", label: "Publishing", detail: "Social targets" },
  { id: "agent", label: "Agent", detail: "Runtime info" },
  { id: "workspace", label: "Workspace", detail: "Mode and shortcuts" },
  { id: "indexers", label: "Indexers", detail: "Config paths" },
  { id: "about", label: "About", detail: "Version" },
];

export function SettingsModal() {
  const isOpen = useSettings((s) => s.isOpen);
  const close = useSettings((s) => s.close);
  const projectPath = useProjectStore((s) => s.current);
  const openAgentsMdEditor = useAgentsMdEditor((s) => s.open);
  const mode = useMode((s) => s.mode);
  const setMode = useMode((s) => s.setMode);
  const introducedCount = useIntroState((s) => s.introduced.size);
  const resetIntroState = useIntroState((s) => s.reset);
  const welcomeShown = useWelcome((s) => s.shown);
  const resetWelcome = useWelcome((s) => s.reset);
  const authStatus = useAuth((s) => s.status);
  const refreshAuth = useAuth((s) => s.refresh);
  const openAuth = useAuth((s) => s.open);
  const [activeSection, setActiveSection] = useState<SettingsSectionId>("project");
  const [actionError, setActionError] = useState<string | null>(null);

  // Lazy-load the indexer config so the modal stays cheap when closed.
  // Calls the same `read_indexer_config` invoke used by App.tsx.
  const [indexerConfig, setIndexerConfig] = useState<IndexerConfigSnapshot | undefined>(undefined);

  useEffect(() => {
    if (!isOpen || !isTauri()) {
      return;
    }
    let cancelled = false;
    invoke<IndexerConfigSnapshot>("read_indexer_config")
      .then((snapshot) => {
        if (!cancelled) setIndexerConfig(snapshot);
      })
      .catch((e) => {
        console.warn("read_indexer_config failed", e);
      });
    // Refresh the OpenAI auth status so the Account section shows live state.
    void refreshAuth();
    return () => {
      cancelled = true;
    };
  }, [isOpen, refreshAuth]);

  // Esc closes — registered at document level so it works regardless of
  // which child has focus. Only mounts when the modal is open so we
  // don't trap key events when it's not.
  useEffect(() => {
    if (!isOpen) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [isOpen, close]);

  if (!isOpen) return null;

  async function openFolder(path: string) {
    setActionError(null);
    if (!isTauri()) {
      setActionError("Folder actions are only available in the desktop app.");
      return;
    }
    try {
      await openPath(path);
    } catch (e) {
      try {
        await revealItemInDir(path);
      } catch {
        setActionError(String(e));
      }
    }
  }

  async function revealFolder(path: string) {
    setActionError(null);
    if (!isTauri()) {
      setActionError("Finder actions are only available in the desktop app.");
      return;
    }
    try {
      await revealItemInDir(path);
    } catch (e) {
      setActionError(String(e));
    }
  }

  function editAgentsMd() {
    if (!projectPath) return;
    openAgentsMdEditor();
  }

  function showWelcomeAgain() {
    // Same rationale as editAgentsMd — close Settings before opening
    // another modal so the Esc / backdrop semantics stay sane.
    close();
    resetWelcome();
  }

  function manageSignIn() {
    openAuth();
  }

  const activeTitle = SETTINGS_SECTIONS.find((section) => section.id === activeSection)?.label ?? "Settings";

  function renderActiveSection() {
    switch (activeSection) {
      case "project":
        return (
          <>
            <SettingsCard title="Project folder" description="Local project paths and folder actions.">
              <SettingsRow label="Path" mono value={projectPath ?? "No project loaded"}>
                {projectPath ? (
                  <>
                    <GlassButton onClick={() => void openFolder(projectPath)}>Open folder</GlassButton>
                    <GlassButton variant="ghost" onClick={() => void revealFolder(projectPath)}>
                      Reveal in Finder
                    </GlassButton>
                  </>
                ) : null}
              </SettingsRow>
              <SettingsRow label="Editorial brief" value="AGENTS.md">
                <GlassButton
                  onClick={editAgentsMd}
                  disabled={!projectPath}
                  title={projectPath ? "Open the in-product editor" : "Load a project first"}
                >
                  Edit AGENTS.md
                </GlassButton>
              </SettingsRow>
              {actionError ? <SettingsError message={actionError} /> : null}
            </SettingsCard>
          </>
        );
      case "account":
        return (
          <SettingsCard title="OpenAI account" description="Choose what powers the editing agent.">
            <SettingsRow
              label="Powered by"
              value={authStatus ? authStatus.walletTitle : "Not signed in"}
            >
              <GlassButton onClick={manageSignIn}>Manage sign-in</GlassButton>
            </SettingsRow>
            {authStatus?.accountHint ? (
              <SettingsRow label="Account" mono value={authStatus.accountHint} />
            ) : null}
            {authStatus?.viaEnv ? (
              <SettingsRow
                label="Environment"
                value={`${authStatus.envVar ?? "Credential"} is overriding stored auth.`}
              />
            ) : null}
          </SettingsCard>
        );
      case "publishing":
        return (
          <SettingsCard title="Publishing" description="Connect accounts and choose default upload behavior.">
            <PublishingSettings />
          </SettingsCard>
        );
      case "agent":
        return (
          <SettingsCard title="Agent runtime" description="Current local editing bridge metadata.">
            <SettingsRow label="Model" value={AGENT_MODEL} />
            <SettingsRow label="Context window" mono value={AGENT_CONTEXT} />
            <SettingsRow label="Provider" value={AGENT_PROVIDER} />
          </SettingsCard>
        );
      case "workspace":
        return (
          <>
            <SettingsCard title="Workspace" description="Editor mode and first-run surfaces.">
              <SettingsRow label="Mode">
                <ModePill mode={mode} onChange={setMode} />
              </SettingsRow>
              <SettingsRow label="Theme" value="Dark" />
              <SettingsRow
                label="Project intro"
                value={
                  introducedCount === 0
                    ? "Off. Next open will introduce."
                    : `${introducedCount} project${introducedCount === 1 ? "" : "s"} introduced`
                }
              >
                <GlassButton
                  variant="ghost"
                  onClick={resetIntroState}
                  disabled={introducedCount === 0}
                  title="Clear the set of projects that already received the agent intro turn"
                >
                  Reset
                </GlassButton>
              </SettingsRow>
              <SettingsRow
                label="Welcome"
                value={
                  welcomeShown
                    ? "Dismissed. Click Show to reopen."
                    : "Pending on next launch"
                }
              >
                <GlassButton variant="ghost" onClick={showWelcomeAgain}>
                  Show
                </GlassButton>
              </SettingsRow>
            </SettingsCard>
            <SettingsCard title="Keyboard shortcuts" description="Common app commands.">
              <div className="grid gap-1.5">
                {SHORTCUTS.map((shortcut) => (
                  <div
                    key={shortcut.keys}
                    className="flex items-center justify-between gap-4 rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] px-3 py-2"
                  >
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
                      {shortcut.label}
                    </span>
                    <kbd className="rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.05)] px-2 py-1 font-mono text-[10px] text-[var(--color-text-primary)]">
                      {shortcut.keys}
                    </kbd>
                  </div>
                ))}
              </div>
            </SettingsCard>
          </>
        );
      case "indexers":
        return (
          <SettingsCard title="Indexer config" description="Open the config files that control local indexing.">
            <SettingsRow
              label="Project config"
              mono
              value={indexerConfig?.projectPath ?? "Unavailable"}
            >
              {indexerConfig?.projectPath ? (
                <GlassButton variant="ghost" onClick={() => void openFolder(indexerConfig.projectPath!)}>
                  Open
                </GlassButton>
              ) : null}
            </SettingsRow>
            <SettingsRow
              label="Global config"
              mono
              value={indexerConfig?.globalPath ?? "Unavailable"}
            >
              {indexerConfig?.globalPath ? (
                <GlassButton variant="ghost" onClick={() => void openFolder(indexerConfig.globalPath!)}>
                  Open
                </GlassButton>
              ) : null}
            </SettingsRow>
          </SettingsCard>
        );
      case "about":
        return (
          <SettingsCard title="About Montage" description="Desktop build details.">
            <SettingsRow label="Version" mono value={APP_VERSION} />
            <SettingsRow label="Montage" value="Studio for agent-driven editing" />
          </SettingsCard>
        );
    }
  }

  return (
    <div
      className="modal-backdrop"
      onClick={close}
      role="presentation"
    >
      <div
        className="settings-shell glass glass-strong flex overflow-hidden text-[var(--color-text-primary)]"
        onClick={(event) => event.stopPropagation()}
        style={{
          width: "min(880px, calc(100vw - 48px))",
          height: "min(620px, calc(100vh - 48px))",
          borderRadius: 16,
          boxShadow: "0 28px 90px rgba(0,0,0,0.62), 0 0 0 1px rgba(239,68,68,0.12)",
        }}
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
      >
        <aside className="settings-sidebar flex w-[218px] shrink-0 flex-col border-r border-[var(--glass-border)] bg-[rgba(8,9,12,0.44)]">
          <header className="flex items-center justify-between border-b border-[var(--glass-border)] px-4 py-4">
            <div>
              <h2 className="m-0 text-[17px] font-bold tracking-normal">Settings</h2>
              <p className="m-0 mt-1 text-[11px] text-[var(--color-text-muted)]">
                Montage desktop
              </p>
            </div>
          </header>
          <nav className="flex min-h-0 flex-1 flex-col gap-1 overflow-auto p-2.5" aria-label="Settings sections">
            {SETTINGS_SECTIONS.map((section) => (
              <button
                key={section.id}
                type="button"
                className={
                  activeSection === section.id
                    ? "glass-cta flex flex-col items-start rounded-lg px-3 py-2 text-left"
                    : "glass-ghost flex flex-col items-start rounded-lg px-3 py-2 text-left text-[var(--color-text-secondary)]"
                }
                onClick={() => setActiveSection(section.id)}
                aria-current={activeSection === section.id ? "page" : undefined}
              >
                <span className="text-[13px] font-semibold">{section.label}</span>
                <span className="text-[10px] opacity-70">{section.detail}</span>
              </button>
            ))}
          </nav>
        </aside>

        <section className="settings-content flex min-w-0 flex-1 flex-col bg-[rgba(10,10,14,0.24)]">
          <header className="flex items-center justify-between border-b border-[var(--glass-border)] px-5 py-4">
            <div className="min-w-0">
              <p className="m-0 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--color-brand)]">
                {activeTitle}
              </p>
              <h3 className="m-0 mt-1 truncate text-[20px] font-bold tracking-normal">
                Settings
              </h3>
            </div>
            <button
              type="button"
              className="glass-content grid h-8 w-8 place-items-center rounded-lg text-[18px] leading-none text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
              onClick={close}
              aria-label="Close settings"
            >
              ×
            </button>
          </header>
          <div className="min-h-0 flex-1 overflow-auto p-5">
            <div className="mx-auto flex max-w-[620px] flex-col gap-4">
              {renderActiveSection()}
            </div>
          </div>
          <footer className="flex justify-end border-t border-[var(--glass-border)] px-5 py-3">
            <GlassButton variant="primary" onClick={close}>
              Close
            </GlassButton>
          </footer>
        </section>
      </div>
    </div>
  );
}

function SettingsCard({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section className="glass-content rounded-xl p-4">
      <header className="mb-3">
        <h4 className="m-0 text-[15px] font-bold tracking-normal text-[var(--color-text-primary)]">
          {title}
        </h4>
        {description ? (
          <p className="m-0 mt-1 text-[12px] leading-snug text-[var(--color-text-muted)]">
            {description}
          </p>
        ) : null}
      </header>
      <div className="grid gap-2">{children}</div>
    </section>
  );
}

function SettingsRow({
  label,
  value,
  mono,
  children,
}: {
  label: string;
  value?: string;
  mono?: boolean;
  children?: React.ReactNode;
}) {
  return (
    <div className="grid min-h-[42px] grid-cols-[148px_minmax(0,1fr)] items-center gap-3 rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] px-3 py-2">
      <span className="text-[12px] font-medium text-[var(--color-text-secondary)]">{label}</span>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
        {value !== undefined ? (
          <span
            className={
              mono
                ? "min-w-0 truncate font-mono text-[11px] text-[var(--color-text-primary)]"
                : "min-w-0 truncate text-[13px] text-[var(--color-text-primary)]"
            }
            title={value}
          >
            {value}
          </span>
        ) : null}
        {children}
      </div>
    </div>
  );
}

function SettingsError({ message }: { message: string }) {
  return (
    <div
      role="alert"
      className="rounded-lg border border-[rgba(239,68,68,0.34)] bg-[rgba(239,68,68,0.10)] px-3 py-2 text-[12px] text-[var(--color-text-danger,#f87171)]"
    >
      {message}
    </div>
  );
}

function GlassButton({
  children,
  onClick,
  disabled,
  title,
  variant = "secondary",
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  title?: string;
  variant?: "primary" | "secondary" | "ghost";
}) {
  const className =
    variant === "primary" || variant === "secondary"
      ? "glass-cta rounded-lg px-3 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
      : "glass-ghost rounded-lg px-3 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45";
  return (
    <button type="button" className={className} onClick={onClick} disabled={disabled} title={title}>
      {children}
    </button>
  );
}

/** Mirrors the Pro/Creator pill from `IdentityRow` so toggling here
 *  feels visually identical to the chrome control. */
function ModePill({
  mode,
  onChange,
}: {
  mode: "pro" | "creator";
  onChange: (mode: "pro" | "creator") => void;
}) {
  return (
    <div
      role="tablist"
      aria-label="Workspace mode"
      className="glass-content inline-flex items-center gap-1 rounded-lg px-1 py-1 text-[11px] font-semibold text-[var(--color-text-muted)]"
    >
      <ModePillSegment label="Pro" active={mode === "pro"} onClick={() => onChange("pro")} />
      <ModePillSegment label="Creator" active={mode === "creator"} onClick={() => onChange("creator")} />
    </div>
  );
}

function ModePillSegment({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={
        active
          ? "glass-cta rounded-md px-2.5 py-1 text-[var(--color-text-primary)]"
          : "rounded-md px-2.5 py-1 hover:text-[var(--color-text-primary)]"
      }
    >
      {label}
    </button>
  );
}
