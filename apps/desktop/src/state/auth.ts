// OpenAI auth-mode store.
//
// Mirrors the small surface of `useSettings`: open-state plus the current
// `AuthStatus` snapshot and the async actions that drive the Tauri `auth_*`
// commands (which wrap the `montage-auth` crate / codex-login).
//
// This is "who powers the agent" auth — publishing account OAuth belongs to
// the separate social server.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

/** Stable mode tags emitted by the backend (`commands::auth::mode_tag`). */
export type AuthModeTag = "chatgpt" | "api_key" | "agent_identity" | "none";

/** Frontend mirror of `AuthStatusDto`. */
export interface AuthStatus {
  mode: AuthModeTag;
  /** "Which wallet" short title, e.g. "ChatGPT subscription". */
  walletTitle: string;
  /** One-sentence explanation of what gets charged. */
  walletDetail: string;
  /** Masked credential hint (e.g. `sk-…wxyz`); never the full secret. */
  accountHint: string | null;
  /** True when an environment variable overrides stored auth. */
  viaEnv: boolean;
  /** When `viaEnv`, the name of the overriding variable (e.g. `CODEX_API_KEY`). */
  envVar: string | null;
}

interface AuthStore {
  isOpen: boolean;
  status: AuthStatus | null;
  loading: boolean;
  error: string | null;
  open: () => void;
  close: () => void;
  /** Re-read the effective auth status from disk. */
  refresh: () => Promise<void>;
  /** Start "Sign in with ChatGPT"; resolves to the consent URL (codex also
   *  auto-opens it). Completion arrives later via the `auth-changed` event. */
  signInWithChatgpt: () => Promise<string | null>;
  /** Validate + persist an API key. Throws on validation error so the form can
   *  surface it inline. */
  setApiKey: (key: string) => Promise<void>;
  /** Clear stored credentials. */
  logout: () => Promise<void>;
}

export const useAuth = create<AuthStore>((set, get) => ({
  isOpen: false,
  status: null,
  loading: false,
  error: null,
  open: () => {
    set({ isOpen: true, error: null });
    void get().refresh();
  },
  close: () => set({ isOpen: false }),
  refresh: async () => {
    if (!isTauri()) return;
    set({ loading: true, error: null });
    try {
      const status = await invoke<AuthStatus>("auth_status");
      set({ status, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },
  signInWithChatgpt: async () => {
    if (!isTauri()) return null;
    set({ loading: true, error: null });
    try {
      const res = await invoke<{ authUrl: string; port: number }>("auth_begin_chatgpt");
      set({ loading: false });
      return res.authUrl;
    } catch (e) {
      set({ error: String(e), loading: false });
      return null;
    }
  },
  setApiKey: async (key: string) => {
    if (!isTauri()) return;
    set({ loading: true, error: null });
    try {
      const status = await invoke<AuthStatus>("auth_set_api_key", { key });
      set({ status, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
      throw e;
    }
  },
  logout: async () => {
    if (!isTauri()) return;
    set({ loading: true, error: null });
    try {
      const status = await invoke<AuthStatus>("auth_logout");
      set({ status, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },
}));

// A ChatGPT login finishes in the browser asynchronously; the backend emits
// `auth-changed` once codex has written the new credentials. Refresh on it.
if (isTauri()) {
  void listen("auth-changed", () => {
    void useAuth.getState().refresh();
  });
  void listen<string>("auth-login-failed", (event) => {
    useAuth.setState({ error: `Sign-in failed: ${event.payload}`, loading: false });
  });
}
