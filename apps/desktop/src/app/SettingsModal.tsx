// SettingsModal — global Settings surface for the redesigned chrome.
//
// Wave 1 removed the Model / Context-window readouts from the footer; the
// design intent was that those move into Settings. This modal is opened
// from the ⚙ icon in `IdentityRow` and via the ⌘, keyboard shortcut
// (wired in App.tsx). It is intentionally read-only for now — the Tauri
// backend doesn't expose live editing for model / context / provider
// yet, and the design intent is "show me what is running" rather than
// "let me reconfigure it". Sections that need backend invokes use
// `TODO(redesign):` markers so future work has a clear hook point.
//
// Sections:
//   - Project    — path + folder actions
//   - Agent      — model / context / provider readouts
//   - Workspace  — Mode pill (Pro/Creator), theme, shortcuts cheatsheet
//   - Indexers   — open project / global indexer config (mirrors IndexRail)
//   - About      — version + AGENTS.md link
//
// Composition follows the modal pattern used elsewhere in the app
// (NewProjectForm, URL import in App.tsx): `.modal-backdrop` ▸ `.modal`
// ▸ `.modal-header` / `.modal-body` / `.modal-footer`. Defined in
// tokens.css; we extend it with section-level utility classes only.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { PublishingSettings } from "./PublishingSettings";
import { useProjectStore } from "./state";
import { useAgentsMdEditor } from "../state/agentsMdEditor";
import { useIntroState } from "../state/introState";
import { useMode } from "../state/mode";
import { useSettings } from "../state/settings";
import { useAuth } from "../state/auth";
import { useWelcome } from "../state/welcome";
import { Button, Inline, Stack } from "../ui";
import type { IndexerConfigSnapshot } from "../shell";

const APP_VERSION = "0.1.0";
// `Awidat Pro 1.2` is the placeholder label Wave 1 used in the footer.
// When the backend exposes a `get_agent_model` invoke we can replace
// this with the real value — see `TODO(redesign):` below.
const AGENT_MODEL = "Awidat Pro 1.2";
const AGENT_CONTEXT = "local";
const AGENT_PROVIDER = "Codex bridge";

/** Static shortcut cheatsheet — mirrors the menu-command bindings the
 *  chrome already handles plus the new ⌘, opener wired in App.tsx. */
const SHORTCUTS: ReadonlyArray<{ keys: string; label: string }> = [
  { keys: "⌘N", label: "New project" },
  { keys: "⌘O", label: "Open project" },
  { keys: "⌘I", label: "Import files" },
  { keys: "⌘1", label: "Focus agent rail" },
  { keys: "⌘2", label: "Focus media rail" },
  { keys: "⌘,", label: "Open settings" },
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

  function openFolder(path: string) {
    if (!isTauri()) return;
    openPath(path).catch((e) => console.warn("openPath failed", e));
  }

  function revealFolder(path: string) {
    if (!isTauri()) return;
    revealItemInDir(path).catch((e) => console.warn("revealItemInDir failed", e));
  }

  function editAgentsMd() {
    if (!projectPath) return;
    // Close Settings first so the editor opens on a clean backdrop —
    // stacked modals make ⌘W / Esc ambiguous.
    close();
    openAgentsMdEditor();
  }

  function showWelcomeAgain() {
    // Same rationale as editAgentsMd — close Settings before opening
    // another modal so the Esc / backdrop semantics stay sane.
    close();
    resetWelcome();
  }

  function manageSignIn() {
    // Close Settings before opening the auth chooser — avoid stacked modals.
    close();
    openAuth();
  }

  return (
    <div
      className="modal-backdrop"
      onClick={close}
      role="presentation"
    >
      <div
        className="modal"
        onClick={(event) => event.stopPropagation()}
        style={{ width: "min(720px, calc(100vw - 48px))", maxHeight: "min(560px, calc(100vh - 48px))" }}
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
      >
        <header className="modal-header">
          <h2>Settings</h2>
          <button
            type="button"
            className="modal-close"
            onClick={close}
            aria-label="Close settings"
          >
            ×
          </button>
        </header>
        <div className="modal-body">
          <Section title="Project">
            <Row label="Path" mono value={projectPath ?? "No project loaded"}>
              {projectPath ? (
                <>
                  <Button variant="secondary" size="sm" onClick={() => openFolder(projectPath)}>
                    Open folder
                  </Button>
                  <Button variant="ghost" size="sm" onClick={() => revealFolder(projectPath)}>
                    Reveal in Finder
                  </Button>
                </>
              ) : null}
            </Row>
            <Row label="AGENTS.md">
              <Button
                variant="secondary"
                size="sm"
                onClick={editAgentsMd}
                disabled={!projectPath}
                title={projectPath ? "Open the in-product editor" : "Load a project first"}
              >
                Edit AGENTS.md
              </Button>
            </Row>
          </Section>

          <Section title="OpenAI account">
            {/* Which "wallet" powers the agent — ChatGPT plan vs API key.
                Full chooser lives in AuthChooser; this is the entry point. */}
            <Row
              label="Powered by"
              value={authStatus ? authStatus.walletTitle : "Not signed in"}
            >
              <Button variant="secondary" size="sm" onClick={manageSignIn}>
                Manage sign-in
              </Button>
            </Row>
            {authStatus?.accountHint ? (
              <Row label="Account" mono value={authStatus.accountHint} />
            ) : null}
            {authStatus?.viaEnv ? (
              <Row
                label="Note"
                value="An OPENAI_API_KEY in your environment is overriding stored auth."
              />
            ) : null}
          </Section>

          <Section title="Publishing">
            {/* W5.A5 — provider connection rows + AI-disclosure toggle +
                default-upload-targets pickers + BYO-credentials inputs.
                Isolated in PublishingSettings.tsx so this modal stays
                readable. */}
            <PublishingSettings />
          </Section>

          <Section title="Agent">
            {/* TODO(redesign): replace these constants with a backend
                invoke (e.g. `get_agent_model`) once the bridge exposes
                runtime model + provider metadata. */}
            <Row label="Model" value={AGENT_MODEL} />
            <Row label="Context window" mono value={AGENT_CONTEXT} />
            <Row label="Provider" value={AGENT_PROVIDER} />
          </Section>

          <Section title="Workspace">
            <Row label="Mode">
              <ModePill mode={mode} onChange={setMode} />
            </Row>
            <Row label="Theme" value="Dark (default)" />
            <Row
              label="Re-introduce on project open"
              value={
                introducedCount === 0
                  ? "Off · next open will introduce"
                  : `${introducedCount} project${introducedCount === 1 ? "" : "s"} introduced`
              }
            >
              <Button
                variant="ghost"
                size="sm"
                onClick={resetIntroState}
                disabled={introducedCount === 0}
                title="Clear the set of projects that have already received the agent intro turn"
              >
                Reset
              </Button>
            </Row>
            <Row
              label="Show welcome again"
              value={
                welcomeShown
                  ? "Dismissed · click to re-open the intro"
                  : "Welcome card pending on next launch"
              }
            >
              <Button
                variant="ghost"
                size="sm"
                onClick={showWelcomeAgain}
                title="Re-open the three-idea welcome card"
              >
                Show
              </Button>
            </Row>
            <Stack gap="1" className="mt-1">
              <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
                Keyboard shortcuts
              </span>
              <Stack gap="1" className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] p-2">
                {SHORTCUTS.map((shortcut) => (
                  <Inline key={shortcut.keys} justify="between" align="center" gap="2">
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
                      {shortcut.label}
                    </span>
                    <kbd className="font-mono text-[var(--text-caption)] text-[var(--color-text-primary)] border border-[var(--color-border-subtle)] rounded-[var(--radius-xs)] px-1.5 py-0.5 bg-[var(--color-surface-card)]">
                      {shortcut.keys}
                    </kbd>
                  </Inline>
                ))}
              </Stack>
            </Stack>
          </Section>

          <Section title="Indexers">
            <Row
              label="Project config"
              mono
              value={indexerConfig?.projectPath ?? "Unavailable"}
            >
              {indexerConfig?.projectPath ? (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => openFolder(indexerConfig.projectPath!)}
                >
                  Open
                </Button>
              ) : null}
            </Row>
            <Row
              label="Global config"
              mono
              value={indexerConfig?.globalPath ?? "Unavailable"}
            >
              {indexerConfig?.globalPath ? (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => openFolder(indexerConfig.globalPath!)}
                >
                  Open
                </Button>
              ) : null}
            </Row>
          </Section>

          <Section title="About">
            <Row label="Version" mono value={APP_VERSION} />
            {/* Build SHA isn't surfaced to the renderer right now — leave
                a hook so we can wire it once a vite-time `__BUILD_SHA__`
                define is in place. */}
            <Row label="Awidat" value="Studio for agent-driven editing" />
          </Section>
        </div>
        <footer className="modal-footer">
          <button type="button" onClick={close}>
            Close
          </button>
        </footer>
      </div>
    </div>
  );
}

/** Section header + stacked rows. Sections are separated by a thin rule
 *  drawn via the wrapper's bottom border. */
function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Stack gap="2" className="pb-3 border-b border-[var(--color-border-subtle)] last:border-b-0 last:pb-0">
      <h3 className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] m-0">
        {title}
      </h3>
      <Stack gap="2">{children}</Stack>
    </Stack>
  );
}

/** A `label: value [action]` row. `mono` makes the value monospace —
 *  use for paths, IDs, and version strings. */
function Row({
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
    <Inline justify="between" align="center" gap="3" className="min-h-[28px]">
      <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] shrink-0">
        {label}
      </span>
      <Inline gap="2" align="center" justify="end" className="min-w-0 flex-1">
        {value !== undefined ? (
          <span
            className={
              mono
                ? "font-mono text-[var(--text-caption)] text-[var(--color-text-primary)] truncate"
                : "text-[var(--text-body-sm)] text-[var(--color-text-primary)] truncate"
            }
            title={value}
          >
            {value}
          </span>
        ) : null}
        {children}
      </Inline>
    </Inline>
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
      className="inline-flex items-center gap-0.5 px-1 py-0.5 rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] text-[11px] font-semibold text-[var(--color-text-muted)]"
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
          ? "px-2 py-0.5 rounded-full bg-[var(--color-surface-selected)] text-[var(--color-text-primary)] shadow-[inset_0_0_0_1px_var(--color-border)]"
          : "px-2 py-0.5 rounded-full hover:text-[var(--color-text-primary)]"
      }
    >
      {label}
    </button>
  );
}
