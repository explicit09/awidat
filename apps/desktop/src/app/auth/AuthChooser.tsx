// AuthChooser - pick how the agent is powered: your ChatGPT plan or an API key.
//
// The whole point of this surface is transparency about *which wallet gets
// charged*. Each option names its wallet explicitly, and the persistent banner
// shows the active one. Uses the same glass modal language as Settings.
//
// All real work lives in the `auth_*` Tauri commands -> `montage-auth` crate ->
// codex-login. This component only renders state and calls those actions.

import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState, type ReactNode } from "react";
import { useAuth, type AuthStatus } from "../../state/auth";
import { useSettings } from "../../state/settings";

export function AuthChooser() {
  const isOpen = useAuth((s) => s.isOpen);
  const close = useAuth((s) => s.close);
  const status = useAuth((s) => s.status);
  const loading = useAuth((s) => s.loading);
  const error = useAuth((s) => s.error);
  const signInWithChatgpt = useAuth((s) => s.signInWithChatgpt);
  const setApiKey = useAuth((s) => s.setApiKey);
  const logout = useAuth((s) => s.logout);
  const settingsOpen = useSettings((s) => s.isOpen);

  const [apiKeyInput, setApiKeyInput] = useState("");
  const [apiKeyError, setApiKeyError] = useState<string | null>(null);

  // Esc closes the same way SettingsModal does.
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

  async function onSignInChatgpt() {
    const url = await signInWithChatgpt();
    // codex auto-opens the browser; this is the manual fallback if it didn't.
    if (url) openUrl(url).catch((e) => console.warn("openUrl failed", e));
  }

  async function onSaveApiKey() {
    setApiKeyError(null);
    try {
      await setApiKey(apiKeyInput);
      setApiKeyInput("");
    } catch (e) {
      // The crate's validation message comes through as the error string.
      setApiKeyError(String(e).replace(/^.*invalid API key:\s*/i, "Invalid API key: "));
    }
  }

  return (
    <div className="modal-backdrop" onClick={close} role="presentation">
      <div
        className="glass glass-strong flex flex-col overflow-hidden text-[var(--color-text-primary)]"
        onClick={(event) => event.stopPropagation()}
        style={{
          width: "min(660px, calc(100vw - 48px))",
          maxHeight: "min(620px, calc(100vh - 48px))",
          borderRadius: 16,
          boxShadow: "0 28px 90px rgba(0,0,0,0.62), 0 0 0 1px rgba(239,68,68,0.12)",
        }}
        role="dialog"
        aria-modal="true"
        aria-label="Sign in to OpenAI"
      >
        <header className="flex items-center justify-between border-b border-[var(--glass-border)] bg-[rgba(10,10,14,0.58)] px-5 py-4">
          <div className="min-w-0">
            <p className="m-0 text-[10px] font-semibold uppercase tracking-[0.14em] text-[var(--color-brand)]">
              OpenAI account
            </p>
            <h2 className="m-0 mt-1 truncate text-[20px] font-bold tracking-normal">
              How should the agent be powered?
            </h2>
          </div>
          <button
            type="button"
            className="glass-content grid h-8 w-8 place-items-center rounded-lg text-[18px] leading-none text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
            onClick={close}
            aria-label="Close"
          >
            ×
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-auto bg-[rgba(10,10,14,0.24)] p-5">
          <div className="grid gap-4">
            {settingsOpen ? (
              <button
                type="button"
                className="glass-ghost w-fit rounded-lg px-3 py-1.5 text-[12px] font-semibold"
                onClick={close}
              >
                Back to settings
              </button>
            ) : null}
            <ActiveWalletBanner status={status} onLogout={() => void logout()} loading={loading} />

            {error ? <ErrorBox message={error} /> : null}

            <div className="auth-options grid gap-3">
              <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                Review the{" "}
                <a
                  href="https://tadiwa.co/montage/privacy"
                  target="_blank"
                  rel="noreferrer"
                >
                  privacy policy
                </a>{" "}
                before connecting accounts or sending media-derived context to model providers.
              </span>
              <OptionCard
                title="Sign in with ChatGPT"
                wallet="Uses your ChatGPT plan"
                recommended
                detail="Best for a personal desktop setup. Uses your ChatGPT plan allowance and avoids per-token API billing."
                footnote="An auto-generated API key may appear in your OpenAI dashboard after sign-in. That is expected."
                active={status?.mode === "chatgpt"}
              >
                <GlassButton onClick={() => void onSignInChatgpt()} disabled={loading}>
                  {loading ? "Working..." : status?.mode === "chatgpt" ? "Change account" : "Continue with ChatGPT"}
                </GlassButton>
              </OptionCard>

              <OptionCard
                title="API key"
                wallet="Billed to your OpenAI API account"
                detail="Use this for automation, CI, or a shared production billing setup."
                active={status?.mode === "api_key"}
              >
                <div className="auth-option-grid grid gap-2">
                  <input
                    type="password"
                    className="auth-input rounded-lg border border-[var(--glass-border)] bg-[rgba(8,9,12,0.62)] px-3 py-2 font-mono text-[12px] text-[var(--color-text-primary)] outline-none focus:border-[rgba(239,68,68,0.45)]"
                    placeholder="sk-..."
                    value={apiKeyInput}
                    spellCheck={false}
                    autoComplete="off"
                    onChange={(e) => setApiKeyInput(e.target.value)}
                  />
                  {apiKeyError ? <ErrorBox message={apiKeyError} /> : null}
                  <GlassButton
                    onClick={() => void onSaveApiKey()}
                    disabled={loading || apiKeyInput.trim().length === 0}
                  >
                    Save API key
                  </GlassButton>
                </div>
              </OptionCard>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/** Persistent "active wallet" banner so the charged account is always visible. */
function ActiveWalletBanner({
  status,
  onLogout,
  loading,
}: {
  status: AuthStatus | null;
  onLogout: () => void;
  loading: boolean;
}) {
  const title = status?.walletTitle ?? "Not signed in";
  const detail = normalizeAccountCopy(status?.walletDetail ?? "No OpenAI credentials found yet.");
  return (
    <div className="auth-current-wallet glass-content flex items-center justify-between gap-3 rounded-xl p-3">
      <div className="min-w-0">
        <div className="flex items-center gap-2 text-[14px] font-semibold leading-snug text-[var(--color-text-primary)]">
          <span className="h-2 w-2 rounded-full bg-[var(--color-brand)] shadow-[0_0_14px_rgba(239,68,68,0.52)]" />
          <span className="truncate">Powered by: {title}</span>
          {status?.accountHint ? (
            <span className="shrink-0 font-normal text-[var(--color-text-muted)]">{status.accountHint}</span>
          ) : null}
        </div>
        <div className="mt-1 text-[13px] leading-snug text-[var(--color-text-secondary)]">{detail}</div>
        {status?.viaEnv ? (
          <div className="mt-1 text-[12px] leading-snug text-[var(--color-text-warning,#fbbf24)]">
            A {status.envVar ?? "credential"} in your environment is overriding this; unset it to
            use the wallet you choose here.
          </div>
        ) : null}
      </div>
      {status && status.mode !== "none" ? (
        <GlassButton variant="ghost" onClick={onLogout} disabled={loading}>
          Disconnect
        </GlassButton>
      ) : null}
    </div>
  );
}

function OptionCard({
  title,
  wallet,
  detail,
  footnote,
  recommended,
  active,
  children,
}: {
  title: string;
  wallet: string;
  detail: string;
  footnote?: string;
  recommended?: boolean;
  active?: boolean;
  children: React.ReactNode;
}) {
  return (
    <div
      className={
        active
          ? "auth-option-card glass-content rounded-xl border-[rgba(239,68,68,0.42)] p-3"
          : "auth-option-card glass-content rounded-xl p-3"
      }
    >
      <div className="grid gap-3">
        <div className="flex items-start justify-between gap-3">
          <div className="grid gap-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-[15px] font-semibold leading-snug text-[var(--color-text-primary)]">{title}</span>
              {recommended ? <Tag label="Recommended" /> : null}
              {active ? <Tag label="Active" tone="active" /> : null}
            </div>
            <span className="text-[13px] leading-snug text-[var(--color-text-secondary)]">{wallet}</span>
          </div>
        </div>
        <span className="text-[13px] leading-snug text-[var(--color-text-muted)]">{detail}</span>
        {footnote ? (
          <span className="text-[12px] leading-snug text-[var(--color-text-muted)]">
            {footnote}
          </span>
        ) : null}
        <div className="auth-option-action">{children}</div>
      </div>
    </div>
  );
}

function normalizeAccountCopy(value: string): string {
  return value.replace(/\s+—\s+/g, ". ");
}

function Tag({ label, tone }: { label: string; tone?: "active" }) {
  return (
    <span
      className={
        tone === "active"
          ? "rounded-full border border-[rgba(239,68,68,0.34)] bg-[rgba(239,68,68,0.12)] px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.08em] text-[var(--color-text-primary)]"
          : "rounded-full border border-[var(--glass-border)] bg-[rgba(255,255,255,0.05)] px-2 py-0.5 text-[10px] font-bold uppercase tracking-[0.08em] text-[var(--color-text-muted)]"
      }
    >
      {label}
    </span>
  );
}

function ErrorBox({ message }: { message: string }) {
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
  variant = "primary",
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  variant?: "primary" | "ghost";
}) {
  return (
    <button
      type="button"
      className={
        variant === "primary"
          ? "glass-cta rounded-lg px-3 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
          : "glass-ghost rounded-lg px-3 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
      }
      onClick={onClick}
      disabled={disabled}
    >
      {children}
    </button>
  );
}
