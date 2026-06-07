# GPT Auth Chooser — Design

**Date:** 2026-05-30
**Branch:** `feat/gpt-auth-chooser` (worktree)
**Status:** Implemented foundation pending user review (built autonomously while user was away)

## Problem

Montage's agent is powered by the vendored codex harness. Codex authenticates to OpenAI
two ways: **Sign in with ChatGPT** (OAuth — spends the user's ChatGPT plan) or an **API key**
(billed at standard API rates). Today montage fully delegates this: the user must run
`codex login` in a terminal and codex reads `~/.codex/auth.json`. There is **no auth UI in
montage itself** — no way to see who you're signed in as, switch modes, or understand which
"wallet" gets charged.

The product goal: most people already pay for ChatGPT and don't use their Codex allowance, so
let them **sign in with their ChatGPT account inside montage** and spend that existing
subscription — while offering an API-key path for users who prefer/need it.

## Critical context: the ToS reality (must read)

Research finding that shapes the whole design:

- **OpenAI's official "Sign in with ChatGPT" / Apps SDK program is identity + data-scopes
  only. It does NOT route model-inference billing to a user's ChatGPT subscription.** The
  feature request to "bring your own plan to third-party apps" (openai/codex#10974) was
  **closed as "not planned."** You **cannot** register your own subscription-billing client ID;
  that program does not exist.
- **The only token that spends a ChatGPT subscription on inference is the first-party Codex
  OAuth client** (`codex_login::CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"`). Reusing it in a
  standalone third-party app is what OpenCode et al. do. OpenAI has **neither sanctioned nor
  prohibited** it (forking codex-rs is explicitly allowed under Apache-2.0 per maintainer
  statement in openai/codex#8338; reusing the client to bill subscriptions is legally
  unaddressed — "works but unsanctioned, could be restricted at any time, à la Anthropic").
- **Dual-wallet footgun (openai/codex#2000):** after ChatGPT OAuth, codex performs an RFC 8693
  token exchange that mints an *auto-generated API key*. Which wallet is charged depends on
  which token + endpoint a request uses. The UI must be unambiguous and must not let a
  "subscription" user silently incur API-account charges.

**Design consequences:**
1. The API-key path is **first-class**, not a footnote — it is the only ToS-sanctioned mode.
2. The OAuth client ID is **centralized + env-overridable** (`MONTAGE_OAUTH_CLIENT_ID`). Caveat:
   the override only changes the initial browser *authorize* — codex's vendored token *refresh*
   and *revoke* still use its built-in client, so a full pivot also needs codex-side changes.
   `oauth_client_id()` logs a warning when an override is active so the limitation isn't silent.
3. The UI **names the wallet explicitly** at selection and persistently after login.

## Decisions (made autonomously, per user delegation)

- **Both paths ship now**, ChatGPT sign-in presented as the headline, API key as an equal,
  fully-functional fallback. (User chose "Both, ChatGPT default".)
- **Drive the vendored `codex-login` crate in-process** — do not reimplement OAuth, do not shell
  out to `codex login`. This is DRY (one OAuth/refresh/storage implementation, codex's) and
  keeps montage's stored creds byte-identical to what codex reads.
- **Write where codex reads:** same `CODEX_HOME` (`~/.codex`) and same credential store mode
  codex uses, so a login performed in montage is immediately visible to the running agent.

## Architecture

Three layers, each with one responsibility (SOC/SRP):

```
React AuthChooser modal  ── invoke() ──▶  Tauri auth commands  ──▶  montage-auth crate  ──▶  codex-login
(apps/desktop/src)                        (src-tauri/src/auth)      (crates/auth)            (vendor)
   which-wallet UI                          thin #[tauri::command]   pure-Rust wrapper        OAuth/apikey/logout
```

### Layer 1 — `crates/auth` (new crate `montage-auth`)

Pure-Rust, UI-agnostic, unit-testable. Single responsibility: **the montage↔codex auth
boundary.** Depends only on `codex-login`, `codex-utils-home-dir`, and `codex-config` (for the
store-mode enum, already re-exported by codex-login).

Public API (intended shape):

```rust
/// Which OAuth client to use for ChatGPT sign-in. Centralized + env-overridable so the
/// policy-risky reuse of codex's first-party client lives in exactly one place.
pub fn oauth_client_id() -> String;              // env MONTAGE_OAUTH_CLIENT_ID else codex_login::CLIENT_ID

pub struct AuthEnv { codex_home: PathBuf, store_mode: AuthCredentialsStoreMode }
impl AuthEnv { pub fn resolve() -> io::Result<Self>; }   // find_codex_home + config.toml store mode (default File)

pub enum AuthModeKind { ChatGpt, ApiKey, AgentIdentity, None }

pub struct AuthStatus {
    pub mode: AuthModeKind,
    pub wallet: WalletLabel,        // human "which wallet is charged" descriptor
    pub account_hint: Option<String>, // masked email / "sk-…1234", never the full secret
}

pub struct WalletLabel { pub title: String, pub detail: String } // transparency copy, one source of truth

pub fn status(env: &AuthEnv) -> AuthStatus;          // reads auth.json, classifies mode
pub fn set_api_key(env: &AuthEnv, key: &str) -> Result<(), AuthError>;  // validates then login_with_api_key
pub fn begin_chatgpt_login(env: &AuthEnv) -> Result<LoginHandle, AuthError>; // run_login_server, returns auth_url+port+cancel
pub fn logout(env: &AuthEnv) -> Result<(), AuthError>;

pub fn validate_api_key(raw: &str) -> Result<String, AuthError>; // trim, non-empty, sk- sanity; pure → unit tested
```

**Testable core (TDD):** `validate_api_key` (trim/empty/shape), `status` classification from a
temp `auth.json` (ChatGPT vs ApiKey vs none), `WalletLabel` correctness per mode, and
`oauth_client_id()` env override. The OAuth server + logout are thin pass-throughs to codex;
we test option construction (correct client_id/issuer/store mode), not the network.

### Layer 2 — Tauri commands (`apps/desktop/src-tauri/src/auth/`)

Thin `#[tauri::command]` wrappers, registered in `lib.rs` `generate_handler!`:

- `auth_status() -> AuthStatusDto`
- `auth_set_api_key(key: String) -> Result<(), String>`
- `auth_begin_chatgpt() -> Result<BeginLoginDto, String>` (returns `auth_url`; opens browser via codex)
- `auth_logout() -> Result<(), String>`

They resolve `AuthEnv` once, call the crate, map errors to strings (codebase convention).
A small DTO module mirrors the crate types for serde across the Tauri boundary.

### Layer 3 — React `AuthChooser` (`apps/desktop/src/app/auth/`)

- `AuthChooser.tsx` — modal, two wallet-named cards (copy below), API-key entry field,
  loading/error states. Matches existing modal pattern (`SettingsModal.tsx`, `invoke<T>`).
- `authStore.ts` — Zustand store: current `AuthStatus`, open/close, async actions.
- Persistent status chip ("Powered by: ChatGPT subscription (a••@…)" / "API key (sk-…1234)").

**Transparency copy (one source of truth, surfaced in UI):**
- *Sign in with ChatGPT* — "Uses your ChatGPT Plus/Pro/Business plan's Codex allowance. No
  per-token API charges; subject to your plan's usage limits." + disclosure that an
  auto-generated API key may appear in the OpenAI dashboard (the #2000 behavior).
- *Use an API key* — "Billed per-token to your OpenAI Platform account at standard API rates.
  Best for automation/CI. This is the only mode OpenAI officially supports for third-party apps."

## Error handling

- `validate_api_key` rejects empty/whitespace and obviously-malformed keys before touching disk;
  errors are user-actionable strings.
- `begin_chatgpt_login` surfaces port-bind failure (1455/1457 busy) distinctly so the UI can
  offer the manual-URL / device-code fallback later (out of scope v1; noted).
- All crate errors are a typed `AuthError` enum (fail loud, full context per house rules);
  Tauri maps to `Result<_, String>`.
- Mismatch guard: `AuthEnv::resolve` reads the same store mode codex uses; documented limitation
  if a user hand-edits `config.toml` to an exotic mode (default `File` covers ~all users).

## Testing

- Crate unit tests (cargo, scoped `-p montage-auth`): validation, status classification against
  temp `CODEX_HOME`, wallet labels, client-id override.
- Tauri command layer: kept logic-free so the crate tests carry correctness.
- Frontend: manual verification (chooser renders, both flows reachable). No e2e in v1.

## Out of scope (v1)

- Device-code / manual-URL headless fallback (designed-for, not built).
- Applying to any OpenAI program (none grants the subscription-billing capability anyway).
- Real OAuth client registration / rebranding away from codex's client ID.
- TikTok/IG-style multi-account switching.

## Guardrails honored while building unattended

Isolated worktree branch; no merge, no push; no real OAuth client registered; no real secrets
written; no outward/irreversible actions. Everything here is reviewable before it leaves the branch.
