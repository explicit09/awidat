# Phase 5: Desktop client rewire: OAuth-via-browser, upload-to-server, poll status (thin HTTPS client, no secrets)

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** [1, 2, 3, 4]

## Prerequisites
- [USER] Phases 1-4 must be deployed and reachable: a running montage-social HTTP service with the OAuth start/callback endpoints, schedule/job/audit routes, an upload-URL handshake endpoint backed by Supabase Storage, and pg_cron firing jobs. Phase 5 is a client of these.
- [USER] Server-side OAuth app credentials (Google/YouTube client_id + client_secret + an HTTPS redirect_uri pointing at the server callback) must be configured ON THE SERVER. The desktop must NOT receive client_secret.
- [USER] A way for the desktop to authenticate to the server (bearer token / dev token the server accepts) until Phase 7 Supabase Auth lands. Provide the dev token value.
- [USER] Supabase Storage bucket + policy for rendered-artifact uploads decided in Phase 1 (signed-URL upload vs direct-multipart proxy) so the upload handshake endpoint contract is fixed.
- [WE-STAGE] MONTAGE_SOCIAL_SERVER_URL env/config wiring, the social_client.rs module, command rewrites, frontend rewire, and tests can all be staged and unit-tested with wiremock/fake-invoke before the real server exists; only the step-7 e2e needs the live server.

## Plan

## Goal

Turn the desktop's social-publishing path into a thin authenticated HTTPS client of the `montage-social` server (Phases 1-4). The desktop must:
- initiate OAuth by opening the system browser to the **server's** auth-start URL (no client_id/redirect_uri/client_secret on the desktop),
- upload the rendered file to server-side storage (Supabase Storage signed URL, with a direct-multipart fallback),
- schedule jobs and poll job status,
- hold **no secrets** and run **no local OAuth listener / no local provider upload**.

The desktop keeps its own local SQLite for editing/project state (timeline, proposals, render queue) — unchanged. Only the *publishing* domain moves to the server.

## Current state (verified in repo)

There are TWO publishing code paths in the desktop today; Phase 5 must converge them onto the server client:

1. **`montage-social`-backed path** (the one the design's "desktop client" maps to):
   - `apps/desktop/src-tauri/src/commands/social.rs` — 14 `social_*` Tauri commands that lock a file-backed `SqliteSocialStore` in `MontageState.social` and call `SocialApi` *in-process*. Uses stub tokens (`format!("stub-access-{}", ...)`), `MockUploadAdapter`, and `MockReadyStatus`. Holds a `TestKeyProvider` token-encryption key locally.
   - Registered in `apps/desktop/src-tauri/src/lib.rs` (lines ~274-287) and the store is opened in the `.setup()` hook (lines ~114-128) at `<data_dir>/social.sqlite`.
   - Frontend: `apps/desktop/src/app/social/SocialAccounts.tsx`, `SocialJobs.tsx`, `SocialSchedule.tsx`, `SocialAudit.tsx`, `socialModel.ts`. `SocialAccounts.tsx` already calls `social_oauth_start` then `openUrl(start.authorizationUrl)` and sends placeholder `clientId: "desktop-local"` + a localhost `redirectUri` — these placeholders move server-side.
   - Campaign path: `apps/desktop/src/campaign/publisher.ts` + `apps/desktop/src/shell/delivery/CampaignApprovalPanel.tsx` drive `set_render_upload_targets` / `set_upload_metadata` / `compute_ai_disclosure` / `start_uploads_for_job` / `poll_upload_states` — these are the **legacy** render-queue commands, not the `social_*` ones.

2. **Legacy local-secret path** (must be retired/neutralized for the "no secrets" invariant):
   - `apps/desktop/src-tauri/src/publishing/` (`oauth_exchange.rs`, `oauth_listener.rs`, `keychain.rs`, `youtube_upload.rs`, `upload_queue.rs`, `youtube.rs`, `tiktok.rs`, `instagram.rs`, `storage.rs`, `provider.rs`) — real OAuth code exchange, local loopback listener, OS-keychain token storage, real YouTube resumable upload, and a local `UploadQueue` (referenced by `MontageState.upload_queue` in `state.rs`).
   - `apps/desktop/src-tauri/src/commands/publishing.rs` wires those + the render-queue auto-upload.

This phase's invariant ("client_secret + refresh tokens live ONLY on the server") means the legacy `publishing/` secret-holding pieces (`oauth_exchange`, `oauth_listener`, `keychain`, provider upload clients) must stop being the live path. Decision below: gate them off behind a build flag this phase (lowest-risk, keeps tests green) and delete in Phase 6/7 cleanup.

## Design decisions for this phase

- **Server base URL + desktop auth token** come from config, never hardcoded. Add a `social_server` block to the existing desktop config (`apps/desktop/src-tauri/src/commands/config.rs`) with `base_url` and a desktop session/auth token. For pre-multi-user (Phase 7 brings real Supabase Auth), use a static dev bearer the server also accepts; the field exists now so Phase 7 only swaps the value source.
- **Rust HTTP client**: reuse the workspace `reqwest` already declared in `apps/desktop/src-tauri/Cargo.toml` (line 62). Add one new module `apps/desktop/src-tauri/src/social_client.rs` (thin typed wrapper: GET/POST JSON + multipart/PUT for upload) — newly built. No new crate.
- **Keep the existing `social_*` command names and their request/response DTOs** so the frontend (`app/social/*`) and `socialModel.ts` change minimally. Command **bodies** change from "lock local store + call `SocialApi`" to "call `SocialClient` → server". The serde DTOs (`AccountSummary`, `OAuthStartResponse`, `PublishJobResponse`, `AccountUsageAudit`, `CampaignVariantTarget`) are **reused verbatim** from `montage-social::api` (re-exported), so client/server share one shape — this is the big reuse win.
- **OAuth start**: desktop no longer sends `clientId`/`redirectUri`/`rawState`. `social_oauth_start` becomes: POST `/social/oauth/start?provider=…` → server returns `{ authorizationUrl }` (server owns client_id, redirect_uri, state). Desktop opens it with `openUrl`. **`social_oauth_complete` is deleted from the desktop** — the provider redirects to the *server* callback; the desktop never sees the `code`. After the browser flow, the desktop re-polls `social_accounts` to discover the newly-connected account.
- **Upload-to-server**: new `social_upload_artifact` command — ask server for a Supabase Storage signed upload URL for the job's artifact, PUT the rendered file bytes to that URL from Rust (streamed, not loaded fully into memory), then tell the server "uploaded" so it stores the storage ref on the job. Fallback: if the server returns `direct: true`, POST the file as multipart to a server endpoint that proxies to storage. The job-firing (provider upload) stays entirely server-side via `pg_cron` (Phase 4).
- **Polling**: `social_publish_job` / `social_account_audit` already return full status; the desktop keeps polling these. **Remove** the desktop "worker" commands `social_execute_upload` and `social_poll_status` (and `MockUploadAdapter`/`MockReadyStatus`) — firing is the server's job now.

## Steps

### 1. Add the server-client config surface
- **Per G6 — there is NO per-field config struct in `commands/config.rs`** (it only exposes indexer-config over `montage_config::Config`). Choose one: (a) read `MONTAGE_SOCIAL_SERVER_URL` (+ a dev `MONTAGE_SOCIAL_AUTH_TOKEN`) from env at command time — simplest, matches how `project_root` defaults from `MONTAGE_DESKTOP_PROJECT` in `state.rs`; or (b) add a `social_server` section to `montage_config::Config` if it must be user-editable/persisted. Default to (a) for this phase. Do NOT follow a "config field pattern in config.rs" — it doesn't exist.
- Verify: `cargo build -p montage-desktop` compiles; a unit test asserting the env-default resolution (or the `montage_config` round-trip if (b) is chosen).

### 2. New `social_client.rs` HTTPS client module
- File (new): `apps/desktop/src-tauri/src/social_client.rs`. A `SocialClient { base_url, auth_token, http: reqwest::Client }` with typed methods returning the **re-exported `montage-social::api` DTOs**:
  - `accounts() -> Vec<AccountSummary>`
  - `oauth_start(provider) -> OAuthStartResponse` (server returns just `authorizationUrl` + connection id)
  - `disconnect_account(account_id) -> AccountSummary`
  - `bind_target(BindTargetRequest) -> CampaignVariantTarget`
  - `validate_target(target_id) -> CampaignVariantTarget`
  - `schedule_target(ScheduleTargetRequest) -> PublishJobResponse`
  - `publish_job(job_id) -> PublishJobResponse`
  - `cancel_job(job_id) -> PublishJobResponse`
  - `retry_job(job_id) -> PublishJobResponse`
  - `account_audit(account_id) -> AccountUsageAudit`
  - `request_upload_url(job_id) -> { url, method, direct }` and `complete_upload(job_id, storage_ref)`
  - `put_file(url, path)` — streamed body via `reqwest::Body::wrap_stream` over a `tokio::fs::File` so multi-GB renders don't load into RAM (mirror the streaming concern already documented in `state.rs` MediaServer comments).
- All methods attach `Authorization: Bearer <auth_token>` and map non-2xx to a stable error string (reuse the `err_string` mapping convention from current `social.rs`, e.g. 401 → `"unauthorized"`).
- Register the module in `apps/desktop/src-tauri/src/lib.rs` (`mod social_client;`).
- Verify: `cargo build`; unit tests in the module's `#[cfg(test)]` using `wiremock` (already a dev-dep pattern used by `publishing/oauth_exchange.rs`) to assert: bearer header is sent, JSON round-trips into the DTOs, and a non-2xx maps to the expected error string. No real network.

### 3. Rewrite `commands/social.rs` bodies to call the client
- File: `apps/desktop/src-tauri/src/commands/social.rs`. Replace every `with_store(...)` body with a `SocialClient` call. Keep command signatures/DTOs so the frontend is undisturbed where possible.
  - `social_providers`: either keep static (it's pure registry data, no secrets) or proxy to server `/social/providers`. Keep static to minimize churn; add a comment that it can move server-side later.
  - `social_accounts` → `client.accounts()`.
  - `social_oauth_start`: drop `client_id`/`redirect_uri`/`raw_state`/`created_at`/`expires_at` from `OAuthStartArgs` (now only `provider`, optional `return_to`); call `client.oauth_start(provider)`.
  - **Delete** `social_oauth_complete` + `OAuthCompleteArgs` + the stub `ProviderTokenBundle`/`key_provider()` construction (the secret-bearing code). The callback is server-side.
  - `social_disconnect_account` → `client.disconnect_account`.
  - `social_bind_target` / `social_validate_target` / `social_schedule_target` / `social_publish_job` / `social_cancel_job` / `social_retry_job` / `social_account_audit` → corresponding client calls.
  - **Delete** `social_execute_upload`, `social_poll_status`, `MockReadyStatus`, and the `MockUploadAdapter`/`PublishService::claim_due_jobs` usage (firing is server-side now).
  - **Add** `social_upload_artifact(job_id, file_path)`: `request_upload_url` → `put_file` (or direct multipart) → `complete_upload`; returns the updated `PublishJobResponse`.
- Get the `SocialClient` from a new `MontageState.social_client: Mutex<Option<SocialClient>>` (replacing the `social: Mutex<Option<SqliteSocialStore>>` slot in `state.rs`). Build it in the `.setup()` hook (lib.rs) from config (step 1) instead of opening `social.sqlite`.
- Verify: rewrite the existing `#[cfg(test)] mod tests` in `social.rs`. The current tests (`accounts_for_local_user_are_token_safe`, `providers_list_has_three_slots`, `oauth_complete_then_disconnect_round_trips_without_tokens`) drive the in-process store and must change: replace with `wiremock`-backed tests asserting the command calls the right server route and that responses carry no token strings (`!json.contains("access_token")`). `providers_list_has_three_slots` stays if `social_providers` remains static.

### 4. Update `state.rs` and `lib.rs`
- File: `apps/desktop/src-tauri/src/state.rs`. Replace field `social: Mutex<Option<SqliteSocialStore>>` (line ~50) with `social_client: Mutex<Option<crate::social_client::SocialClient>>`. Remove the `use montage_social::sqlite_store::SqliteSocialStore;` import. Decide on `upload_queue` (legacy): keep the field for now (still used by render-queue auto-upload) but see step 6.
- File: `apps/desktop/src-tauri/src/lib.rs`. In `.setup()` (lines ~114-128) replace the `SqliteSocialStore::open` block with `SocialClient::from_config(...)` construction. Update `generate_handler!` registration (lines ~274-287): remove `social_oauth_complete`, `social_execute_upload`, `social_poll_status`; add `social_upload_artifact`.
- Verify: `cargo build -p montage-desktop` and `cargo clippy` clean; app boots (`pnpm tauri dev` smoke if available, else build only).

### 5. Frontend rewire (minimal — DTOs unchanged)
- File: `apps/desktop/src/app/social/SocialAccounts.tsx`. Simplify `connect()`: drop the `clientId`/`redirectUri`/`rawState`/`createdAt`/`expiresAt` args; call `social_oauth_start` with just `{ provider }`, then `openUrl(start.authorizationUrl)`, then after the browser returns, re-`refresh()` to pick up the new account (and/or add a "Refresh accounts" affordance, since the callback now lands server-side and the desktop must re-poll). Remove `randomToken("state")` (state is server-owned) but keep `randomToken` only if still needed for connection id; otherwise delete.
- File: `apps/desktop/src/app/social/SocialJobs.tsx`. Remove the "Advance" worker action and `nextWorkerAction`/`social_execute_upload`/`social_poll_status` usage; the only worker is the server. Keep cancel/retry/refresh and the "View post" link. The `advance` button is replaced by passive polling (a `setInterval` refresh while any job is non-terminal).
- File: `apps/desktop/src/app/social/socialModel.ts`. Remove `nextWorkerAction` (and its test coverage). Everything else (labels, `canCancel`, `canRetry`, status counts) is unchanged.
- File: `apps/desktop/src/app/social/SocialSchedule.tsx` / `SocialAudit.tsx`: no logic change (same commands/DTOs); verify they compile after the type tweaks.
- File: `apps/desktop/src/campaign/publisher.ts` + `apps/desktop/src/shell/delivery/CampaignApprovalPanel.tsx`: route campaign publishing through the server. The cleanest mapping that reuses Phase 1-4: for each approved variant, `social_bind_target` → `social_validate_target` → `social_upload_artifact(jobId, filePath)` → `social_schedule_target`, then poll `social_publish_job`. This replaces the legacy `set_render_upload_targets`/`start_uploads_for_job`/`poll_upload_states` chain in `startCampaignUploads`. Keep the AI-disclosure compute as a parameter passed to the server (the server stamps disclosure — the merged safety logic already lives in `montage-social`), so the desktop no longer calls `compute_ai_disclosure` locally for the campaign path.
- Verify (node:assert style, the repo convention — `node --experimental-strip-types tests/*.test.ts`):
  - Update `apps/desktop/tests/campaign-publisher.test.ts` to assert the new invoke sequence (bind/validate/upload/schedule + poll) using the existing fake-`invoke` injection pattern already in that test.
  - Update `apps/desktop/tests/social-model.test.ts` to drop `nextWorkerAction` assertions.
  - Add `apps/desktop/tests/social-accounts-connect.test.ts` (new, node:assert) asserting `connect()` calls `social_oauth_start` with only `{ provider }` and then `openUrl`. Pure-logic extraction may be needed (move the connect side-effect sequence into a testable helper in `socialModel.ts` or a new `socialActions.ts`, JSX-free, matching how `socialModel.ts` is kept JSX-free for testing).
  - Register any new test in `apps/desktop/package.json` `scripts` (e.g. `test:social-accounts`) and add it to the aggregate `test` script, mirroring `test:social-model`.
  - Run: `pnpm --filter montage-desktop test:social-model && pnpm --filter montage-desktop test:campaign-publisher && pnpm --filter montage-desktop test:social-accounts`.

### 6. Retire the legacy local-secret path (no-secrets invariant)
- Files: `apps/desktop/src-tauri/src/publishing/oauth_exchange.rs`, `oauth_listener.rs`, `keychain.rs`, and the provider upload clients (`youtube_upload.rs`, `youtube.rs`, `tiktok.rs`, `instagram.rs`), plus `commands/publishing.rs` render-queue auto-upload.
- This phase: **gate the secret-holding + local-provider-upload code behind `#[cfg(feature = "legacy_local_publishing")]` (default off)** in `apps/desktop/src-tauri/Cargo.toml`, so the shipped binary holds no client_secret and runs no loopback OAuth listener, while the modules and their tests remain compilable under the feature for reference. Full deletion is deferred to a cleanup phase to keep this phase's diff reviewable and tests green.
- The render-queue auto-upload (`set_render_upload_targets`/`start_uploads_for_job`/`poll_upload_states` + `MontageState.upload_queue`) is the legacy campaign uploader; once step 5 routes campaign publishing through the server, these commands are no longer the live publish path. Leave them registered this phase only if other UI still depends on them; otherwise gate them too. Confirm with `grep -rn "start_uploads_for_job\|poll_upload_states\|set_render_upload_targets" apps/desktop/src` before removing.
- Verify: build with default features holds no secret literals — `grep -rn "client_secret" apps/desktop/src-tauri/src | grep -v cfg` returns nothing in the live path; full `cargo test -p montage-desktop` green with default features.

### 7. End-to-end verification (against a running Phase 1-4 server)
- Point `MONTAGE_SOCIAL_SERVER_URL` at the deployed `montage-social` service. Manual flow: Connect YouTube (browser opens to server, consent, server callback stores encrypted tokens) → desktop re-poll shows connected account → create campaign, approve, publish (bind/validate/upload-to-storage/schedule) → desktop polls `social_publish_job` and shows `scheduled` then (after the `pg_cron` minute-tick fires server-side) `uploading`/`processing`/`published` with provider URL — **with the desktop closed during firing**, then reopened to confirm status caught up.
- Verify token-safety end to end: inspect every desktop-received JSON; assert no `access_token`/`refresh_token`/`client_secret` substrings (the server facade already redacts — covered by `montage-social` tests; re-confirm at the wire here).

## Reuse vs new

- **Reused (already tested):** all `montage-social::api` DTOs and the `SocialApi`/FSM/merged safety logic (now living server-side); `socialModel.ts` derivations; the `app/social/*` UI shells; the `reqwest` workspace dep; the `wiremock` test pattern from `publishing/oauth_exchange.rs`; the `openUrl` browser-open in `SocialAccounts.tsx`.
- **Newly built:** `social_client.rs`; `social_upload_artifact` command + server upload-URL handshake; config `social_server` block; the feature-gating of the legacy `publishing/` secret path; new/updated node:assert tests.


## Open risks
- Exact server route shapes (paths, request/response JSON) are defined in Phases 1-4; this plan assumes they mirror the existing SocialApi DTOs. If the server diverges (e.g. wraps responses in an envelope, or names the upload handshake differently), social_client.rs and the command bodies need to match the real contract — confirm against the Phase 1/3 specs before coding.
- OAuth callback completion is server-side, so the desktop has no synchronous signal that connect() succeeded; it must re-poll social_accounts. UX for 'pending/just-authorized' is unspecified — may need a short poll-with-timeout or a server-sent 'connection complete' signal. Flagged for implementation.
- Desktop-to-server auth before Phase 7 is a static dev bearer; this is acceptable for single-user dev but is not real multi-user auth. Ensure the dev token path is clearly temporary and not shipped to untrusted users.
- Large-file upload: signed-URL PUT must stream from disk (reqwest wrap_stream over tokio::fs::File); confirm Supabase signed-URL uploads accept streamed PUT bodies and the size limits, else the direct-multipart fallback (and its server proxy memory cost) becomes the primary path.
- Retiring the legacy publishing/ path: feature-gating (not deleting) this phase keeps tests green but leaves dead-ish code; confirm nothing else in the live render-queue/auto-upload UI still depends on set_render_upload_targets/start_uploads_for_job/poll_upload_states before gating them, or the campaign UI breaks.
- social_providers is currently static client-side; if provider eligibility/capabilities must reflect server state (e.g. audit-gate status per platform), it should proxy to the server instead of staying static.
