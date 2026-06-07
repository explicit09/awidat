# Social Desktop UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put a desktop surface (connect accounts, schedule a publish, monitor jobs, review audit) on top of the verified `montage-social` `SocialApi` facade, bridged in-process through Tauri commands, replacing the legacy desktop-local publishing connect/status path as we go.

**Architecture:** React 19 surfaces call thin Tauri commands (`commands/social.rs`); each command builds a single-user `ApiActor`/`ApiOwner`, locks a file-backed `SqliteSocialStore` held in `MontageState`, calls `SocialApi`, and returns a camelCase serde DTO carrying no token material. No HTTP layer; the command bodies lift onto axum later. Worker status uses the crate's mock adapters this pass.

**Tech Stack:** Rust 2024, `montage-social`, `rusqlite`, Tauri 2, React 19 + TypeScript, existing desktop node test harness.

Spec: `docs/superpowers/specs/2026-06-03-social-desktop-ui-design.md`.

---

## File Structure

- Modify `crates/social/src/sqlite_store.rs` — add `SqliteSocialStore::open(path)`.
- Modify `apps/desktop/src-tauri/src/state.rs` — add `social: Mutex<Option<SqliteSocialStore>>` field.
- Modify `apps/desktop/src-tauri/src/lib.rs` — open the store in `.setup()`, register `social_*` commands.
- Create `apps/desktop/src-tauri/src/commands/social.rs` — the 13 thin commands + response DTOs + tests.
- Modify `apps/desktop/src-tauri/src/commands/mod.rs` — declare the `social` module.
- Modify `apps/desktop/src-tauri/Cargo.toml` — depend on `montage-social`.
- Create `apps/desktop/src/app/social/socialModel.ts` — types + pure derivations.
- Create `apps/desktop/src/app/social/SocialAccounts.tsx`, `SocialSchedule.tsx`, `SocialJobs.tsx`, `SocialAudit.tsx`.
- Create `apps/desktop/src/app/social/social.test.ts` — model unit tests.
- Modify legacy publishing connect surfaces only at the cutover steps named below.

Each Rust command file stays focused: `commands/social.rs` holds only social commands and their DTOs. The React folder splits model (JSX-free, tested) from presentation, matching `publishingSettingsModel.ts` / `PublishingSettings.tsx`.

---

## Task 1: File-backed SqliteSocialStore

**Files:**
- Modify: `crates/social/src/sqlite_store.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/social/src/sqlite_store.rs`:

```rust
#[test]
fn open_persists_account_across_reopen() {
    let dir = std::env::temp_dir().join(format!("montage_social_open_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("create temp dir: {err}"));
    let path = dir.join("social.sqlite");
    let _ = std::fs::remove_file(&path);

    {
        let mut store =
            SqliteSocialStore::open(&path).unwrap_or_else(|err| panic!("open store: {err}"));
        store
            .save_connected_account(connected_account("acct_open"))
            .unwrap_or_else(|err| panic!("save account: {err}"));
    }

    let reopened =
        SqliteSocialStore::open(&path).unwrap_or_else(|err| panic!("reopen store: {err}"));
    let loaded = reopened
        .connected_account("acct_open")
        .unwrap_or_else(|err| panic!("load account: {err}"));
    assert_eq!(loaded.id, "acct_open");

    let _ = std::fs::remove_file(&path);
}
```

Note: the existing `connected_account(id)` test helper hardcodes `provider_account_id: "channel_1"`; the unique index tolerates a single account, so reuse it as-is.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p montage-social sqlite_store::tests::open_persists_account_across_reopen`
Expected: FAIL — `no function or associated item named `open` found`.

- [ ] **Step 3: Write minimal implementation**

In `crates/social/src/sqlite_store.rs`, add to `impl SqliteSocialStore`, directly after `new_in_memory`:

```rust
/// Opens (creating if absent) a file-backed store and ensures the schema.
pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, SocialStoreError> {
    let connection = Connection::open(path).map_err(storage_error)?;
    let store = Self { connection };
    store.create_schema()?;
    Ok(store)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p montage-social sqlite_store::tests::open_persists_account_across_reopen`
Expected: PASS.

- [ ] **Step 5: Verify and commit**

Run: `cargo test -p montage-social && cargo clippy -p montage-social --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all pass, fmt exit 0.

```bash
git add crates/social/src/sqlite_store.rs
git commit -m "feat(social): add file-backed SqliteSocialStore::open"
```

---

## Task 2: Wire the store into MontageState

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Modify: `apps/desktop/src-tauri/src/state.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

The store cannot be `Default` (it needs a path + fallible open), so it is an
`Option` initialized in `.setup()`, matching the `Mutex<Option<…>>` pattern
already used for `codex`/`turn`/`project_root`.

- [ ] **Step 1: Add the crate dependency**

In `apps/desktop/src-tauri/Cargo.toml`, under `[dependencies]`, add:

```toml
montage-social = { workspace = true }
```

- [ ] **Step 2: Add the state field**

In `apps/desktop/src-tauri/src/state.rs`, add this field to `struct MontageState`
(after `project_root`), and add the import at the top:

```rust
// top of file, with the other use statements:
use montage_social::sqlite_store::SqliteSocialStore;
```

```rust
    /// Server-backed social publishing store (file-backed SQLite under the
    /// app data dir). `None` until initialized in the Tauri `.setup()` hook.
    /// Guards all `SocialApi` calls from the `social_*` commands.
    pub social: Mutex<Option<SqliteSocialStore>>,
```

(`#[derive(Default)]` still applies: `Mutex<Option<_>>` defaults to `Mutex(None)`.)

- [ ] **Step 3: Initialize the store in setup**

In `apps/desktop/src-tauri/src/lib.rs`, inside the existing `.setup(|app| { … })`
closure (where `app` is available), add before the closure returns `Ok(())`:

```rust
        {
            use tauri::Manager;
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|err| format!("resolve app data dir: {err}"))?;
            std::fs::create_dir_all(&data_dir)
                .map_err(|err| format!("create app data dir: {err}"))?;
            let social_path = data_dir.join("social.sqlite");
            let store = montage_social::sqlite_store::SqliteSocialStore::open(&social_path)
                .map_err(|err| format!("open social store: {err}"))?;
            let state = app.state::<crate::state::MontageState>();
            // `MontageState.social` is a tokio Mutex; the setup closure is sync,
            // so use `blocking_lock()` exactly as the existing setup block does
            // for `project_root`.
            *state.social.blocking_lock() = Some(store);
        }
```

The existing `.setup` closure already uses `app.state::<MontageState>()` and
`.blocking_lock()` (see the `project_root` block) and returns a boxed-error
`Result`, so the `String` errors above coerce via `?`. If your `String` does not
coerce, append `.map_err(|e| -> Box<dyn std::error::Error> { e.into() })`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p montage-desktop` (use the actual desktop crate name from
`apps/desktop/src-tauri/Cargo.toml`'s `[package] name`).
Expected: compiles. (No command uses the field yet — `social` is read in Task 3.)

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src/state.rs apps/desktop/src-tauri/src/lib.rs
git commit -m "feat(desktop): hold file-backed social store in MontageState"
```

---

## Task 3: Account commands + cutover of legacy connect

**Files:**
- Create: `apps/desktop/src-tauri/src/commands/social.rs`
- Modify: `apps/desktop/src-tauri/src/commands/mod.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register commands)

This task adds the account-management commands and the shared command
scaffolding (local user id, state accessor, error mapping). DTOs reuse the
facade response types directly where they already serialize.

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src-tauri/src/commands/social.rs` with the scaffolding,
the account commands, and this test module:

```rust
//! Server-backed social publishing Tauri commands.
//!
//! Each command is a thin translation: build a single-user actor/owner, lock
//! the file-backed `SqliteSocialStore` in `MontageState`, call `SocialApi`, and
//! return a serde response carrying no token material. No business logic here.

use montage_social::api::{
    ApiActor, ApiOwner, AccountSummary, OAuthCompleteRequest, OAuthStartRequest, ProviderSummary,
    SocialApi, SocialApiError,
};
use montage_social::model::{OwnerRef, Provider};
use montage_social::oauth_url::OAuthProviderConfig;
use montage_social::provider::ProviderRegistry;
use montage_social::store::SocialStore;
use montage_social::token::LocalTokenKeyProvider;
use tauri::State;

use crate::state::MontageState;

/// Stable single-user identity for this pass. Swapped for a real authenticated
/// user id when an identity service exists; see the design doc.
const LOCAL_USER_ID: &str = "local-user";

fn actor() -> ApiActor {
    ApiActor::new(LOCAL_USER_ID, Vec::new())
}

fn owner() -> ApiOwner {
    ApiOwner::user(LOCAL_USER_ID)
}

/// Maps a `SocialApiError` to a stable string the frontend can branch on.
fn err_string(err: SocialApiError) -> String {
    match err {
        SocialApiError::Unauthorized => "unauthorized".to_string(),
        other => other.to_string(),
    }
}

/// Runs `f` with an exclusive lock on the initialized social store.
///
/// `MontageState.social` is a `tokio::sync::Mutex`, so this is async and locks
/// via `.lock().await` — matching every other command in this crate.
async fn with_store<T>(
    state: &State<'_, MontageState>,
    f: impl FnOnce(&mut montage_social::sqlite_store::SqliteSocialStore) -> Result<T, SocialApiError>,
) -> Result<T, String> {
    let mut guard = state.social.lock().await;
    let store = guard
        .as_mut()
        .ok_or_else(|| "social store not initialized".to_string())?;
    f(store).map_err(err_string)
}

#[tauri::command]
pub async fn social_providers() -> Result<Vec<ProviderSummary>, String> {
    let registry = ProviderRegistry::default_multi_platform();
    Ok(SocialApi::providers(&registry))
}

#[tauri::command]
pub async fn social_accounts(
    state: State<'_, MontageState>,
) -> Result<Vec<AccountSummary>, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| SocialApi::accounts(store, &actor, &owner)).await
}

// CONVENTION for all commands below: `with_store(&state, |store| …)` returns a
// future — every call site ends with `.await`. (The examples in later tasks
// show the closure; append `.await` exactly as shown here.)

#[cfg(test)]
mod tests {
    use super::*;
    use montage_social::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus,
        ProviderCapabilities,
    };
    use montage_social::sqlite_store::SqliteSocialStore;
    use montage_social::store::SocialStore;

    fn store_with_account() -> SqliteSocialStore {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("open store: {err}"));
        store
            .save_connected_account(ConnectedAccount {
                id: "acct_1".into(),
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                provider: Provider::YouTube,
                provider_account_id: "channel_1".into(),
                display_name: "Montage Channel".into(),
                handle: Some("@montage".into()),
                avatar_url: None,
                account_kind: AccountKind::Channel,
                status: ConnectedAccountStatus::Connected,
                scopes: vec!["youtube.upload".into()],
                capabilities: ProviderCapabilities::default(),
                eligibility: AccountEligibility::eligible(),
                last_verified_at: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
    }

    #[test]
    fn accounts_for_local_user_are_token_safe() {
        let mut store = store_with_account();
        let accounts = SocialApi::accounts(&store, &actor(), &owner())
            .unwrap_or_else(|err| panic!("accounts: {err}"));
        assert_eq!(accounts.len(), 1);
        let json = serde_json::to_string(&accounts)
            .unwrap_or_else(|err| panic!("serialize: {err}"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
        let _ = &mut store; // store reused to assert facade is store-generic
    }

    #[test]
    fn providers_list_has_three_slots() {
        let registry = ProviderRegistry::default_multi_platform();
        assert_eq!(SocialApi::providers(&registry).len(), 3);
    }
}
```

- [ ] **Step 2: Declare the module and run the failing test**

In `apps/desktop/src-tauri/src/commands/mod.rs`, add:

```rust
pub mod social;
```

Run: `cargo test -p montage-desktop social::tests`
Expected: FAIL to compile until `mod.rs` change is saved, then PASS once it is —
if it already passes, that is acceptable (the test exercises the facade through
the store directly). The red signal for this task is the module not existing.

- [ ] **Step 3: Add the remaining account commands**

Append to `apps/desktop/src-tauri/src/commands/social.rs`, before the test module.

`OAuthStartArgs`/`OAuthCompleteArgs` are the camelCase argument DTOs the frontend
sends; `social_oauth_complete` builds the deterministic stub token bundle (no
live exchange) described in the spec.

```rust
use montage_social::model::ProviderCapabilities as _Caps; // (only if needed; remove if unused)
```

```rust
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStartArgs {
    pub oauth_connection_id: String,
    pub provider: Provider,
    pub client_id: String,
    pub redirect_uri: String,
    pub raw_state: String,
    pub return_to: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[tauri::command]
pub async fn social_oauth_start(
    state: State<'_, MontageState>,
    args: OAuthStartArgs,
) -> Result<montage_social::api::OAuthStartResponse, String> {
    let actor = actor();
    with_store(&state, |store| {
        SocialApi::oauth_start(
            store,
            &actor,
            OAuthStartRequest {
                oauth_connection_id: args.oauth_connection_id,
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                provider: args.provider,
                config: OAuthProviderConfig {
                    client_id: args.client_id,
                    redirect_uri: args.redirect_uri,
                },
                raw_state: args.raw_state,
                return_to: args.return_to,
                created_at: args.created_at,
                expires_at: args.expires_at,
            },
        )
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCompleteArgs {
    pub oauth_connection_id: String,
    pub provider: Provider,
    pub raw_state: String,
    pub account_id: String,
    pub provider_account_id: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub now: i64,
}

#[tauri::command]
pub async fn social_oauth_complete(
    state: State<'_, MontageState>,
    args: OAuthCompleteArgs,
) -> Result<montage_social::api::OAuthCompleteResponse, String> {
    use montage_social::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus,
        ProviderCapabilities,
    };
    use montage_social::token_bundle::ProviderTokenBundle;

    let actor = actor();
    let key_provider = LocalDevKeyProvider;
    with_store(&state, |store| {
        let connected_account = ConnectedAccount {
            id: args.account_id.clone(),
            owner: OwnerRef::User(LOCAL_USER_ID.into()),
            provider: args.provider.clone(),
            provider_account_id: args.provider_account_id.clone(),
            display_name: args.display_name.clone(),
            handle: args.handle.clone(),
            avatar_url: None,
            account_kind: AccountKind::Unknown,
            status: ConnectedAccountStatus::Connected,
            scopes: Vec::new(),
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: args.now,
            updated_at: args.now,
        };
        // Deterministic stub bundle — no live token exchange this pass.
        let token_bundle = ProviderTokenBundle {
            provider: args.provider.clone(),
            provider_account_id: args.provider_account_id.clone(),
            scopes: Vec::new(),
            access_token_expires_at: args.now + 3_600,
            refresh_token_expires_at: Some(args.now + 86_400),
        };
        SocialApi::oauth_complete(
            store,
            &key_provider,
            &actor,
            OAuthCompleteRequest {
                oauth_connection_id: args.oauth_connection_id.clone(),
                owner: OwnerRef::User(LOCAL_USER_ID.into()),
                raw_state: args.raw_state.clone(),
                connected_account,
                token_bundle,
                access_token: format!("stub-access-{}", args.account_id),
                refresh_token: Some(format!("stub-refresh-{}", args.account_id)),
                now: args.now,
            },
        )
    })
}

#[tauri::command]
pub async fn social_disconnect_account(
    state: State<'_, MontageState>,
    account_id: String,
    now: i64,
) -> Result<AccountSummary, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| {
        SocialApi::disconnect_account(store, &actor, &owner, &account_id, now)
    })
}

/// Local XOR-envelope key provider for desktop dev. Mirrors the crate's
/// `TestKeyProvider` contract; real KMS lands with the live-provider work.
struct LocalDevKeyProvider;

impl LocalTokenKeyProvider for LocalDevKeyProvider {
    // Implement the trait methods exactly as `TestKeyProvider` does — copy the
    // method signatures from `crates/social/src/token.rs` (key id + key
    // material). Verify the trait surface before writing; do not guess.
}
```

Before writing `LocalDevKeyProvider`, open `crates/social/src/token.rs`, read
the `LocalTokenKeyProvider` trait definition and the `TestKeyProvider` impl, and
mirror it with a fixed key id/material constant. If `TestKeyProvider` is not
`#[cfg(test)]`-gated (confirmed: it is `pub` and not test-gated), you may use
`montage_social::token::TestKeyProvider::new("desktop-key", "local-key")`
directly instead of defining `LocalDevKeyProvider` — prefer that if available.

- [ ] **Step 4: Register the commands**

In `apps/desktop/src-tauri/src/lib.rs`, add inside `tauri::generate_handler![ … ]`:

```rust
            commands::social::social_providers,
            commands::social::social_accounts,
            commands::social::social_oauth_start,
            commands::social::social_oauth_complete,
            commands::social::social_disconnect_account,
```

- [ ] **Step 5: Run tests + compile**

Run: `cargo test -p montage-desktop social::tests`
Run: `cargo check -p montage-desktop`
Expected: tests PASS, crate compiles.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/social.rs apps/desktop/src-tauri/src/commands/mod.rs apps/desktop/src-tauri/src/lib.rs
git commit -m "feat(desktop): add server-backed social account commands"
```

---

## Task 4: Accounts surface + model (cutover legacy connect UI)

**Files:**
- Create: `apps/desktop/src/app/social/socialModel.ts`
- Create: `apps/desktop/src/app/social/social.test.ts`
- Create: `apps/desktop/src/app/social/SocialAccounts.tsx`

- [ ] **Step 1: Write the failing model test**

This repo runs node-native tests via `node --experimental-strip-types` with
`node:assert` (see `apps/desktop/package.json` `test:*` scripts, e.g.
`test:tokens`, `test:status-pill`). It does NOT use vitest/jest. Place the test
under `apps/desktop/tests/` to match siblings, importing from the source tree.

Create `apps/desktop/tests/social-model.test.ts`:

```ts
import { strict as assert } from "node:assert";

import {
  accountStatusLabel,
  eligibilitySummary,
} from "../src/app/social/socialModel.ts";

assert.equal(accountStatusLabel("connected"), "Connected");
assert.equal(accountStatusLabel("needs_reauth"), "Needs reconnect");
assert.equal(eligibilitySummary({ eligible: true, reasons: [] }), "Eligible");
assert.equal(
  eligibilitySummary({ eligible: false, reasons: ["account_not_eligible"] }),
  "Not eligible — account not eligible",
);

console.log("social-model.test.ts: ok");
```

Wire it into `apps/desktop/package.json`: add a script
`"test:social-model": "node --experimental-strip-types tests/social-model.test.ts"`
and append `&& npm run test:social-model` to the `"test"` script chain (before
the trailing `node tests/desktop-ui-smoke.mjs`).

(Check the exact import-extension convention in a sibling like
`tests/status-pill.test.ts` first — Node strip-types may require the `.ts`
suffix on relative imports, as shown above.)

- [ ] **Step 2: Run test to verify it fails**

Run (from `apps/desktop/`): `npm run test:social-model`.
Expected: FAIL — cannot resolve `socialModel` (not yet created).

- [ ] **Step 3: Write the model**

Create `apps/desktop/src/app/social/socialModel.ts`. Field names are the
camelCase serde mirror of `AccountSummary`/`ProviderSummary` from the facade:

```ts
export type Provider = "youtube" | "tiktok" | "instagram";

export type OwnerRef = { user: string } | { workspace: string };

export type Eligibility = { eligible: boolean; reasons: string[] };

export type Capabilities = {
  nativeScheduling: boolean;
  queueScheduling: boolean;
  uploadVideo: boolean;
  uploadThumbnail: boolean;
  publicPosting: boolean;
  requiresUserConsent: boolean;
};

export type AccountStatus =
  | "connected"
  | "needs_reauth"
  | "missing_scope"
  | "ineligible"
  | "disabled"
  | "revoked";

export type AccountSummary = {
  id: string;
  owner: OwnerRef;
  provider: Provider;
  providerAccountId: string;
  displayName: string;
  handle: string | null;
  avatarUrl: string | null;
  accountKind: string;
  status: AccountStatus;
  scopes: string[];
  capabilities: Capabilities;
  eligibility: Eligibility;
  lastVerifiedAt: number | null;
  createdAt: number;
  updatedAt: number;
};

const STATUS_LABELS: Record<AccountStatus, string> = {
  connected: "Connected",
  needs_reauth: "Needs reconnect",
  missing_scope: "Missing permission",
  ineligible: "Not eligible",
  disabled: "Disabled",
  revoked: "Revoked",
};

export function accountStatusLabel(status: AccountStatus): string {
  return STATUS_LABELS[status];
}

/** Maps facade reason codes to human copy. Extend as new codes appear. */
const REASON_COPY: Record<string, string> = {
  account_not_eligible: "account not eligible",
  account_not_connected: "account not connected",
  missing_publish_capability: "missing publish capability",
  scheduled_time_invalid: "scheduled time is in the past",
  missing_youtube_upload_scope: "missing YouTube upload permission",
};

export function reasonCopy(code: string): string {
  return REASON_COPY[code] ?? code.replace(/_/g, " ");
}

export function eligibilitySummary(eligibility: Eligibility): string {
  if (eligibility.eligible) return "Eligible";
  const first = eligibility.reasons[0];
  return first ? `Not eligible — ${reasonCopy(first)}` : "Not eligible";
}
```

- [ ] **Step 4: Run test to verify it passes**

Run the TS test command again.
Expected: PASS.

- [ ] **Step 5: Write the Accounts surface**

Create `apps/desktop/src/app/social/SocialAccounts.tsx`. It lists accounts via
`invoke("social_accounts")`, renders a **dot + label** status (house style: no
colored pills), and connects via `social_oauth_start` → open URL, then
`social_oauth_complete`. Use the existing `@tauri-apps/api/core` `invoke` and
`@tauri-apps/plugin-opener` patterns already in `PublishingSettings.tsx` (read
it for the exact imports). Keep all derivation in `socialModel.ts`.

```tsx
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  accountStatusLabel,
  eligibilitySummary,
  type AccountSummary,
} from "./socialModel";

export function SocialAccounts() {
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setAccounts(await invoke<AccountSummary[]>("social_accounts"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <section className="social-accounts">
      <header>Connected accounts</header>
      {error && <p role="alert">{error}</p>}
      <ul>
        {accounts.map((a) => (
          <li key={a.id}>
            <span className="status-dot" data-status={a.status} aria-hidden />
            <span>{a.displayName}</span>
            <span>{accountStatusLabel(a.status)}</span>
            <span>{eligibilitySummary(a.eligibility)}</span>
            <button
              onClick={async () => {
                try {
                  await invoke("social_disconnect_account", {
                    accountId: a.id,
                    now: Math.floor(Date.now() / 1000),
                  });
                  await refresh();
                } catch (e) {
                  setError(String(e));
                }
              }}
            >
              Disconnect
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
```

Styling (dot color by `data-status`) follows the existing CSS-variable system;
mirror the muted/hairline approach from `PublishingSettings.tsx`. Do not
introduce colored pills.

- [ ] **Step 6: Cutover — mount SocialAccounts, retire legacy connect UI**

In the Settings/Publishing screen that currently renders the legacy connect
section (`PublishingSettings.tsx`), replace the per-provider connect/disconnect
block with `<SocialAccounts />`. Leave credential entry and any not-yet-replaced
sections intact. Do NOT delete `PublishingSettings.tsx` wholesale — remove only
the connect/status/disconnect portion now served by the new surface.

- [ ] **Step 7: Verify and commit**

Run the desktop TS test command and `cargo check -p montage-desktop`.
Expected: model tests pass, crate compiles.

```bash
git add apps/desktop/src/app/social/ apps/desktop/src/app/PublishingSettings.tsx
git commit -m "feat(desktop): server-backed accounts surface; retire legacy connect UI"
```

---

## Task 5: Publish commands + Schedule and Jobs surfaces

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/social.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register)
- Create: `apps/desktop/src/app/social/SocialSchedule.tsx`
- Create: `apps/desktop/src/app/social/SocialJobs.tsx`
- Modify: `apps/desktop/src/app/social/socialModel.ts` (+ job types/derivations)
- Modify: `apps/desktop/src/app/social/social.test.ts` (+ job tests)

- [ ] **Step 1: Write the failing command test**

Add to the `tests` module in `commands/social.rs`:

```rust
#[test]
fn bind_validate_schedule_round_trip() {
    use montage_social::api::{
        BindTargetRequest, ScheduleTargetRequest, ValidateTargetRequest,
    };
    use montage_social::model::{PublishJobStatus, ValidationState};

    let mut store = store_with_publishable_account();
    let registry = ProviderRegistry::default_multi_platform();

    SocialApi::bind_target(
        &mut store,
        &actor(),
        BindTargetRequest {
            target_id: "target_1".into(),
            campaign_id: "campaign_1".into(),
            variant_id: "variant_1".into(),
            connected_account_id: "acct_1".into(),
            platform_fields: serde_json::json!({"privacy": "private"}),
            scheduled_for: 5_000,
            now: 1_000,
        },
    )
    .unwrap_or_else(|err| panic!("bind: {err}"));

    let validated = SocialApi::validate_target(
        &mut store,
        &registry,
        &actor(),
        ValidateTargetRequest { target_id: "target_1".into(), now: 1_100 },
    )
    .unwrap_or_else(|err| panic!("validate: {err}"));
    assert_eq!(validated.validation_state, ValidationState::Valid);

    let job = SocialApi::schedule_target(
        &mut store,
        &registry,
        &actor(),
        ScheduleTargetRequest {
            target_id: "target_1".into(),
            job_id: "job_1".into(),
            artifact_ref: "render://artifact_1".into(),
            created_by: LOCAL_USER_ID.into(),
            now: 1_200,
        },
    )
    .unwrap_or_else(|err| panic!("schedule: {err}"));
    assert_eq!(job.status, PublishJobStatus::Scheduled);
}
```

Add the `store_with_publishable_account` helper to the test module (a YouTube
account with `upload_video`/`public_posting` capabilities + eligible, owned by
`LOCAL_USER_ID`) — copy the capability shape from
`crates/social/src/publish_service.rs` tests' `connected_account(.., true)`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p montage-desktop social::tests::bind_validate_schedule_round_trip`
Expected: FAIL to compile — helper missing — then write the helper; the assertion
logic itself is exercised by the facade.

- [ ] **Step 3: Add the publish commands**

Append to `commands/social.rs` before the test module. `PublishJobResponse`,
`CampaignVariantTarget` come from the facade and already serialize.

```rust
use montage_social::api::{
    BindTargetRequest, PublishJobResponse, ScheduleTargetRequest, ValidateTargetRequest,
};
use montage_social::model::CampaignVariantTarget;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindArgs {
    pub target_id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub now: i64,
}

#[tauri::command]
pub async fn social_bind_target(
    state: State<'_, MontageState>,
    args: BindArgs,
) -> Result<CampaignVariantTarget, String> {
    let actor = actor();
    with_store(&state, |store| {
        SocialApi::bind_target(
            store,
            &actor,
            BindTargetRequest {
                target_id: args.target_id,
                campaign_id: args.campaign_id,
                variant_id: args.variant_id,
                connected_account_id: args.connected_account_id,
                platform_fields: args.platform_fields,
                scheduled_for: args.scheduled_for,
                now: args.now,
            },
        )
    })
}

#[tauri::command]
pub async fn social_validate_target(
    state: State<'_, MontageState>,
    target_id: String,
    now: i64,
) -> Result<CampaignVariantTarget, String> {
    let actor = actor();
    let registry = ProviderRegistry::default_multi_platform();
    with_store(&state, |store| {
        SocialApi::validate_target(
            store,
            &registry,
            &actor,
            ValidateTargetRequest { target_id, now },
        )
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleArgs {
    pub target_id: String,
    pub job_id: String,
    pub artifact_ref: String,
    pub now: i64,
}

#[tauri::command]
pub async fn social_schedule_target(
    state: State<'_, MontageState>,
    args: ScheduleArgs,
) -> Result<PublishJobResponse, String> {
    let actor = actor();
    let registry = ProviderRegistry::default_multi_platform();
    with_store(&state, |store| {
        SocialApi::schedule_target(
            store,
            &registry,
            &actor,
            ScheduleTargetRequest {
                target_id: args.target_id,
                job_id: args.job_id,
                artifact_ref: args.artifact_ref,
                created_by: LOCAL_USER_ID.into(),
                now: args.now,
            },
        )
    })
}

#[tauri::command]
pub async fn social_publish_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<PublishJobResponse, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| {
        SocialApi::publish_job(store, &actor, &owner, &job_id)
    })
}

#[tauri::command]
pub async fn social_cancel_job(
    state: State<'_, MontageState>,
    job_id: String,
    now: i64,
) -> Result<PublishJobResponse, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| {
        SocialApi::cancel_job(store, &actor, &owner, &job_id, now)
    })
}

#[tauri::command]
pub async fn social_retry_job(
    state: State<'_, MontageState>,
    job_id: String,
    now: i64,
) -> Result<PublishJobResponse, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| {
        SocialApi::retry_job(store, &actor, &owner, &job_id, now)
    })
}
```

Note: `SocialApi::publish_job` takes `&impl SocialStore` (read-only), but
`with_store` hands out `&mut`; that is fine — `&mut` coerces to `&`.

- [ ] **Step 4: Register and run tests**

Add to `generate_handler!` in `lib.rs`:

```rust
            commands::social::social_bind_target,
            commands::social::social_validate_target,
            commands::social::social_schedule_target,
            commands::social::social_publish_job,
            commands::social::social_cancel_job,
            commands::social::social_retry_job,
```

Run: `cargo test -p montage-desktop social::tests`
Expected: PASS.

- [ ] **Step 5: Add job types + derivations to the model**

Append to `socialModel.ts`:

```ts
export type PublishJobStatus =
  | "draft"
  | "validated"
  | "scheduled"
  | "uploading"
  | "processing"
  | "published"
  | "failed"
  | "requires_action"
  | "cancelled";

export type PublishJobEvent = {
  id: string;
  eventType: string;
  message: string;
  metadata: unknown;
  createdAt: number;
};

export type PublishJob = {
  id: string;
  campaignId: string;
  variantId: string;
  connectedAccountId: string;
  provider: Provider;
  status: PublishJobStatus;
  attemptCount: number;
  scheduledFor: number;
  providerPostId: string | null;
  providerPostUrl: string | null;
  normalizedError: string | null;
  rawErrorRef: string | null;
  requiresActionReason: string | null;
  createdAt: number;
  updatedAt: number;
  events: PublishJobEvent[];
};

const JOB_STATUS_LABELS: Record<PublishJobStatus, string> = {
  draft: "Draft",
  validated: "Validated",
  scheduled: "Scheduled",
  uploading: "Uploading",
  processing: "Processing",
  published: "Published",
  failed: "Failed",
  requires_action: "Action needed",
  cancelled: "Cancelled",
};

export function jobStatusLabel(status: PublishJobStatus): string {
  return JOB_STATUS_LABELS[status];
}

export function canCancel(status: PublishJobStatus): boolean {
  return status !== "published" && status !== "cancelled";
}

export function canRetry(status: PublishJobStatus): boolean {
  return status === "failed" || status === "requires_action";
}
```

Append to `apps/desktop/tests/social-model.test.ts` (node:assert style):

```ts
import { jobStatusLabel, canCancel, canRetry } from "../src/app/social/socialModel.ts";

assert.equal(jobStatusLabel("processing"), "Processing");
assert.equal(canCancel("scheduled"), true);
assert.equal(canCancel("published"), false);
assert.equal(canRetry("failed"), true);
assert.equal(canRetry("scheduled"), false);

console.log("social-model.test.ts (jobs): ok");
```

- [ ] **Step 6: Write Schedule and Jobs surfaces**

Create `SocialSchedule.tsx` (pick account + scheduled time + platform fields →
`social_bind_target` → `social_validate_target`, show `reasonCopy` of any
reasons → `social_schedule_target`) and `SocialJobs.tsx` (list jobs, dot+label
status via `jobStatusLabel`, Cancel/Retry gated by `canCancel`/`canRetry`
calling `social_cancel_job`/`social_retry_job`). Follow the `invoke` + error
state pattern from `SocialAccounts.tsx`. Keep all derivation in the model.

(Full component code mirrors `SocialAccounts.tsx`'s structure: `useState` for
data + error, an async action calling `invoke`, dot+label rendering. Write each
list item with `data-status={job.status}` for the status dot.)

- [ ] **Step 7: Run TS tests + compile, commit**

Run the desktop TS test command and `cargo check -p montage-desktop`.

```bash
git add apps/desktop/src-tauri/src/commands/social.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src/app/social/
git commit -m "feat(desktop): server-backed schedule + jobs surfaces"
```

---

## Task 6: Worker commands + status advance

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/social.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register)
- Modify: `apps/desktop/src/app/social/SocialJobs.tsx` (advance button)

Worker commands use the crate's **mock** adapters so the lifecycle is
demonstrable without live providers. `claim_due_jobs` (queue claim) is exposed
so a Scheduled job becomes Uploading before upload.

- [ ] **Step 1: Write the failing command test**

Add to the `tests` module in `commands/social.rs`:

```rust
#[test]
fn worker_advances_scheduled_job_to_published() {
    use montage_social::publish_service::PublishService;
    use montage_social::upload_adapter::MockUploadAdapter;
    use montage_social::model::PublishJobStatus;
    // A processing-then-poll status adapter:
    use montage_social::upload_status::{
        UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError,
        UploadStatusRequest, UploadStatusResult,
    };

    struct DonePoll;
    impl UploadStatusAdapter for DonePoll {
        fn provider(&self) -> Provider { Provider::YouTube }
        fn poll_status(
            &self,
            _r: &UploadStatusRequest,
        ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
            Ok(UploadStatusResult {
                provider_post_id: "yt_1".into(),
                provider_post_url: Some("https://youtu.be/yt_1".into()),
                status: UploadProcessingStatus::Published,
                normalized_error: None,
                raw_error_ref: None,
            })
        }
    }

    let mut store = scheduled_job_store(); // helper: account + token + Scheduled job_1
    // Claim the due job (Scheduled -> Uploading).
    let claimed = PublishService::claim_due_jobs(&mut store, 5_000, 10)
        .unwrap_or_else(|err| panic!("claim: {err}"));
    assert_eq!(claimed.len(), 1);

    // Upload (processing).
    let adapter = ProcessingUpload; // local adapter returning processing: true
    let uploaded = SocialApi::execute_claimed_upload_job(
        &mut store,
        &adapter,
        montage_social::api::ExecuteUploadRequest {
            job_id: "job_1".into(),
            title: "t".into(),
            description: None,
            tags: vec![],
            thumbnail_ref: None,
            now: 5_100,
        },
    )
    .unwrap_or_else(|err| panic!("upload: {err}"));
    assert_eq!(uploaded.status, PublishJobStatus::Processing);

    // Poll to published.
    let published = SocialApi::poll_upload_status(&mut store, &DonePoll, "job_1", 5_200)
        .unwrap_or_else(|err| panic!("poll: {err}"));
    assert_eq!(published.status, PublishJobStatus::Published);
    let _ = MockUploadAdapter::published(Provider::YouTube, "x", "y"); // ref to keep import
}
```

Add the `ProcessingUpload` test adapter (an `UploadAdapter` returning
`processing: true`) and the `scheduled_job_store` helper (account + token secret
+ a Scheduled `job_1` due at 5000), mirroring the crate's
`crates/social/tests/pipeline_e2e.rs` fixtures.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p montage-desktop social::tests::worker_advances_scheduled_job_to_published`
Expected: FAIL to compile — adapters/helpers missing — then add them; assertions
are exercised by the facade.

- [ ] **Step 3: Add the worker commands**

Append to `commands/social.rs`. They use mock adapters internally this pass.

```rust
use montage_social::api::ExecuteUploadRequest;
use montage_social::publish_service::PublishService;
use montage_social::upload_adapter::MockUploadAdapter;
use montage_social::upload_status::{
    UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError, UploadStatusRequest,
    UploadStatusResult,
};

/// Mock status adapter: reports the provider finished processing. Replaced by a
/// live client in the provider sub-project.
struct MockReadyStatus;
impl UploadStatusAdapter for MockReadyStatus {
    fn provider(&self) -> Provider {
        Provider::YouTube
    }
    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
        Ok(UploadStatusResult {
            provider_post_id: request.provider_post_id.clone(),
            provider_post_url: Some(format!("https://youtu.be/{}", request.provider_post_id)),
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        })
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteUploadArgs {
    pub job_id: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub thumbnail_ref: Option<String>,
    pub now: i64,
}

/// Claim any due jobs, then execute the named upload via the mock adapter.
#[tauri::command]
pub async fn social_execute_upload(
    state: State<'_, MontageState>,
    args: ExecuteUploadArgs,
) -> Result<PublishJobResponse, String> {
    let adapter = MockUploadAdapter::published(
        Provider::YouTube,
        format!("yt_{}", args.job_id),
        format!("https://youtu.be/yt_{}", args.job_id),
    );
    with_store(&state, |store| {
        // Move Scheduled -> Uploading if still due.
        let _ = PublishService::claim_due_jobs(store, args.now, 50)
            .map_err(|err| SocialApiError::Publish(err.to_string()))?;
        SocialApi::execute_claimed_upload_job(
            store,
            &adapter,
            ExecuteUploadRequest {
                job_id: args.job_id,
                title: args.title,
                description: args.description,
                tags: args.tags,
                thumbnail_ref: args.thumbnail_ref,
                now: args.now,
            },
        )
    })
}

#[tauri::command]
pub async fn social_poll_status(
    state: State<'_, MontageState>,
    job_id: String,
    now: i64,
) -> Result<PublishJobResponse, String> {
    let adapter = MockReadyStatus;
    with_store(&state, |store| {
        SocialApi::poll_upload_status(store, &adapter, &job_id, now)
    })
}
```

Note: `MockUploadAdapter::published` returns `processing: false`, which drives
the job straight to Published in one call. That is acceptable for the desktop
demo (the status-poll command remains exercised for jobs left Processing by
future live adapters). If a visible Processing → Published two-step is wanted in
the demo, swap to a local `processing: true` adapter as in the test.

- [ ] **Step 4: Register and run tests**

Add to `generate_handler!`:

```rust
            commands::social::social_execute_upload,
            commands::social::social_poll_status,
```

Run: `cargo test -p montage-desktop social::tests`
Expected: PASS.

- [ ] **Step 5: Add the advance action to SocialJobs**

In `SocialJobs.tsx`, add a "Run / Advance" button per job that calls
`social_execute_upload` (for scheduled/uploading) or `social_poll_status` (for
processing), then refreshes. Gate by status using the model helpers.

- [ ] **Step 6: Verify and commit**

Run: `cargo test -p montage-desktop social::tests` and the TS test command.

```bash
git add apps/desktop/src-tauri/src/commands/social.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src/app/social/SocialJobs.tsx
git commit -m "feat(desktop): worker commands advance jobs to published (mock adapters)"
```

---

## Task 7: Audit command + surface

**Files:**
- Modify: `crates/social/src/api.rs` (add `account_usage_audit` facade method)
- Modify: `apps/desktop/src-tauri/src/commands/social.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register)
- Create: `apps/desktop/src/app/social/SocialAudit.tsx`

The facade has no audit method yet; `TeamService::account_usage_audit` exists in
the crate. Add a thin facade wrapper so the command composes the facade (not the
service directly), consistent with Phase 6's "compose existing services" rule.

- [ ] **Step 1: Write the failing facade test**

In `crates/social/src/api.rs` test module, add:

```rust
#[test]
fn account_audit_returns_owner_scoped_jobs_token_safe() {
    let mut store = InMemorySocialStore::default();
    let registry = ProviderRegistry::default_multi_platform();
    schedule_job(&mut store, &registry, &user_actor(), user_owner());

    let audit = SocialApi::account_usage_audit(
        &store,
        &user_actor(),
        &ApiOwner { owner: user_owner() },
        "acct_1",
    )
    .unwrap_or_else(|err| panic!("audit: {err}"));
    assert_eq!(audit.connected_account_id, "acct_1");
    assert_eq!(audit.jobs.len(), 1);

    let json = serde_json::to_string(&audit)
        .unwrap_or_else(|err| panic!("serialize: {err}"));
    assert!(!json.contains("access_token"));
    assert!(!json.contains("refresh_token"));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p montage-social api::tests::account_audit_returns_owner_scoped_jobs_token_safe`
Expected: FAIL — no `account_usage_audit` on `SocialApi`.

- [ ] **Step 3: Add the facade method**

In `crates/social/src/api.rs`, add to imports:

```rust
use crate::model::AccountUsageAudit;
use crate::team_service::{TeamService, TeamServiceError};
```

Add the error conversion next to the others:

```rust
impl From<TeamServiceError> for SocialApiError {
    fn from(error: TeamServiceError) -> Self {
        match error {
            TeamServiceError::Store(store) => SocialApiError::Store(store),
            other => SocialApiError::Team(other.to_string()),
        }
    }
}
```

Add the method to `impl SocialApi`, after `retry_job`:

```rust
    /// `GET /social/accounts/:id/audit`: per-account jobs, events, and counts.
    pub fn account_usage_audit(
        store: &impl SocialStore,
        actor: &ApiActor,
        owner: &ApiOwner,
        connected_account_id: &str,
    ) -> Result<AccountUsageAudit, SocialApiError> {
        // Confirm the requested owner actually owns the account, then gate read.
        let account_owner = account_owner(store, connected_account_id)?;
        if account_owner != owner.owner {
            return Err(SocialApiError::Unauthorized);
        }
        authorize_read(actor, &owner.owner)?;
        Ok(TeamService::account_usage_audit(
            store,
            &owner.owner,
            connected_account_id,
        )?)
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p montage-social api::tests::account_audit_returns_owner_scoped_jobs_token_safe`
Expected: PASS.
Run: `cargo test -p montage-social && cargo clippy -p montage-social --all-targets -- -D warnings`
Expected: pass, clean.

- [ ] **Step 5: Add the audit command**

Append to `commands/social.rs`:

```rust
use montage_social::model::AccountUsageAudit;

#[tauri::command]
pub async fn social_account_audit(
    state: State<'_, MontageState>,
    account_id: String,
) -> Result<AccountUsageAudit, String> {
    let actor = actor();
    let owner = owner();
    with_store(&state, |store| {
        SocialApi::account_usage_audit(store, &actor, &owner, &account_id)
    })
}
```

Register in `lib.rs`:

```rust
            commands::social::social_account_audit,
```

- [ ] **Step 6: Write the Audit surface**

Create `SocialAudit.tsx`: takes an `accountId`, calls `social_account_audit`,
renders status counts, the job list with final URLs (linkified via the opener),
and the event trail. Add `AccountUsageAudit`/`PublishJobStatusCounts` types to
`socialModel.ts` (camelCase mirror). Follow the established `invoke` + error
pattern; dot+label for status.

- [ ] **Step 7: Verify and commit**

Run: `cargo test -p montage-social`, `cargo test -p montage-desktop social::tests`,
the TS test command, and `cargo fmt --all -- --check`.

```bash
git add crates/social/src/api.rs apps/desktop/src-tauri/src/commands/social.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src/app/social/
git commit -m "feat(social): account audit facade method + desktop audit surface"
```

---

## Task 8: Full verification

**Files:** none unless fixes are required.

- [ ] **Step 1: Crate gates**

Run:

```bash
cargo test -p montage-social
cargo clippy -p montage-social --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected: all pass; fmt exit 0 (the non-blocking `imports_granularity` warning
is acceptable when exit code is 0); diff-check exit 0.

- [ ] **Step 2: Desktop crate gates**

Run:

```bash
cargo test -p montage-desktop social::tests
cargo clippy -p montage-desktop --all-targets -- -D warnings
```

Expected: pass, clean.

- [ ] **Step 3: Desktop TS gates**

Run (from `apps/desktop/`): the project's TS test + typecheck commands
(e.g. `pnpm test` and `pnpm tsc --noEmit`, confirmed from `package.json`).
Expected: model tests pass, no type errors.

- [ ] **Step 4: Manual smoke**

Launch the desktop app. Verify the four surfaces: connect an account (OAuth URL
opens; completion persists), schedule a campaign variant, advance the job to
Published, and view the audit with the final URL. Confirm no token strings
appear in any rendered view or devtools network/state.

- [ ] **Step 5: Fresh review**

Ask a fresh reviewer to check: command-layer leakage of token material; the
single-user actor/owner threading; that no business logic leaked into commands;
that the legacy connect cutover removed only the replaced portion; and
in-process parity with the facade's own tests.

- [ ] **Step 6: Completion status**

Report implemented commits, verification outcomes, review outcome, and remaining
work (HTTP/axum wrapper, live providers, durable worker daemon, team UI, full
legacy `publishing/` deletion).

---

## Self-Review

- **Spec coverage:** Accounts (Tasks 3–4), schedule (Task 5), jobs/monitoring
  (Tasks 5–6), audit (Task 7), file-backed store (Task 1), `MontageState` wiring
  (Task 2), single-user actor (Tasks 3+), mock-adapter worker lifecycle
  (Task 6), token-safety tests (Tasks 3,4,7), legacy "replace as we go" cutover
  (Tasks 3,4). All spec sections map to a task.
- **Placeholder scan:** Component bodies for `SocialSchedule`/`SocialJobs`/
  `SocialAudit` are described structurally rather than fully transcribed because
  they mechanically mirror the fully-shown `SocialAccounts.tsx` (same `invoke` +
  error-state + dot/label pattern); the model layer they depend on is given in
  full and tested. `LocalDevKeyProvider` instructs reusing the existing public
  `TestKeyProvider` rather than guessing the trait surface.
- **Type consistency:** `LOCAL_USER_ID`, `with_store`, `actor()`/`owner()`,
  `err_string` are defined in Task 3 and reused verbatim in Tasks 5–7. camelCase
  TS types in `socialModel.ts` match the facade's serde field names. Facade
  method signatures (`bind_target`, `validate_target`, `schedule_target`,
  `publish_job`, `cancel_job`, `retry_job`, `execute_claimed_upload_job`,
  `poll_upload_status`, new `account_usage_audit`) match `crates/social/src/api.rs`.
