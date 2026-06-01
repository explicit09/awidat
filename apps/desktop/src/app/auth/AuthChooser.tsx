// AuthChooser — pick how the agent is powered: your ChatGPT plan or an API key.
//
// The whole point of this surface is transparency about *which wallet gets
// charged*. Each option names its wallet explicitly, and the persistent banner
// shows the active one. Follows the modal pattern used elsewhere
// (SettingsModal, NewProjectForm): `.modal-backdrop` ▸ `.modal` ▸ header/body.
//
// All real work lives in the `auth_*` Tauri commands → `awidat-auth` crate →
// codex-login. This component only renders state and calls those actions.

import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import { useAuth, type AuthStatus } from "../../state/auth";
import { Button, Stack } from "../../ui";

export function AuthChooser() {
  const isOpen = useAuth((s) => s.isOpen);
  const close = useAuth((s) => s.close);
  const status = useAuth((s) => s.status);
  const loading = useAuth((s) => s.loading);
  const error = useAuth((s) => s.error);
  const signInWithChatgpt = useAuth((s) => s.signInWithChatgpt);
  const setApiKey = useAuth((s) => s.setApiKey);
  const logout = useAuth((s) => s.logout);

  const [apiKeyInput, setApiKeyInput] = useState("");
  const [apiKeyError, setApiKeyError] = useState<string | null>(null);

  // Esc closes — same convention as SettingsModal.
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

  const signedIn = status && status.mode !== "none";

  return (
    <div className="modal-backdrop" onClick={close} role="presentation">
      <div
        className="modal"
        onClick={(event) => event.stopPropagation()}
        style={{ width: "min(620px, calc(100vw - 48px))", maxHeight: "min(620px, calc(100vh - 48px))" }}
        role="dialog"
        aria-modal="true"
        aria-label="Sign in to OpenAI"
      >
        <header className="modal-header">
          <h2>How should the agent be powered?</h2>
          <button type="button" className="modal-close" onClick={close} aria-label="Close">
            ×
          </button>
        </header>

        <div className="modal-body">
          <Stack gap="3">
            <ActiveWalletBanner status={status} onLogout={() => void logout()} loading={loading} />

            {error ? <ErrorBox message={error} /> : null}

            <OptionCard
              title="Sign in with ChatGPT"
              wallet="Uses your ChatGPT plan"
              recommended
              detail="Spends your ChatGPT Plus / Pro / Business plan's Codex allowance — no per-token API charges, subject to your plan's usage limits."
              footnote="After signing in, an auto-generated API key may appear in your OpenAI dashboard; that's expected and isn't what's billed here."
              active={status?.mode === "chatgpt"}
            >
              <Button variant="primary" size="sm" onClick={() => void onSignInChatgpt()} disabled={loading}>
                {loading ? "Working…" : "Continue with ChatGPT"}
              </Button>
            </OptionCard>

            <OptionCard
              title="Use an API key"
              wallet="Billed to your OpenAI API account"
              detail="Billed per-token to your OpenAI Platform account at standard API rates. Best for automation / CI. This is the mode OpenAI officially supports for third-party apps."
              active={status?.mode === "api_key"}
            >
              <Stack gap="2">
                <input
                  type="password"
                  className="auth-input"
                  placeholder="sk-…"
                  value={apiKeyInput}
                  spellCheck={false}
                  autoComplete="off"
                  onChange={(e) => setApiKeyInput(e.target.value)}
                  style={{
                    width: "100%",
                    fontFamily: "var(--font-mono, monospace)",
                    fontSize: "var(--text-caption)",
                    padding: "6px 8px",
                    borderRadius: "var(--radius-sm)",
                    border: "1px solid var(--color-border-subtle)",
                    background: "var(--color-surface-input)",
                    color: "var(--color-text-primary)",
                  }}
                />
                {apiKeyError ? <ErrorBox message={apiKeyError} /> : null}
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => void onSaveApiKey()}
                  disabled={loading || apiKeyInput.trim().length === 0}
                >
                  Save API key
                </Button>
              </Stack>
            </OptionCard>

            {signedIn ? (
              <Button variant="ghost" size="sm" onClick={() => void logout()} disabled={loading}>
                Sign out
              </Button>
            ) : null}
          </Stack>
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
  const detail = status?.walletDetail ?? "No OpenAI credentials found yet.";
  return (
    <div
      style={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
        gap: "12px",
        padding: "10px 12px",
        borderRadius: "var(--radius-sm)",
        border: "1px solid var(--color-border-subtle)",
        background: "var(--color-surface-card)",
      }}
    >
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600, color: "var(--color-text-primary)", fontSize: "var(--text-body-sm)" }}>
          Powered by: {title}
          {status?.accountHint ? (
            <span style={{ color: "var(--color-text-muted)", fontWeight: 400 }}> · {status.accountHint}</span>
          ) : null}
        </div>
        <div style={{ color: "var(--color-text-secondary)", fontSize: "var(--text-caption)" }}>{detail}</div>
        {status?.viaEnv ? (
          <div style={{ color: "var(--color-text-warning, #c90)", fontSize: "var(--text-caption)" }}>
            A {status.envVar ?? "credential"} in your environment is overriding this; unset it to
            use the wallet you choose here.
          </div>
        ) : null}
      </div>
      {status && status.mode !== "none" ? (
        <Button variant="ghost" size="sm" onClick={onLogout} disabled={loading}>
          Sign out
        </Button>
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
      style={{
        padding: "12px",
        borderRadius: "var(--radius-sm)",
        border: active ? "1px solid var(--color-border)" : "1px solid var(--color-border-subtle)",
        background: active ? "var(--color-surface-selected)" : "var(--color-surface-panel)",
      }}
    >
      <Stack gap="2">
        <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
          <span style={{ fontWeight: 600, color: "var(--color-text-primary)" }}>{title}</span>
          {recommended ? <Tag label="Recommended" /> : null}
          {active ? <Tag label="Active" tone="active" /> : null}
        </div>
        <span style={{ color: "var(--color-text-secondary)", fontSize: "var(--text-body-sm)" }}>{wallet}</span>
        <span style={{ color: "var(--color-text-muted)", fontSize: "var(--text-caption)" }}>{detail}</span>
        {footnote ? (
          <span style={{ color: "var(--color-text-muted)", fontSize: "var(--text-caption)", fontStyle: "italic" }}>
            {footnote}
          </span>
        ) : null}
        <div>{children}</div>
      </Stack>
    </div>
  );
}

function Tag({ label, tone }: { label: string; tone?: "active" }) {
  return (
    <span
      style={{
        fontSize: "10px",
        textTransform: "uppercase",
        letterSpacing: "0.04em",
        fontWeight: 700,
        padding: "1px 6px",
        borderRadius: "999px",
        color: tone === "active" ? "var(--color-text-primary)" : "var(--color-text-muted)",
        border: "1px solid var(--color-border-subtle)",
        background: "var(--color-surface-card)",
      }}
    >
      {label}
    </span>
  );
}

function ErrorBox({ message }: { message: string }) {
  return (
    <div
      role="alert"
      style={{
        padding: "8px 10px",
        borderRadius: "var(--radius-sm)",
        border: "1px solid var(--color-border-danger, #b44)",
        background: "var(--color-surface-danger, rgba(180,68,68,0.1))",
        color: "var(--color-text-danger, #d66)",
        fontSize: "var(--text-caption)",
      }}
    >
      {message}
    </div>
  );
}
