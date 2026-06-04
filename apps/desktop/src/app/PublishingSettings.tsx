// SettingsModal → Publishing section.
//
// Owns three groups:
//
//   1. Connected accounts — the server-backed social publishing surface
//      (`<SocialAccounts />`). Connect / list / disconnect run through the
//      `social_*` Tauri commands over the `awidat-social` SocialApi facade;
//      tokens stay server-side. This replaces the legacy desktop-local
//      OAuth-connection rows ("replace as we go" — see
//      docs/superpowers/specs/2026-06-03-social-desktop-ui-design.md).
//
//   2. Global publishing preferences — Auto-disclose AI toggle and the
//      default-targets checkbox set. These persist across projects.
//
//   3. BYO OAuth-app credentials — `client_id` / `client_secret` inputs per
//      provider. Still serve the legacy upload path that has not yet been
//      replaced by the server-backed worker; retained until that cutover.
//
// Why a separate file: SettingsModal.tsx is already a stack of orthogonal
// sections; the Publishing section is the heaviest, so isolating it keeps the
// modal readable.

import { SocialAccounts } from "./social/SocialAccounts";
import { PROVIDERS } from "./publishingSettingsModel";
import { useAiDisclosure } from "../state/aiDisclosure";
import { useUploadPrefs } from "../state/uploadPrefs";
import { Inline, Stack } from "../ui";

export function PublishingSettings() {
  const autoDisclose = useAiDisclosure((s) => s.autoDiscloseEnabled);
  const setAutoDisclose = useAiDisclosure((s) => s.setAutoDiscloseEnabled);
  const uploadDefaults = useUploadPrefs((s) => s.enabled);
  const toggleUploadDefault = useUploadPrefs((s) => s.toggle);

  // BYO-credential state/handlers removed: the server holds the OAuth app and
  // the server-backed <SocialAccounts /> surface owns connect/list/disconnect.

  return (
    <Stack gap="3">
      {/* ---- Connected accounts (server-backed) ---- */}
      <SocialAccounts />

      {/* ---- Global preferences ---- */}
      <Stack gap="2" className="pt-3 border-t border-[var(--color-border-subtle)]">
        <h4 className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] m-0">
          Preferences
        </h4>
        <PreferenceToggle
          label="Auto-disclose AI content"
          note="AI labels are required by YouTube, TikTok, and Meta for synthetic content."
          checked={autoDisclose}
          onChange={setAutoDisclose}
        />
        <Stack gap="1">
          <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
            Default upload targets
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            Providers below get the Upload pip toggled on by default when
            you queue a new render.
          </span>
          <Inline gap="2" align="center" className="flex-wrap">
            {PROVIDERS.map((p) => (
              <DefaultTargetCheckbox
                key={p.key}
                label={p.displayName}
                checked={uploadDefaults.has(p.key)}
                onChange={() => void toggleUploadDefault(p.key)}
              />
            ))}
          </Inline>
        </Stack>
      </Stack>

      {/* BYO-credentials UI removed: the desktop is a thin client of the
          awidat-social server, which holds the OAuth app server-side. Users
          connect via the server-backed <SocialAccounts /> surface above
          ("just sign in") — they no longer paste per-platform client_id/secret.
          The legacy local-publishing path still exists behind the
          `legacy_local_publishing` build flag (render-queue auto-upload); this
          panel intentionally no longer surfaces it. */}
    </Stack>
  );
}

// ----------------------------------------------------------------- rows

function PreferenceToggle({
  label,
  note,
  checked,
  onChange,
}: {
  label: string;
  note: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <Stack gap="1">
      <Inline justify="between" align="center" gap="2">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
          {label}
        </span>
        <label className="inline-flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => onChange(e.target.checked)}
            className="accent-[var(--color-accent-primary,#3b82f6)]"
          />
          <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">
            {checked ? "On" : "Off"}
          </span>
        </label>
      </Inline>
      <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
        {note}
      </span>
    </Stack>
  );
}

function DefaultTargetCheckbox({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <label className="inline-flex items-center gap-1.5 cursor-pointer px-2 py-1 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]">
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        className="accent-[var(--color-accent-primary,#3b82f6)]"
      />
      <span className="text-[var(--text-caption)] text-[var(--color-text-primary)]">
        {label}
      </span>
    </label>
  );
}

