# Phase 2: Server-side OAuth exchange + encrypted token storage (YouTube first)

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** [1]

## Prerequisites
- [USER] Create a Google Cloud project, configure the OAuth consent screen, and create an OAuth 2.0 Web client (client_id + client_secret) with the server callback URL as an authorized redirect URI. The desktop/native flow cannot hold the secret.
- [USER] Add the YouTube Data API v3 scope (youtube.upload) and the channels read scope needed to resolve channel identity; submit the Google OAuth verification/TOS audit (long pole — start now; uploads stay private until it passes).
- [USER] Provision the runtime secret store on the Phase-1 deployment target (Supabase secret / KMS-injected env) for GOOGLE_CLIENT_SECRET and a 32-byte SOCIAL_TOKEN_AEAD_KEY; decide key rotation policy/key id.
- [WE-STAGE] Generate a dev 32-byte AEAD key for local builds and wire it through env in commands/social.rs key_provider().
- [WE-STAGE] Add chacha20poly1305 + rand + reqwest + tokio to crates/social/Cargo.toml (all already present in workspace/lock).
- [WE-STAGE] Choose and add an HTTP mocking approach for exchange/refresh tests (wiremock or hand-rolled), or gate live exchange tests behind #[ignore].

## Plan

## Context (verified against the actual tree)

What already exists and is tested, to be reused (do NOT reimplement):
- `crates/social/src/oauth_url.rs` — builds the Google authorize URL with `access_type=offline` + `prompt=consent` (guarantees a refresh token). Keep as-is.
- `crates/social/src/oauth.rs` — `OAuthConnection` with hashed `state`, `validate_callback` (expiry/state/already-completed). Keep as-is; it is the CSRF/replay guard for the callback.
- `crates/social/src/token_bundle.rs` — `ProviderTokenBundle::from_oauth_response` already normalizes `expires_in`/`refresh_expires_in`/scopes into absolute expiries. This is exactly the struct the real exchange must produce. Reuse.
- `crates/social/src/account_service.rs::SocialAccountService::complete_oauth` — already takes pre-exchanged `access_token` + `refresh_token` + `token_bundle`, encrypts via `TokenSecret::encrypt`, persists account+secret, marks the connection `Completed`, and enforces account/provider/bundle consistency. Reuse unchanged except the key-provider type (see Step 3).
- `crates/social/src/api.rs::SocialApi::oauth_complete` — the facade seam that forwards tokens to the account service. Its DTO `OAuthCompleteRequest` already carries `access_token`/`refresh_token`. Reuse.
- `crates/social/src/token.rs` — `TokenSecret` struct + `LocalTokenKeyProvider` trait + XOR `encode_with_key/decode_with_key`. The struct shape (`encrypted_access_token`, `encrypted_refresh_token`, `*_expires_at`, `token_version`, `kms_key_id`, `last_refreshed_at`) is correct and persisted by `sqlite_store.rs`. Only the crypto primitive and the key-provider semantics change.
- `crates/social/src/store.rs` + `sqlite_store.rs` — `save_token_secret` / `token_secret_for_account` persist `TokenSecret` as JSON in `oauth_token_secrets`. Reuse.

What is a STUB to be replaced:
- `apps/desktop/src-tauri/src/commands/social.rs::social_oauth_complete` — fabricates `stub-access-*` / `stub-refresh-*` tokens and a deterministic bundle (lines ~169-191). No HTTP exchange.
- `crates/social/src/token.rs` — XOR with caller-supplied raw key (no integrity, hardcoded desktop key in `commands/social.rs::key_provider()`).
- No token-refresh code anywhere; `youtube_upload.rs` clients take an opaque `access_token_ref` but nothing refreshes it. `ConnectedAccountStatus::NeedsReauth`/`Revoked` exist in `model.rs` but nothing transitions accounts into them.

Decision locked for this phase (resolving the spec's open question):
- **Encryption mechanism: app-level AEAD (ChaCha20-Poly1305) with a 32-byte data-encryption key supplied from a Supabase secret / KMS-injected env var.** Rationale: `chacha20poly1305` is already in `Cargo.lock` (transitive via `keyring`'s `crypto-rust`), so no new vendor review; it is authenticated (fixes the XOR-no-integrity hole); and it keeps encryption inside the tested Rust crate so the existing token-safety tests and the `LocalTokenKeyProvider` seam still apply. Supabase Vault/pgsodium is rejected for the token path because the worker is the Rust service, not SQL, and we do not want plaintext tokens to transit Postgres functions. `kms_key_id` records which key version encrypted each row to allow rotation.

This phase depends on Phase 1 only for two externally-decided facts: (a) where the Rust service runs and how it injects secrets/env (the AEAD key + Google `client_secret`), and (b) the invocation shape (HTTP vs queue). All crate-level code below is buildable and unit-testable in isolation against `SqliteSocialStore`/`InMemorySocialStore` without Phase 1 infra, then wired to the service entrypoint Phase 1 defines.

---

## Step 1 — Add authenticated encryption to `crates/social/src/token.rs`

Files: `crates/social/src/token.rs`, `crates/social/Cargo.toml`, `Cargo.toml` (workspace deps table).

1. In root `Cargo.toml` `[workspace.dependencies]`, add `chacha20poly1305 = "0.10"` and `rand = "0.9"` (rand already pinned in the workspace at line 483 — reference it). In `crates/social/Cargo.toml` add both under `[dependencies]` (`{ workspace = true }`).
2. In `token.rs`, introduce a new trait `TokenEncryptionKey` (or repurpose `LocalTokenKeyProvider`) that yields `key_id() -> &str` and `key_bytes() -> &[u8; 32]`. Keep the existing `LocalTokenKeyProvider` trait name to avoid churn across `account_service.rs`/`api.rs`/`commands/social.rs` call sites, but change `key_material()` semantics to "must be exactly 32 bytes" and add a constructor `Aead256Key::from_secret(key_id, &[u8;32])`.
3. Replace `encode_with_key`/`decode_with_key`:
   - `encrypt`: generate a fresh 12-byte nonce via `rand`, ChaCha20-Poly1305 seal the plaintext token, store `base64(nonce || ciphertext||tag)` in `encrypted_access_token` / `encrypted_refresh_token`. Set `token_version = 2` (XOR was v1) and `kms_key_id = key.key_id()`.
   - Add `decrypt_access_token` (rewrite) and a NEW `decrypt_refresh_token(&self, key) -> Result<Option<String>, TokenError>` — required by the refresh path; currently absent.
4. Extend `TokenError` with `EncryptionFailed`/`DecryptionFailed` (auth-tag failure) and `BadKeyLength`.
5. Keep `TestKeyProvider` for unit tests but make its `key_material` derive/pad to 32 bytes deterministically so existing tests compile.

Verification:
- `cargo test -p montage-social token::` — port the 5 existing token tests; add: round-trip with AEAD; tamper a ciphertext byte and assert `DecryptionFailed`; assert two encryptions of the same plaintext differ (nonce randomization); assert serialized `TokenSecret` JSON still contains no plaintext (the existing `token_secret_serialization_does_not_include_plaintext_tokens` test must stay green).

## Step 2 — Build the real Google/YouTube OAuth token exchange + refresh client

Files: new `crates/social/src/oauth_exchange.rs` (declared in `crates/social/src/lib.rs`), `crates/social/Cargo.toml`.

1. Define a provider-neutral trait `OAuthTokenExchange` with two methods returning the existing types:
   - `exchange_code(provider, config, client_secret, code, redirect_uri, now) -> Result<ExchangedTokens, OAuthExchangeError>`
   - `refresh(provider, config, client_secret, refresh_token, now) -> Result<ExchangedTokens, OAuthExchangeError>`
   where `ExchangedTokens { access_token: String, refresh_token: Option<String>, bundle: ProviderTokenBundle }`. Build `bundle` by feeding the provider's JSON (`expires_in`, granted `scope`) through the existing `ProviderTokenBundle::from_oauth_response` — reuse, do not duplicate expiry math.
2. Implement `GoogleOAuthExchange` (reqwest async) hitting `https://oauth2.googleapis.com/token`:
   - exchange: `grant_type=authorization_code`, `code`, `client_id`, `client_secret`, `redirect_uri`.
   - refresh: `grant_type=refresh_token`, `refresh_token`, `client_id`, `client_secret`. Note Google omits a new `refresh_token` on refresh — carry the prior one forward (return `refresh_token: None` and have the caller keep the stored one).
   - **Per G3 — MANDATORY prerequisite edit (not just a risk):** add a YouTube read scope (`https://www.googleapis.com/auth/youtube.readonly`, or `youtube.force-ssl`) to `scopes_for(YouTube)` in `crates/social/src/oauth_url.rs` (currently only `youtube.upload` at line 48). Without it the next bullet's call is unauthorized and `complete_oauth` rejects the connection.
   - Resolve channel identity (`provider_account_id`) by calling YouTube Data API `channels?part=id&mine=true` with the new access token, since Google's token response has no channel id; this fills `ProviderTokenBundle.provider_account_id` to satisfy `complete_oauth`'s consistency check.
   - Map HTTP/JSON failures and `invalid_grant` (revoked/expired refresh) into a distinct `OAuthExchangeError::RefreshRejected` so callers can mark `NeedsReauth`.
3. `client_secret` is passed as a parameter, never stored in this crate — it lives only in the service process env (Phase 1 injection).

Reuse: `reqwest` is already a workspace dep with `json`+`rustls-tls` (root `Cargo.toml` line 368). Add `reqwest`, `tokio` (for async), to `crates/social/Cargo.toml`.

Verification:
- `cargo test -p montage-social oauth_exchange::` against a mock HTTP server (use a lightweight stub: a `wiremock`-style server or a hand-rolled `tokio` listener; if adding `wiremock` is undesirable, gate live tests behind `#[ignore]` and unit-test the request-building + JSON parsing pure functions). Tests: successful exchange yields a bundle with correct absolute expiries; refresh with `invalid_grant` body → `RefreshRejected`; missing `refresh_token` on first exchange → error (Google should always return one given `prompt=consent`).

## Step 3 — Add the token-refresh service + `NeedsReauth` transition

Files: new `crates/social/src/token_refresh.rs` (declared in `lib.rs`), `crates/social/src/store.rs`, `crates/social/src/sqlite_store.rs`, `crates/social/src/model.rs` (no enum change — `NeedsReauth`/`Revoked` already exist).

1. Add a store method to flip account status without disabling it. The trait currently has only `disable_connected_account`; `save_connected_account` is an upsert and can be reused, but add an explicit `set_connected_account_status(id, owner, status, now)` to `SocialStore` for clarity and to keep the owner check. Implement in both `InMemorySocialStore` and `SqliteSocialStore` (the sqlite `connected_account_status_as_str` mapping at line 703 already handles all variants).
2. Implement `TokenRefreshService::ensure_fresh_access_token(store, exchange, key, config, client_secret, account_id, skew_secs, now) -> Result<String, TokenRefreshError>`:
   - Load `TokenSecret` via `token_secret_for_account`.
   - If `access_token_expires_at` is more than `skew_secs` in the future, decrypt and return it (reuse `decrypt_access_token`).
   - Else decrypt the refresh token (`decrypt_refresh_token` from Step 1), call `exchange.refresh(...)`, re-encrypt via `TokenSecret::encrypt`, preserve the existing refresh token when the provider returned none, set `last_refreshed_at = now`, `save_token_secret`.
   - On `OAuthExchangeError::RefreshRejected`: call `set_connected_account_status(..., NeedsReauth, now)`, append a `PublishJobEvent`-style audit (or account audit) noting refresh failure, and return `TokenRefreshError::NeedsReauth` so the worker stops hammering the provider (spec error-handling requirement).
3. Add `TokenRefreshService::refresh_due_secrets(store, exchange, key, ..., horizon, now)` — a sweep that selects accounts whose `access_token_expires_at` falls within `horizon` and proactively refreshes them. This is the unit of work the Phase 4 `pg_cron` token-refresh sweep will call. (Phase 2 ships the capability; Phase 4 schedules it.)

Verification:
- `cargo test -p montage-social token_refresh::` with a fake `OAuthTokenExchange` impl + `InMemorySocialStore`: (a) non-expired token returns without calling exchange; (b) expired token triggers refresh, re-encrypts, persists new expiry; (c) refresh returning `None` refresh_token keeps the old one decryptable; (d) `RefreshRejected` flips account to `NeedsReauth` and returns the typed error and does not re-call exchange on the next tick.

## Step 4 — Wire the real key + exchange into the facade callers

Files: `crates/social/src/api.rs` (light), `apps/desktop/src-tauri/src/commands/social.rs`, plus the Phase-1 service entrypoint (whatever module Phase 1 creates for the HTTP/cron glue).

1. `SocialApi::oauth_complete` already accepts the exchanged tokens; the only change is the key-provider trait bound now being the AEAD key from Step 1 (same trait name, so signature is unchanged). No facade logic change needed — confirm the existing `api.rs` oauth tests still pass with the AEAD `TestKeyProvider`.
2. In `apps/desktop/src-tauri/src/commands/social.rs::social_oauth_complete`, replace the stub bundle/tokens block (lines ~169-191): the desktop is a thin client and must NOT do the exchange (no `client_secret`). Per the architecture, the exchange happens on the server's callback. For this phase, change the command to call the server's OAuth-complete endpoint (the Phase 1 service) rather than fabricating tokens locally; if the desktop-rewire is deferred to Phase 5, gate the stub behind a clearly-named `cfg`/feature and leave a `// TODO(phase5): replace with server call` so the stub never ships as the real path. Update `key_provider()` (lines ~49-51) to construct an `Aead256Key` from an env-injected dev key rather than the hardcoded string, so local builds exercise real AEAD.
3. Add the server-side OAuth callback handler in the Phase-1 service module: validate connection (`OAuthConnection::validate_callback` — reuse), call `GoogleOAuthExchange::exchange_code` (Step 2), then `SocialApi::oauth_complete` with the AEAD key (Step 1). This is where `client_secret` is read from env.

Verification:
- `cargo test -p montage-social api::account_api_starts_and_completes_oauth` and `api_round_trips_account_routes_with_sqlite_store` stay green (these already assert no `access-secret`/`refresh-secret` leaks).
- `cargo test -p montage-desktop` (or the tauri crate's `commands::social::tests`) — `oauth_complete_then_disconnect_round_trips_without_tokens` must stay green; add a test asserting the dev key is 32 bytes and AEAD round-trips.
- Build the whole workspace: `cargo build` and `cargo clippy --workspace` (lints are workspace-enforced).

## Step 5 — Mark the encryption-version migration / rotation path

Files: `crates/social/src/token.rs` (read path), `crates/social/src/sqlite_store.rs` (no schema change — `payload_json` already stores the full `TokenSecret`).

1. The decrypt path must reject `token_version == 1` (old XOR) with a typed `TokenError::UnsupportedTokenVersion` rather than silently mis-decrypting. Since the only existing token rows are dev stubs, no data migration is required; document that any pre-existing local `social.sqlite` token rows must be reconnected. `kms_key_id` carries the key version so a future key rotation can re-encrypt without schema change.

Verification:
- `cargo test -p montage-social` full suite green; add a test that a `token_version: 1` payload decrypts to `UnsupportedTokenVersion`.

## Step 6 — Documentation + secret inventory

Files: append to `docs/superpowers/specs/2026-06-03-social-publishing-server-architecture-design.md` is read-only-blocked here; instead record in the phase plan (this doc) and the service's README created in Phase 1.

1. Enumerate the secrets the service needs at runtime: `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, `SOCIAL_TOKEN_AEAD_KEY` (32 bytes, base64), `SOCIAL_TOKEN_KEY_ID`. These are injected by the Phase-1 deployment target; this phase only reads them.

Verification:
- Manual: confirm none of these are committed; `git grep` for the literal values returns nothing.

---

## Reuse vs new summary

Reused unchanged: `oauth_url.rs`, `oauth.rs`, `token_bundle.rs`, `account_service.rs::complete_oauth` body, `SocialApi::oauth_complete` shape, `sqlite_store` token persistence, all existing token-safety tests.

New: `oauth_exchange.rs` (real Google exchange/refresh), `token_refresh.rs` (refresh service + sweep), AEAD crypto inside `token.rs` (replacing XOR), `decrypt_refresh_token`, `set_connected_account_status` store method, server callback handler (lands in the Phase-1 service module).

Replaced/removed: XOR `encode_with_key`/`decode_with_key`; the `stub-access`/`stub-refresh` fabrication in `commands/social.rs`; the hardcoded desktop key.

## Open risks
- Where the server callback handler physically lives depends on Phase 1's invocation-shape decision (axum HTTP service vs Edge Function calling the Rust service). The crate-level exchange/refresh/encryption code is decoupled and testable regardless, but the callback wiring (Step 4.3) cannot be finalized until Phase 1 lands.
- Google does not return a new refresh_token on refresh and may not return one on re-consent if a prior grant exists; the access_type=offline + prompt=consent in oauth_url.rs mitigates first-exchange, but we must carry forward the stored refresh token on refresh and handle the case where Google rotates/omits it.
- Channel identity resolution adds a second Google API call (channels?mine=true) during exchange; if the granted scope set does not permit it, provider_account_id resolution fails and complete_oauth's consistency check rejects the connection. Confirm the read scope is requested in oauth_url.rs scopes_for(YouTube).
- AEAD key rotation: kms_key_id records the encrypting key, but no re-encrypt-on-rotation job is built this phase; rotating the key strands existing rows until reconnect or a future migration sweep.
- Desktop oauth_complete stub removal vs Phase 5 desktop rewire: if Phase 5 is not yet done, the desktop path must call the server endpoint for completion, which may not exist until Phase 1 deploys — interim the stub may need to stay behind a feature flag, risking the real path not being exercised end-to-end until both phases land.
- Token-version gate (Step 5) rejects v1 XOR rows; any local social.sqlite with stub tokens must be reconnected — acceptable for dev, but note no automated migration.
