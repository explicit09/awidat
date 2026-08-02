// SettingsModal → Publishing section.
//
// Owns two groups:
//
//   1. Connected accounts — the server-backed social publishing surface
//      (`<SocialAccounts />`). Connect / list / disconnect run through the
//      `social_*` Tauri commands over the `montage-social` SocialApi facade;
//      tokens stay server-side. This replaces the legacy desktop-local
//      OAuth-connection rows ("replace as we go" — see
//      docs/superpowers/specs/2026-06-03-social-desktop-ui-design.md).
//
//   2. Default upload targets. These persist across projects.
//
// Why a separate file: SettingsModal.tsx is already a stack of orthogonal
// sections; the Publishing section is the heaviest, so isolating it keeps the
// modal readable.

import { SocialAccounts } from "./social/SocialAccounts";
import { VISIBLE_PROVIDERS } from "./publishingSettingsModel";
import { useUploadPrefs } from "../state/uploadPrefs";

export function PublishingSettings() {
  const uploadDefaults = useUploadPrefs((s) => s.enabled);
  const toggleUploadDefault = useUploadPrefs((s) => s.toggle);

  // BYO-credential state/handlers removed: the server holds the OAuth app and
  // the server-backed <SocialAccounts /> surface owns connect/list/disconnect.

  return (
    <div className="grid gap-3">
      <div className="rounded-xl border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] p-3">
        <SocialAccounts />
      </div>

      <div className="grid gap-3 rounded-xl border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] p-3">
        <div>
          <h4 className="m-0 text-[13px] font-bold tracking-normal text-[var(--color-text-primary)]">
            Preferences
          </h4>
          <p className="m-0 mt-1 text-[11px] text-[var(--color-text-muted)]">
            Defaults for queued renders.
          </p>
        </div>
        <div className="grid gap-1">
          <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
            Default upload targets
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            Providers below get the Upload pip toggled on by default when
            you queue a new render.
          </span>
          <div className="flex flex-wrap items-center gap-2">
            {VISIBLE_PROVIDERS.map((p) => (
              <DefaultTargetCheckbox
                key={p.key}
                label={p.displayName}
                checked={uploadDefaults.has(p.key)}
                onChange={() => void toggleUploadDefault(p.key)}
              />
            ))}
          </div>
        </div>
      </div>

      {/* BYO-credentials UI removed: the desktop is a thin client of the
          montage-social server, which holds the OAuth app server-side. Users
          connect via the server-backed <SocialAccounts /> surface above
          ("just sign in"). They no longer paste per-platform client_id/secret.
          This panel intentionally has no local credential path. */}
    </div>
  );
}

// ----------------------------------------------------------------- rows

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
    <label className="glass-ghost inline-flex cursor-pointer items-center gap-1.5 rounded-lg px-2.5 py-1.5">
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
