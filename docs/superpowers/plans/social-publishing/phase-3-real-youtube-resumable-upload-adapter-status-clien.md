# Phase 3: Real YouTube resumable-upload adapter + status client, wired into the server service

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** [1, 2]

## Prerequisites
- [USER] Create/own a Google Cloud project with YouTube Data API v3 enabled and an OAuth client (client_id/secret). Phase 2 stores the secret; Phase 3 needs the project to exist to smoke-test against the live API.
- [USER] Submit the YouTube API project for the TOS/compliance audit (the long-pole gate). Until it clears, ALL API-uploaded videos are forced private — the force_private config in Step 3 must default true. Start this in parallel now.
- [USER] Provide at least one real YouTube channel + a connected account (via Phase 2 OAuth) with the youtube.upload scope to run the manual sandbox smoke test.
- [WE-STAGE] reqwest/tokio are already workspace deps; we only add them to crates/social/Cargo.toml under a feature flag.
- [WE-STAGE] wiremock/httpmock dev-dependency for offline HTTP-level tests; no external network needed for CI.
- [WE-STAGE] Decide chunk size (multiple of 256KiB) and force_private/quota config surface in the server crate config.

## Plan

## Context discovered in the codebase (read before executing)

The `montage-social` crate (`crates/social/`) is today **fully synchronous** and has **no HTTP client** — `Cargo.toml` lists only `base64, serde, serde_json, rusqlite, sha2, thiserror`. The workspace root `Cargo.toml` already provides `reqwest = { version = "0.12", default-features = false, features = ["json","stream","rustls-tls","cookies"] }` and `tokio` as workspace deps, so we add them to the crate, we do not introduce new versions.

The adapter contract is deliberately token-blind and bytes-blind:
- `UploadAdapter::upload(&self, &UploadRequest)` (`crates/social/src/upload_adapter.rs:50-53`) is sync and the request carries only `artifact_ref: String` and `access_token_ref: String`.
- `UploadService::execute_claimed_job` (`crates/social/src/upload_service.rs:89-102`) reads `store.token_secret_for_account(&account.id)?` into `_token_secret` (discarded), then sets `access_token_ref: format!("token_secret:{}", account.id)`. **The real access token is never handed to the adapter, and neither are the video bytes.** Tests at `upload_service.rs:494-538` and `upload_status.rs:441-475` assert that no raw `access-secret`/`refresh-secret`/`access_token`/`refresh_token` strings ever appear in the serialized `UploadRequest`/events — this redaction invariant is load-bearing and must survive.
- Status side mirrors this: `UploadStatusService::poll_processing_job` (`crates/social/src/upload_status.rs:100-108`) discards `_token_secret` and passes `access_token_ref: format!("token_secret:{}", account.id)`.

The `YouTubeUploadAdapter<C>` / `YouTubeStatusAdapter<C>` generics + `YouTubeUploadClient` / `YouTubeStatusClient` traits already exist (`crates/social/src/youtube_upload.rs:35-72`) with full request/response mapping and error mapping tested (`youtube_upload.rs:230-520`). **Only the concrete client `impl` of those two traits is missing** — the adapter glue, privacy mapping, URL formatting, and error→`UploadAdapterError` mapping are done and tested. This is the big reuse.

The desktop currently wires mocks: `social_execute_upload` builds `MockUploadAdapter::published(...)` and `social_poll_status` builds `MockReadyStatus` (`apps/desktop/src-tauri/src/commands/social.rs:356-437`). `api.rs` worker entrypoints `execute_claimed_upload_job` / `poll_upload_status` (`crates/social/src/api.rs:644-693`) take `&impl UploadAdapter` / `&impl UploadStatusAdapter` — these are the seams the real client plugs into. The server-side caller that replaces the desktop mock is built in this phase's server crate (introduced in Phase 1).

Token decryption exists but is the XOR stub: `TokenSecret::decrypt_access_token(&impl LocalTokenKeyProvider)` (`crates/social/src/token.rs:85-91`). `token_bundle.rs` already models `access_token_expires_at` for refresh decisions. Phase 2 owns the real exchange/refresh + real encryption; Phase 3 consumes whatever `decrypt_access_token` Phase 2 lands (we depend on the decrypted-token accessor, not its crypto).

### The one real design decision Phase 3 forces

The real client needs three things the current contract withholds: the **decrypted access token**, the **video bytes** (resolved from `artifact_ref`), and **async**. Resolution that keeps the spec's "FSM unchanged" promise and the redaction tests green:

1. Keep `UploadRequest.access_token_ref` exactly as-is (opaque ref) so all redaction tests stay green untouched. The concrete client receives the real token and the byte source through a **separate constructor-injected resolver**, never through the serializable `UploadRequest`. Concretely: the real `YouTubeUploadClient` is constructed in the server crate holding (a) a token resolver closure/handle that maps `access_token_ref`→decrypted bearer token, and (b) an artifact reader that maps `artifact_ref`→a streaming body. The serializable request still only carries refs.
2. Async: rather than make the whole synchronous FSM async (large, risky, contradicts "callers move, FSM unchanged"), the concrete `YouTubeUploadClient::upload_video` (a sync trait method) drives the async reqwest work on a runtime handle the client owns (e.g. `tokio::runtime::Handle::block_on` or a dedicated current-thread runtime inside the client). The FSM/service stay sync; only the leaf client bridges to async. This is the minimal-blast-radius option and is called out as an open risk below.

---

## Step 1 — Add HTTP/runtime deps to the social crate (gated)

File: `crates/social/Cargo.toml`. Add `reqwest = { workspace = true }` and `tokio = { workspace = true }` (and `bytes`/`futures` if the streaming body needs them, both already in workspace). Put the concrete client behind a Cargo feature, e.g. `youtube-live`, so the existing sync, dependency-light unit-test surface (`cargo test -p montage-social` default features) stays fast and the new networked code compiles only when asked:
```
[features]
youtube-live = ["dep:reqwest", "dep:tokio"]
```
Verify: `cargo build -p montage-social` (no feature) unchanged; `cargo build -p montage-social --features youtube-live` compiles.

## Step 2 — Define the token + artifact resolution seams

File: `crates/social/src/youtube_upload.rs` (extend; do not touch the existing traits/adapter). Add two small traits the concrete client depends on, e.g. `AccessTokenResolver { fn bearer_for(&self, access_token_ref: &str) -> Result<String, ...> }` and `ArtifactSource { fn open(&self, artifact_ref: &str) -> Result<ArtifactBody, ...> }` where `ArtifactBody` exposes total length (needed for the `Content-Range`/`X-Upload-Content-Length` headers) and a chunked reader. These are the boundary that keeps real tokens/bytes out of `UploadRequest`. Provide a test/in-memory impl of each for unit tests.

Why a seam and not inline file IO: `artifact_ref` is `render://...` / `file://...` today and will become a Supabase Storage signed URL (open question in spec, "File transport"). Abstracting the source lets Phase 5/file-transport decisions slot in without touching the upload state machine.

Verify: unit tests construct the real client over the in-memory resolver/source.

## Step 3 — Implement the concrete `YouTubeUploadClient` (resumable upload)

File: `crates/social/src/youtube_upload.rs`, behind `#[cfg(feature = "youtube-live")]`. Implement `LiveYouTubeUploadClient<R: AccessTokenResolver, A: ArtifactSource>` with `impl YouTubeUploadClient`. The Data API v3 resumable flow (per spec lines 38-41):
1. **Initiate**: `POST https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status` with `Authorization: Bearer <token>`, `X-Upload-Content-Type`, `X-Upload-Content-Length: <total>`, and a JSON body = video resource. The `status.privacyStatus` field maps from `YouTubeUploadRequest.privacy` (already normalized to `private`/`unlisted`/`public` by the adapter at `youtube_upload.rs:120,200-206`). **Private-until-audit gate**: while the project's TOS audit has not cleared, force `privacyStatus="private"` regardless of requested privacy and ignore/clamp `publishAt`; gate via a `force_private: bool` client config flag (default true). On success read the `Location` response header = the session URI.
2. **Upload session**: PUT the bytes to the session URI. Implement chunked resumable upload: send chunks (multiple of 256KiB; pick a chunk size, e.g. 8–16 MiB) with `Content-Range: bytes <start>-<end>/<total>`. On `308 Resume Incomplete`, read the `Range` response header to learn the server's confirmed byte offset and continue from there (this is the resume/interruption path). On `200/201` the upload is complete; parse the returned video resource JSON for `id` and `status.uploadStatus`/`processingDetails` to set `YouTubeUploadResponse { video_id, processing }`. `processing = true` when YouTube reports the video still processing (so the FSM moves to `Processing` and the status client takes over).
3. **Size cap**: enforce `<= 256GB` before initiating; oversized → `YouTubeUploadClientError::NetworkOrServer` or a media-constraint mapping (the adapter already maps `MediaConstraintFailed`; if we want a clean terminal-fail, return a constraint error — confirm with the error-mapping at `youtube_upload.rs:216-228`).
4. **Error mapping**: 401/403 missing-scope → `YouTubeUploadClientError::MissingScope`; eligibility/forbidden (channel not enabled for uploads) → `AccountNotEligible`; 5xx / network / 429 → `NetworkOrServer(message)` (so `UploadService` records a retryable `Failed` and the bounded-backoff retry from Phase 4 re-runs). These three variants already map cleanly to `UploadAdapterError` (`youtube_upload.rs:216-228`) — reuse, do not change the mapping.
5. Async bridge per the design decision: run the reqwest calls on the client's owned runtime handle inside the sync `upload_video`.

Verify (unit, `--features youtube-live`): drive the client against `wiremock`/`httpmock` (add as `dev-dependency`, workspace if present else pin) asserting: initiate sends correct headers + privacy body; a forced `308` with a `Range` header triggers a correctly-offset resume PUT; final `200` parses `video_id`; a `403 insufficientPermissions` maps to `MissingScope`; a `500` maps to `NetworkOrServer`; `force_private=true` overrides a `public` request to `private` in the initiate body; a `>256GB` length is rejected before any HTTP call.

## Step 4 — Implement the concrete `YouTubeStatusClient` (status polling)

File: `crates/social/src/youtube_upload.rs`, same feature gate. Implement `LiveYouTubeStatusClient<R: AccessTokenResolver>` with `impl YouTubeStatusClient`:
- `GET https://www.googleapis.com/youtube/v3/videos?part=status,processingDetails&id=<video_id>` with bearer auth.
- Map `processingDetails.processingStatus` / `status.uploadStatus`: `processing` → `YouTubeProcessingState::Processing`; `succeeded`/`processed` → `Processed`; `failed`/`terminated` (or `status.rejectionReason` present) → `Failed` with `failure_reason` from the rejection/processing failure reason. These three states already map into `UploadStatusResult` in `YouTubeStatusAdapter::poll_status` (`youtube_upload.rs:163-197`) — reuse.
- Network/5xx/429 → `YouTubeStatusClientError::NetworkOrServer(message)` (the only variant; `youtube_upload.rs:62-65`).

Verify (unit, `--features youtube-live`): mock responses for each of the three states + an error; assert the resulting `YouTubeStatusResponse`. The downstream mapping into `UploadStatusResult` is already covered by existing tests at `youtube_upload.rs:430-489`, so we only test the HTTP→`YouTubeStatusResponse` layer.

## Step 5 — Make the real token reachable for the resolver

The resolver in Step 2 must turn `access_token_ref` (`token_secret:<account_id>`) into a live bearer token. Implement the production `AccessTokenResolver` in the **server crate** (introduced Phase 1, owned by Phase 2 for refresh) so it: looks up `store.token_secret_for_account(account_id)`, checks `access_token_expires_at` (`token.rs` field), refreshes via the Phase-2 refresh path if expired/near-expiry, then `decrypt_access_token(&key_provider)` (`token.rs:85-91`). Phase 3 defines the trait and a test impl; the production impl lives where the key provider + refresh live (Phase 2 output). **Per D6 (HARD ordering): Phase 2 must be fully merged before this server wiring — there is NO decrypt-only/TODO fallback.** The crate-level YouTube client code (no token dependency) may be written in parallel with Phase 2, but the production `AccessTokenResolver` (decrypt + refresh) does not ship until Phase 2's AEAD storage + refresh entrypoint is merged.

Verify: resolver unit test with a `TokenSecret::encrypt(...)` round-trip (mirrors `token.rs:147-155`) returning the decrypted bearer.

## Step 6 — Wire the real adapters into the server service instead of `MockUploadAdapter`

The replacement target is the **server-side caller** (Phase 1's `montage-social` HTTP service / worker), which calls the already-existing `SocialApi::execute_claimed_upload_job` (`crates/social/src/api.rs:644-671`) and `SocialApi::poll_upload_status` (`api.rs:678-693`). Construct:
- `YouTubeUploadAdapter::new(LiveYouTubeUploadClient::new(resolver, artifact_source, YouTubeClientConfig { force_private, chunk_size, .. }))`
- `YouTubeStatusAdapter::new(LiveYouTubeStatusClient::new(resolver))`
and pass `&adapter` into those two `SocialApi` methods. No change to `api.rs`, `upload_service.rs`, or `upload_status.rs` — they already accept `&impl UploadAdapter` / `&impl UploadStatusAdapter`. This is the entire "wire it in" step on the server side.

**Quota gate (100 uploads/day/project, spec line 40-41) — per D5, Phase 3 OWNS the counter table:** add a new migration in `crates/social/migrations/` creating `provider_upload_quota (project_key text, day date, count int, PK(project_key, day))`. Enforce in the server worker *before* calling `execute_claimed_upload_job`, because the FSM has no quota concept and must not gain one (keeps FSM unchanged): increment the row transactionally when an upload is accepted, check it before `videos.insert`. (Do NOT count `Uploaded` events and do NOT reference a Phase-1 counter table — Phase 1 does not create one.) When the cap is hit, skip executing new YouTube uploads this tick and leave jobs `Scheduled` (they fire next day) — do not mark them `Failed`. Verify with a unit test of the worker gate (counter at 100 → no adapter call).

The **desktop** `social_execute_upload` / `social_poll_status` mocks (`apps/desktop/src-tauri/src/commands/social.rs:356-437`) are intentionally NOT changed here — per the architecture the desktop becomes a thin client that polls the server (Phase 5). Leave the desktop mocks until Phase 5 rewires the desktop to call the server. (Optional: add a comment pointing at the server adapter so the mock isn't mistaken for production.)

## Step 7 — Failure-injection integration tests (spec testing strategy lines 170-175)

File: a new integration test module behind `youtube-live`, e.g. `crates/social/tests/youtube_live.rs`. Using `wiremock`/`httpmock`:
1. **Resumable interruption/resume**: initiate → first PUT returns `308` with partial `Range` → second PUT from the correct offset returns `200`. Assert one resulting `Published`/`Processing` job through `UploadService::execute_claimed_job` with the real adapter over a mock server + `InMemorySocialStore`.
2. **Token-refresh failure**: resolver returns a refresh error → job ends in a retryable failure / `RequiresAction` and the account-needs-reauth path is observable (coordinate exact mapping with Phase 2).
3. **Status failure**: status poll returns `failed`/rejection → job transitions to `Failed` via the existing `UploadStatusService` mapping.
Verify: `cargo test -p montage-social --features youtube-live`.

## Step 8 — Full verification + redaction guard

Run:
- `cargo test -p montage-social` (default features) — all existing sync FSM/redaction tests stay green, proving the FSM was not disturbed.
- `cargo test -p montage-social --features youtube-live` — new client + integration tests.
- `cargo clippy -p montage-social --all-features` and `cargo fmt --check`.
- Re-run the existing redaction assertions (`upload_service.rs:494-538`, `upload_status.rs:441-475`, `token.rs:129-144`) and confirm they still pass — this is the proof that the real token/bytes seam (Step 2) did not leak secrets into `UploadRequest`/events.

A manual sandbox smoke test (real Google project in test/private mode, real small mp4) is a human prerequisite gated on Phase 2 OAuth being live and the Google project existing — do it once the audit-gated private upload can be exercised end-to-end.


## Open risks
- Async-in-sync bridge: the existing UploadAdapter/UploadService API is synchronous, but reqwest is async. Step 3/4 block_on inside the leaf client. If a future phase makes the FSM async, this bridge is throwaway. Alternative (bigger blast radius, rejected here): make the adapter traits async — contradicts spec's 'FSM unchanged, callers move' and would touch every adapter/test. Confirm the block_on approach is acceptable before coding.
- Token + bytes never flow through UploadRequest today (only opaque refs), and the redaction tests enforce that. The resolver/ArtifactSource seam (Step 2) is the chosen way to inject real material without breaking redaction — but it means the concrete client is constructed with extra collaborators the mock path never had. Validate the seam keeps all redaction assertions green.
- Hard dependency on Phase 2 for the production AccessTokenResolver (decrypt + refresh). If Phase 2's refresh entrypoint isn't merged, the resolver can only decrypt; an expired token at fire-time would fail rather than refresh. Sequence Phase 2 before Phase 3's server wiring, or ship Step 5 with a refresh seam + TODO.
- Quota enforcement (100 uploads/day/project) has no home in the FSM and is added in the server worker (Step 6). The counter source (Postgres row vs counting Uploaded events) depends on Phase 1's schema; pick one once Phase 1 lands. Multi-user would blow the per-project cap (spec open question) — out of scope, flagged.
- ArtifactSource resolution depends on the unresolved 'file transport' open question (Supabase signed URL vs direct). The trait abstracts it, but the production impl can't be finalized until that decision lands (likely Phase 1/5).
- YouTube processing-status field semantics (uploadStatus vs processingDetails.processingStatus, rejection reasons) need validation against live API responses; the mock tests encode our assumption and the manual smoke test (Step 8) confirms it.
- Desktop mocks in commands/social.rs are intentionally left in place this phase; if anyone expects desktop to publish for real after Phase 3, that's Phase 5. Make the boundary explicit to avoid confusion.
