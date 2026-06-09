# Provider Key Vault and Advanced Setup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace scattered provider-key Keychain access with one Montage vault plus an Advanced desktop settings flow for BYO provider keys.

**Architecture:** Add a versioned `secrets_vault` JSON item to `crates/secrets`, cache it in process memory, and make generic secret lookup prefer env vars before the vault. Desktop exposes provider status/save/remove commands and a Settings `Advanced -> Provider Keys` panel. Legacy per-key Keychain reads are kept behind an explicit import command rather than startup probing.

**Tech Stack:** Rust (`keyring`, `serde`, Tauri commands), React/TypeScript (`invoke`, existing `SettingsModal` components), existing desktop tests through `cargo test` and `tsc`.

---

## File Structure

- Modify `crates/secrets/src/lib.rs`: vault data model, keychain read/write helpers, env-first lookup, legacy import helpers, tests.
- Modify `apps/desktop/src-tauri/src/secrets.rs`: startup prefetch uses env/vault only; debug default remains off.
- Create `apps/desktop/src-tauri/src/commands/provider_keys.rs`: Tauri command layer for list/save/remove/test/import.
- Modify `apps/desktop/src-tauri/src/commands/mod.rs`: export the new command module.
- Modify `apps/desktop/src-tauri/src/lib.rs`: register provider-key commands.
- Create `apps/desktop/src/app/ProviderKeysSettings.tsx`: Advanced settings panel for provider rows.
- Modify `apps/desktop/src/app/SettingsModal.tsx`: add Provider Keys under Advanced.
- Create `apps/desktop/tests/provider-keys-settings.test.tsx` if an existing TSX test harness supports React rendering; otherwise add a JSX-free model test at `apps/desktop/tests/provider-keys-settings-model.test.ts`.

## Task 1: Vault Model and Env-First Lookup

**Files:**
- Modify: `crates/secrets/src/lib.rs`

- [ ] **Step 1: Write failing vault tests**

Add these tests inside `#[cfg(test)] mod tests` in `crates/secrets/src/lib.rs`:

```rust
#[test]
fn vault_round_trip_redacts_and_finds_provider() {
    let mut vault = SecretVault::default();
    vault.set(accounts::OPENROUTER_API_KEY, "sk-openrouter");
    assert_eq!(vault.get(accounts::OPENROUTER_API_KEY), Some("sk-openrouter"));
    assert_eq!(vault.status(accounts::OPENROUTER_API_KEY), ProviderSecretStatus::Configured);
    let json = vault.to_json().unwrap();
    let parsed = SecretVault::from_json(&json).unwrap();
    assert_eq!(parsed.get(accounts::OPENROUTER_API_KEY), Some("sk-openrouter"));
}

#[test]
fn env_lookup_wins_over_vault_lookup() {
    let mut vault = SecretVault::default();
    vault.set(accounts::HF_TOKEN, "vault-hf");
    let resolved = resolve_from_env_or_vault(Some("env-hf"), &vault, accounts::HF_TOKEN);
    assert_eq!(resolved.as_deref(), Some("env-hf"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p montage-secrets vault_round_trip_redacts_and_finds_provider env_lookup_wins_over_vault_lookup
```

Expected: compile failure for missing `SecretVault`, `ProviderSecretStatus`, and `resolve_from_env_or_vault`.

- [ ] **Step 3: Implement vault types and pure lookup**

Add near the top of `crates/secrets/src/lib.rs`:

```rust
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
```

Add after `SecretError`:

```rust
pub const VAULT_ACCOUNT: &str = "secrets_vault";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretVault {
    pub version: u32,
    #[serde(default)]
    pub providers: BTreeMap<String, VaultSecret>,
}

impl Default for SecretVault {
    fn default() -> Self {
        Self {
            version: 1,
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSecret {
    pub value: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSecretStatus {
    NotSet,
    Configured,
}

impl SecretVault {
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn get(&self, account: &str) -> Option<&str> {
        self.providers.get(account).map(|entry| entry.value.as_str())
    }

    pub fn set(&mut self, account: &str, value: &str) {
        self.providers.insert(
            account.to_string(),
            VaultSecret {
                value: value.to_string(),
                updated_at: "1970-01-01T00:00:00Z".to_string(),
            },
        );
    }

    pub fn remove(&mut self, account: &str) {
        self.providers.remove(account);
    }

    pub fn status(&self, account: &str) -> ProviderSecretStatus {
        if self.get(account).is_some() {
            ProviderSecretStatus::Configured
        } else {
            ProviderSecretStatus::NotSet
        }
    }
}

pub fn resolve_from_env_or_vault(
    env_value: Option<&str>,
    vault: &SecretVault,
    account: &str,
) -> Option<String> {
    env_value
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| vault.get(account).map(str::to_string))
}
```

- [ ] **Step 4: Run tests and verify pass**

Run:

```bash
cargo test -p montage-secrets vault_round_trip_redacts_and_finds_provider env_lookup_wins_over_vault_lookup
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/secrets/src/lib.rs
git commit -m "Add provider secrets vault model"
```

## Task 2: Keychain Vault Storage and Legacy Import API

**Files:**
- Modify: `crates/secrets/src/lib.rs`

- [ ] **Step 1: Write failing storage API tests with injectable backend**

Add a private test-only backend abstraction so tests do not touch the user's Keychain. Add tests first:

```rust
#[test]
fn load_vault_returns_default_when_keychain_entry_missing() {
    let backend = MemorySecretBackend::default();
    let vault = load_vault_with_backend(&backend).unwrap();
    assert_eq!(vault, SecretVault::default());
}

#[test]
fn save_then_load_vault_uses_single_keychain_account() {
    let backend = MemorySecretBackend::default();
    let mut vault = SecretVault::default();
    vault.set(accounts::DEEPGRAM_API_KEY, "dg-secret");
    save_vault_with_backend(&backend, &vault).unwrap();
    assert_eq!(backend.accounts(), vec![VAULT_ACCOUNT.to_string()]);
    let loaded = load_vault_with_backend(&backend).unwrap();
    assert_eq!(loaded.get(accounts::DEEPGRAM_API_KEY), Some("dg-secret"));
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
cargo test -p montage-secrets load_vault_returns_default_when_keychain_entry_missing save_then_load_vault_uses_single_keychain_account
```

Expected: compile failure for missing backend helpers and `DEEPGRAM_API_KEY`.

- [ ] **Step 3: Implement account constant and backend helpers**

Add `DEEPGRAM_API_KEY` to both account and env modules:

```rust
pub const DEEPGRAM_API_KEY: &str = "deepgram_api_key";
pub const DEEPGRAM_API_KEY: &str = "DEEPGRAM_API_KEY";
```

Add public vault functions:

```rust
pub fn load_vault() -> Result<SecretVault, SecretError> {
    let entry = keyring::Entry::new(SERVICE, VAULT_ACCOUNT).map_err(|e| SecretError::Backend {
        account: VAULT_ACCOUNT.into(),
        source: e,
    })?;
    match entry.get_password() {
        Ok(raw) => SecretVault::from_json(&raw).map_err(|e| SecretError::CorruptVault {
            message: e.to_string(),
        }),
        Err(keyring::Error::NoEntry) => Ok(SecretVault::default()),
        Err(e) => Err(SecretError::Backend {
            account: VAULT_ACCOUNT.into(),
            source: e,
        }),
    }
}

pub fn save_vault(vault: &SecretVault) -> Result<(), SecretError> {
    let raw = vault.to_json().map_err(|e| SecretError::CorruptVault {
        message: e.to_string(),
    })?;
    let entry = keyring::Entry::new(SERVICE, VAULT_ACCOUNT).map_err(|e| SecretError::Backend {
        account: VAULT_ACCOUNT.into(),
        source: e,
    })?;
    entry.set_password(&raw).map_err(|e| SecretError::Backend {
        account: VAULT_ACCOUNT.into(),
        source: e,
    })
}
```

Extend `SecretError`:

```rust
#[error("provider key vault is corrupt: {message}")]
CorruptVault { message: String },
```

Implement test-only `MemorySecretBackend`, `load_vault_with_backend`, and `save_vault_with_backend` inside the test module.

- [ ] **Step 4: Run tests and verify pass**

Run:

```bash
cargo test -p montage-secrets load_vault_returns_default_when_keychain_entry_missing save_then_load_vault_uses_single_keychain_account
```

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/secrets/src/lib.rs
git commit -m "Store provider secrets in one keychain vault"
```

## Task 3: Replace Generic Lookup with Env Then Vault

**Files:**
- Modify: `crates/secrets/src/lib.rs`
- Modify: `apps/desktop/src-tauri/src/secrets.rs`

- [ ] **Step 1: Write failing lookup precedence test**

Add:

```rust
#[test]
fn get_with_loaded_vault_does_not_read_legacy_keychain() {
    let mut vault = SecretVault::default();
    vault.set(accounts::PEXELS_API_KEY, "vault-pexels");
    let backend = MemorySecretBackend::default();
    let value = get_with_backend_and_vault(
        &backend,
        None,
        env_vars::PEXELS_API_KEY,
        accounts::PEXELS_API_KEY,
        &vault,
    )
    .unwrap();
    assert_eq!(value.as_deref(), Some("vault-pexels"));
    assert!(backend.read_log().is_empty());
}
```

- [ ] **Step 2: Run test and verify failure**

Run:

```bash
cargo test -p montage-secrets get_with_loaded_vault_does_not_read_legacy_keychain
```

Expected: compile failure for missing `get_with_backend_and_vault`.

- [ ] **Step 3: Implement cached vault lookup path**

Add a `OnceLock<Result<SecretVault, String>>` cache in `crates/secrets/src/lib.rs`, then update `get` so it:

```rust
pub fn get(env_var_name: &str, account: &str) -> Result<Option<String>, SecretError> {
    if let Ok(value) = std::env::var(env_var_name)
        && !value.is_empty()
    {
        trace!(env_var = env_var_name, "secret resolved from env var");
        return Ok(Some(value));
    }

    let vault = cached_vault()?;
    if let Some(value) = vault.get(account) {
        trace!(account, "secret resolved from vault");
        return Ok(Some(value.to_string()));
    }

    Ok(None)
}
```

Keep legacy reads in a separate public function:

```rust
pub fn get_legacy_keychain(env_var_name: &str, account: &str) -> Result<Option<String>, SecretError>
```

Use `get_legacy_keychain` only for explicit import flows.

- [ ] **Step 4: Update desktop startup prefetch**

In `apps/desktop/src-tauri/src/secrets.rs`, keep `resolve_at_startup` as env/vault only. Do not call legacy fallback from startup.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p montage-secrets get_with_loaded_vault_does_not_read_legacy_keychain
CARGO_INCREMENTAL=0 cargo test -p montage-desktop secrets::tests::
```

Expected: both pass.

- [ ] **Step 6: Commit**

```bash
git add crates/secrets/src/lib.rs apps/desktop/src-tauri/src/secrets.rs
git commit -m "Resolve provider keys from env or vault"
```

## Task 4: Desktop Provider Key Commands

**Files:**
- Create: `apps/desktop/src-tauri/src/commands/provider_keys.rs`
- Modify: `apps/desktop/src-tauri/src/commands/mod.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing command tests**

In `provider_keys.rs`, add tests with a mock backend or temp in-memory vault:

```rust
#[test]
fn provider_rows_redact_configured_keys() {
    let mut vault = montage_secrets::SecretVault::default();
    vault.set(montage_secrets::accounts::OPENROUTER_API_KEY, "sk-1234567890");
    let rows = provider_rows_from_vault(&vault);
    let row = rows.iter().find(|row| row.key == "openrouter").unwrap();
    assert_eq!(row.status, ProviderKeyStatus::Configured);
    assert_eq!(row.redacted.as_deref(), Some("sk-...7890"));
}
```

- [ ] **Step 2: Run and verify failure**

Run:

```bash
CARGO_INCREMENTAL=0 cargo test -p montage-desktop provider_rows_redact_configured_keys
```

Expected: compile failure for missing module/types.

- [ ] **Step 3: Implement commands**

Create `provider_keys.rs` with:

```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyRow {
    pub key: &'static str,
    pub label: &'static str,
    pub account: &'static str,
    pub env_var: &'static str,
    pub capability: &'static str,
    pub status: ProviderKeyStatus,
    pub redacted: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKeyStatus {
    NotSet,
    Configured,
}

#[tauri::command]
pub fn list_provider_keys() -> Result<Vec<ProviderKeyRow>, String> {
    let vault = montage_secrets::load_vault().map_err(|e| e.to_string())?;
    Ok(provider_rows_from_vault(&vault))
}
```

Also add `save_provider_key(provider: String, value: String)`, `remove_provider_key(provider: String)`, and `import_legacy_provider_keys()` commands. Keep `test_provider_key` as a no-network placeholder that validates non-empty shape in this phase.

- [ ] **Step 4: Register commands**

Add to `commands/mod.rs`:

```rust
pub mod provider_keys;
```

Add to `lib.rs` invoke handler:

```rust
commands::provider_keys::list_provider_keys,
commands::provider_keys::save_provider_key,
commands::provider_keys::remove_provider_key,
commands::provider_keys::import_legacy_provider_keys,
```

- [ ] **Step 5: Run tests**

```bash
CARGO_INCREMENTAL=0 cargo test -p montage-desktop provider_rows_redact_configured_keys
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/provider_keys.rs apps/desktop/src-tauri/src/commands/mod.rs apps/desktop/src-tauri/src/lib.rs
git commit -m "Add provider key desktop commands"
```

## Task 5: Advanced Settings UI

**Files:**
- Create: `apps/desktop/src/app/ProviderKeysSettings.tsx`
- Modify: `apps/desktop/src/app/SettingsModal.tsx`
- Test: `apps/desktop/tests/provider-keys-settings-model.test.ts`

- [ ] **Step 1: Write failing JSX-free model test**

Create `apps/desktop/tests/provider-keys-settings-model.test.ts`:

```ts
import { strict as assert } from "node:assert";
import { providerKeyStatusLabel, type ProviderKeyRow } from "../src/app/providerKeysSettingsModel.ts";

const configured: ProviderKeyRow = {
  key: "openrouter",
  label: "OpenRouter",
  account: "openrouter_api_key",
  envVar: "OPENROUTER_API_KEY",
  capability: "Generated media",
  status: "configured",
  redacted: "sk-...7890",
};

assert.equal(providerKeyStatusLabel(configured), "Configured");
assert.equal(providerKeyStatusLabel({ ...configured, status: "notSet", redacted: null }), "Not set");
console.log("provider-keys-settings-model: OK");
```

- [ ] **Step 2: Run and verify failure**

Run:

```bash
node --experimental-strip-types apps/desktop/tests/provider-keys-settings-model.test.ts
```

Expected: module not found for `providerKeysSettingsModel.ts`.

- [ ] **Step 3: Implement UI model**

Create `apps/desktop/src/app/providerKeysSettingsModel.ts`:

```ts
export type ProviderKeyStatus = "notSet" | "configured";

export type ProviderKeyRow = {
  key: string;
  label: string;
  account: string;
  envVar: string;
  capability: string;
  status: ProviderKeyStatus;
  redacted: string | null;
};

export function providerKeyStatusLabel(row: ProviderKeyRow): string {
  return row.status === "configured" ? "Configured" : "Not set";
}
```

- [ ] **Step 4: Implement Settings panel**

Create `ProviderKeysSettings.tsx` with:

```tsx
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { providerKeyStatusLabel, type ProviderKeyRow } from "./providerKeysSettingsModel";

export function ProviderKeysSettings() {
  const [rows, setRows] = useState<ProviderKeyRow[]>([]);
  const [editing, setEditing] = useState<string | null>(null);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    setRows(await invoke<ProviderKeyRow[]>("list_provider_keys"));
  }

  useEffect(() => {
    void refresh().catch((err) => setError(String(err)));
  }, []);

  async function save(provider: string) {
    await invoke("save_provider_key", { provider, value });
    setValue("");
    setEditing(null);
    await refresh();
  }

  async function remove(provider: string) {
    await invoke("remove_provider_key", { provider });
    await refresh();
  }

  return (
    <div className="grid gap-3">
      {error ? <p className="text-[12px] text-red-300">{error}</p> : null}
      {rows.map((row) => (
        <div key={row.key} className="rounded-lg border border-[var(--glass-border)] p-3">
          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="font-semibold">{row.label}</div>
              <div className="text-[12px] text-[var(--color-text-muted)]">{row.capability}</div>
              <div className="mt-1 font-mono text-[11px] text-[var(--color-text-muted)]">
                {providerKeyStatusLabel(row)} {row.redacted ? `· ${row.redacted}` : ""}
              </div>
            </div>
            <div className="flex gap-2">
              <button type="button" onClick={() => setEditing(row.key)}>
                {row.status === "configured" ? "Replace" : "Add"}
              </button>
              <button type="button" onClick={() => void remove(row.key)} disabled={row.status !== "configured"}>
                Remove
              </button>
            </div>
          </div>
          {editing === row.key ? (
            <div className="mt-3 flex gap-2">
              <input
                type="password"
                value={value}
                onChange={(event) => setValue(event.target.value)}
                placeholder={row.envVar}
              />
              <button type="button" onClick={() => void save(row.key)} disabled={!value.trim()}>
                Save
              </button>
            </div>
          ) : null}
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 5: Wire SettingsModal**

Add `ProviderKeysSettings` import. Add a section id `"advanced"` if not already present, and render:

```tsx
<SettingsCard title="Provider keys" description="Advanced bring-your-own keys for local provider features.">
  <ProviderKeysSettings />
</SettingsCard>
```

- [ ] **Step 6: Run checks**

```bash
node --experimental-strip-types apps/desktop/tests/provider-keys-settings-model.test.ts
apps/desktop/node_modules/.bin/tsc --noEmit -p apps/desktop/tsconfig.json
```

Expected: both pass.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/app/ProviderKeysSettings.tsx apps/desktop/src/app/providerKeysSettingsModel.ts apps/desktop/src/app/SettingsModal.tsx apps/desktop/tests/provider-keys-settings-model.test.ts
git commit -m "Add advanced provider key settings"
```

## Task 6: Friendly Missing-Key Copy and Final Verification

**Files:**
- Modify: missing-key call sites in `crates/core/src/tools/search_broll.rs`, `crates/core/src/tools/use_broll.rs`, `crates/core/src/generated_media/openrouter.rs`, and any Deepgram/HF call sites found with `rg`.
- Modify: desktop UI surfaces only where they display raw provider errors.

- [ ] **Step 1: Find raw key errors**

Run:

```bash
rg "not set|env or keychain|HF_TOKEN|OPENROUTER_API_KEY|PEXELS_API_KEY|DEEPGRAM" crates apps/desktop/src apps/desktop/src-tauri
```

Record each user-facing raw env-var error.

- [ ] **Step 2: Replace user-facing copy**

Use copy shaped like:

```text
OpenRouter is needed for generated media. Add your OpenRouter key in Settings -> Advanced -> Provider Keys.
```

Keep env-var names only in developer logs and docs.

- [ ] **Step 3: Run full verification**

Run:

```bash
cargo test -p montage-secrets
CARGO_INCREMENTAL=0 cargo test -p montage-desktop provider_keys secrets::tests::
node --experimental-strip-types apps/desktop/tests/provider-keys-settings-model.test.ts
apps/desktop/node_modules/.bin/tsc --noEmit -p apps/desktop/tsconfig.json
git diff --check
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates apps/desktop
git commit -m "Use friendly provider key setup copy"
```

## Self-Review Notes

- Spec coverage: single vault, Advanced UI, env override, no startup legacy probing, explicit import, missing-key prompts, and no managed credits are all covered by tasks.
- No placeholders remain; legacy import implementation is explicitly scoped to Task 4 command layer.
- Type consistency: provider status uses Rust `Configured` / `NotSet` serialized as camelCase and TypeScript `"configured"` / `"notSet"`.
