# Phase 6: TikTok + Instagram adapters behind their app-review/sandbox audits

> **BINDING:** Read `RECONCILIATION.md` first. Where this plan conflicts with it, RECONCILIATION wins. Key: domain stays sync (D1), server crate = `crates/social-server` (D2).


**Depends on phases:** [1, 2, 3, 4]

## Prerequisites
- [USER] TikTok Developer app with the content-posting API enabled, the `video.publish` scope added, sandbox/audit mode configured, and ~5 sandbox test users registered (TikTok forces private + limited test users until the `video.publish` approval + sandbox demo + content audit pass). Required only for Task 3 live integration tests, not for the domain adapters.
- [USER] TikTok app-review submission (`video.publish` approval + sandbox demo + content audit) — long calendar-pole; start in parallel immediately. Public posting stays clamped to private until this clears.
- [USER] Meta Developer app with Instagram Graph API product, a connected Instagram Business/Creator (professional) test account, and the `instagram_content_publish` permission. In dev mode the app can publish to admin/test users before App Review. Required only for Task 6 live integration tests.
- [USER] Meta App Review submission for `instagram_content_publish` (Business/Creator) — long calendar-pole; start in parallel.
- [USER] Confirm/obtain current Instagram Graph API content-publishing daily rate limits (historically ~25 posts/24h) — bounds retry/backoff config.
- [WE-STAGE] Step 0 Instagram API verification: read current Graph API docs and lock the exact container-then-publish field names/flow before writing IG code (design doc flags this flow as never independently verified).
- [WE-STAGE] Decide and stage file transport for the PULL model: rendered artifact served as a server-reachable signed URL (Supabase Storage) for both TikTok PULL_FROM_URL and IG `video_url` (resolves design open question, line 205).
- [WE-STAGE] Per-provider sandbox/audit config flags (e.g. `tiktok_public_enabled`, `instagram_public_enabled`) defaulting to false, gating the privacy clamp; flip per-platform as each audit clears.

## Plan

## Phase 6 — TikTok + Instagram content-publishing adapters (sandbox-first, behind audit gates)

### Goal & shape

Add two new provider upload adapters that mirror the already-tested YouTube adapter pattern, built to function in each platform's **private/sandbox** mode first, so the full pipeline (schedule → claim → upload → poll → published/failed) can be demoed before any app-review clears. Public posting flips on per-platform as audits pass — no code change required beyond an eligibility/privacy gate.

The work splits into two layers, exactly as YouTube does today:

1. **Domain layer (pure Rust, no HTTP) in `crates/social/src/`** — new `tiktok_upload.rs` and `instagram_upload.rs` modules, each defining a mockable `*Client` trait (the HTTP boundary), an adapter implementing the existing `UploadAdapter` trait (`crates/social/src/upload_adapter.rs:50`), and a status adapter implementing `UploadStatusAdapter` (`crates/social/src/upload_status.rs:52`). Unit-tested with mock clients — no credentials, no network. This is the bulk of Phase 6 and is fully testable now.
2. **Live HTTP `*Client` impls in the server crate** (the crate Phases 1–3 stand up). These make the real REST calls against TikTok/Meta sandbox apps. They are thin and integration-tested against sandboxes; they carry no FSM logic.

This deliberately reuses the verified machinery: the job FSM (`crates/social/src/job.rs`), `UploadService::execute_claimed_job` (`crates/social/src/upload_service.rs:47`), `UploadStatusService::poll_processing_job` (`crates/social/src/upload_status.rs:79`), token decryption (`crates/social/src/token.rs`), eligibility (`crates/social/src/eligibility.rs`), and the `ProviderRegistry` slots already wired for TikTok/Instagram (`crates/social/src/provider.rs:122`). None of that is reimplemented.

> Note on the existing `BlockedUploadAdapter` (`crates/social/src/upload_adapter.rs:99`): today TikTok/Instagram are routed to this blocked slot, which deterministically returns `RequiresAction`. Phase 6 replaces the blocked routing with the real adapters when the account is eligible, and keeps `BlockedUploadAdapter` as the fallback for ineligible/pre-audit accounts.

---

### Step 0 (Instagram only): VERIFY THE API before writing any IG code

The spec explicitly flags that the Instagram Graph API content-publishing flow was **never independently verified** (design doc lines 43, 162, 200–201). Do this before Task 4.

0.1 Read the current Instagram Graph API content-publishing docs (Instagram Graph API → Content Publishing) and confirm the two-step flow and exact field names: `POST /{ig-user-id}/media` (container create, with `media_type`, `video_url`/`image_url`, `caption`, and for Reels `media_type=REELS`) returning a creation `id`; then `POST /{ig-user-id}/media_publish` with `creation_id`; then `GET /{container-id}?fields=status_code` polling (`IN_PROGRESS` / `FINISHED` / `ERROR`) before publish. Confirm the published media id maps to a permalink via `GET /{media-id}?fields=permalink`.
0.2 Confirm the gate facts: Instagram Business/Creator (professional) account required; `instagram_content_publish` permission; App Review for that permission; daily container/publish rate limits (historically ~25/24h — confirm current number, it bounds retry/backoff); video must be a publicly fetchable URL (PULL model, like TikTok), which determines the file-transport decision (Supabase Storage signed URL).
0.3 Record findings in the Instagram task as the contract the mock client encodes. If the flow differs materially from the assumed container-then-publish, adjust Task 4's request/response shapes before writing tests.

Verification of this step: a short written confirmation (in the PR description / task notes) citing the doc sections, with the exact JSON field names the mock client will assert on. This is a [WE-STAGE] research step; it does not require the audit to be approved.

---

### Task 1 — TikTok upload adapter domain module (mockable, no HTTP)

**Files:**
- Create `crates/social/src/tiktok_upload.rs`
- Modify `crates/social/src/lib.rs` (add `pub mod tiktok_upload;` after line 22, alongside the existing `youtube_upload` export)

1.1 Mirror `crates/social/src/youtube_upload.rs` structure. Define:
- `TikTokUploadRequest` (fields needed by `/v2/post/publish/video/init/`: `title`/caption, `privacy_level` mapped from `UploadPrivacy`, source-info: PULL_FROM_URL `video_url` derived from `artifact_ref`, `access_token_ref`, and the `post_mode`/`media_type` constants for direct post). Map `UploadPrivacy::Private → SELF_ONLY`, `Public → PUBLIC_TO_EVERYONE`, `Unlisted → MUTUAL_FOLLOW_FRIENDS` or the nearest sandbox-legal value (in sandbox/unaudited mode privacy is forced to `SELF_ONLY` — encode that clamp in the adapter, gated on an `eligible_for_public` flag threaded from account eligibility).
- `TikTokInitResponse { publish_id: String }` — TikTok returns a `publish_id` from init, not a final post id; the post id/URL only become available after processing completes (poll).
- `TikTokUploadClient` trait with `init_video_publish(&self, &TikTokUploadRequest) -> Result<TikTokInitResponse, TikTokUploadClientError>`.
- `TikTokUploadClientError { MissingScope, AccountNotEligible, RateLimited, NetworkOrServer(String) }`.
- `TikTokUploadAdapter<C>` implementing `UploadAdapter`: validate provider == TikTok, non-empty token, non-empty caption constraints, build the `TikTokUploadRequest`, call `init_video_publish`, and return `UploadResult { provider_post_id: publish_id, provider_post_url: "" or placeholder, processing: true }`. Because TikTok is always async, `processing` is always `true`, so the FSM moves the job to `Processing` (handled already by `UploadService::complete_success`, `crates/social/src/upload_service.rs`), then the status adapter resolves it.
- Map `TikTokUploadClientError` → `UploadAdapterError` exactly like `youtube_client_error` (`crates/social/src/youtube_upload.rs:216`): `MissingScope`/`AccountNotEligible` → `RequiresAction{reason}`, `RateLimited`/`NetworkOrServer` → `NetworkOrServer{message}`.

1.2 Define `TikTokStatusClient` + `TikTokStatusAdapter<C>` implementing `UploadStatusAdapter` (`crates/social/src/upload_status.rs:52`), mirroring `YouTubeStatusAdapter` (`crates/social/src/youtube_upload.rs:137`). It calls TikTok's `/v2/post/publish/status/fetch/` with the `publish_id`, maps `status` (`PROCESSING_DOWNLOAD`/`PROCESSING_UPLOAD` → `Processing`, `PUBLISH_COMPLETE` → `Published` with the resolved `share_url`/post id, `FAILED` → `Failed` with `normalized_error: "platform_processing_failed"` and a `raw_error_ref`).

1.3 **Tests (TDD, write first):** in the `#[cfg(test)] mod tests` of `tiktok_upload.rs`, copy the shape of the YouTube tests (`crates/social/src/youtube_upload.rs:230`). Use a `RecordingTikTokClient` mock that asserts the request fields (privacy clamp to `SELF_ONLY` when not `eligible_for_public`, token ref passthrough, caption) and returns a canned `publish_id`. Cover: maps init response to processing `UploadResult`; rejects wrong provider (`ProviderMismatch`); rejects empty token (`MissingUploadToken`); maps each client error variant; status adapter maps PROCESSING/COMPLETE/FAILED; privacy-clamp test. Add a token-redaction test asserting the serialized request contains the `token_ref` reference string but not `access_token`/`refresh_token` (matches `crates/social/src/upload_adapter.rs:163`).

**Verify:** `cargo test -p montage-social tiktok_upload::tests` (expect fail → implement → pass), then `cargo test -p montage-social`, `cargo clippy -p montage-social --all-targets -- -D warnings`, `cargo fmt --all -- --check`.

---

### Task 2 — TikTok eligibility + registry + privacy clamp wiring

**Files:**
- Modify `crates/social/src/eligibility.rs` (the `tiktok_eligibility` fn, line 52) — reuse as-is; it already gates on the `video.publish` scope. Add (or confirm) that the report exposes an `unaudited`/private-only signal so the adapter and `provider.rs` agree on the public-posting clamp. If `ProviderCapabilities.public_posting` already encodes this (it is `has_publish` today, line 73), thread *that* into the adapter's `eligible_for_public` rather than adding a new field.
- Modify `crates/social/src/provider.rs` (`default_multi_platform`, line 122) — TikTok is currently `AccountEligibility::blocked("tiktok_direct_post_permission_required")` with `public_posting:false` (lines 140–155). Keep blocked-by-default, but ensure the registry/account path can flip to eligible+private-only once the account carries `video.publish`. No new file; this is a value/flow change verified by the existing `provider.rs` tests (lines 209–234) plus a new test asserting a TikTok account with the publish scope yields `upload_video:true, public_posting` gated.

2.1 The decision point of "use real adapter vs BlockedUploadAdapter" lives wherever the worker selects an adapter for a provider (the cron worker / server crate from Phase 4, and the desktop dev path in `apps/desktop/src-tauri/src/commands/social.rs:26` which currently imports only `MockUploadAdapter`). For the domain crate, expose a clear constructor (`TikTokUploadAdapter::new(client)`) and let the caller decide; do not bake routing into the crate.

**Verify:** `cargo test -p montage-social provider::tests eligibility::tests` and the new TikTok eligibility test.

---

### Task 3 — TikTok live HTTP client (server crate, sandbox-integration-tested)

**Files (in the server crate created by Phases 1–3 — likely `crates/social-server/` or wherever Phase 3's real YouTube `YouTubeUploadClient` impl landed; place the TikTok client beside it):**
- Create the live `TikTokUploadClient`/`TikTokStatusClient` impls using the same HTTP stack Phase 3 chose (`reqwest` is NOT yet a dependency of `montage-social` per `crates/social/Cargo.toml` — it belongs in the server crate, keeping the domain crate HTTP-free).

3.1 Implement init: `POST https://open.tiktokapis.com/v2/post/publish/video/init/` with `Authorization: Bearer <decrypted access token>` (decrypt via `crates/social/src/token.rs` `decrypt_access_token` using the server's real key provider), body `{ post_info: {title, privacy_level, ...}, source_info: { source: "PULL_FROM_URL", video_url } }`. Parse `data.publish_id`. The `video_url` is a server-reachable signed URL (Supabase Storage) for the rendered artifact — this is the file-transport open question (design line 205); resolve it here as PULL_FROM_URL (no FILE_UPLOAD chunking needed), which is simpler and matches IG.
3.2 Implement status: `POST /v2/post/publish/status/fetch/` with `{ publish_id }`, map `status` field.
3.3 Map HTTP/error responses to the `TikTokUploadClientError` variants (401/scope → `MissingScope`/`AccountNotEligible`, 429 → `RateLimited`, 5xx/transport → `NetworkOrServer`).

3.4 **Integration tests** against the TikTok **sandbox** app (gated behind an env flag / `#[ignore]` so CI without creds stays green): init a real PULL_FROM_URL publish to a sandbox test user in `SELF_ONLY`, poll to completion, assert a `publish_id` and eventual COMPLETE. Add a failure-injection test: interrupt/return a bad `video_url` and assert the FAILED mapping.

**Prerequisite (external):** the TikTok sandbox app, `video.publish` scope added, and ~5 sandbox test users must exist (see prerequisites). The adapter and its unit tests do NOT need this; only the live integration tests do.

**Verify:** unit/compile in CI; sandbox integration tests run manually/in a creds-gated job.

---

### Task 4 — Instagram upload adapter domain module (mockable, no HTTP)

**Files:**
- Create `crates/social/src/instagram_upload.rs`
- Modify `crates/social/src/lib.rs` (add `pub mod instagram_upload;`)

Depends on Step 0's verified contract.

4.1 Define, mirroring YouTube/TikTok modules:
- `InstagramContainerRequest { video_url, caption, media_type, access_token_ref, ig_user_id }` and `InstagramContainerResponse { creation_id }`.
- `InstagramPublishResponse { media_id }`.
- `InstagramUploadClient` trait with `create_container(&self, &InstagramContainerRequest) -> Result<InstagramContainerResponse, ...>` and `publish_container(&self, creation_id, access_token_ref) -> Result<InstagramPublishResponse, ...>`. (Two calls — the adapter orchestrates create → poll-container-status → publish.) Because IG also needs the container to finish processing before publish, model the adapter to return `processing: true` after `create_container` and let the **status adapter** drive the container-status poll and the final `media_publish`, OR (simpler, matches verified flow) have the adapter do create+publish synchronously only when the container is `FINISHED`. Decide based on Step 0 findings; default to: adapter creates container and returns `processing:true` with `provider_post_id = creation_id`; status adapter polls container `status_code`, calls `media_publish` on `FINISHED`, then resolves the permalink → `Published`.
- `InstagramUploadClientError { NotProfessional, MissingScope, RateLimited, NetworkOrServer(String) }` mapped to `UploadAdapterError` (NotProfessional/MissingScope → `RequiresAction`, others → `NetworkOrServer`).
- `InstagramUploadAdapter<C>` implementing `UploadAdapter`; `InstagramStatusAdapter<C>` implementing `UploadStatusAdapter`.

4.2 **Tests (TDD):** mock `RecordingInstagramClient` asserting field names confirmed in Step 0; cover container-create → processing, status poll `IN_PROGRESS`→Processing / `FINISHED`→publish→Published(permalink) / `ERROR`→Failed, professional-account gate via `RequiresAction`, token redaction. Mirror `crates/social/src/youtube_upload.rs:396` status sub-module.

**Verify:** `cargo test -p montage-social instagram_upload::tests`, then full `cargo test -p montage-social`, clippy, fmt.

---

### Task 5 — Instagram eligibility + registry wiring

**Files:**
- `crates/social/src/eligibility.rs` `instagram_eligibility` (line 83) — already gates on professional account + `instagram_content_publish` scope. Reuse unchanged.
- `crates/social/src/provider.rs` Instagram slot (lines 156–171, currently `blocked("instagram_professional_account_required")`, `public_posting:false`). Same flip pattern as TikTok: stays blocked until account is professional + scoped, then eligible+private gated by audit. Add a test mirroring the TikTok one.

**Verify:** `cargo test -p montage-social provider::tests eligibility::tests`.

---

### Task 6 — Instagram live HTTP client (server crate, sandbox-integration-tested)

**Files:** in the server crate beside the TikTok/YouTube live clients.

6.1 Implement `create_container` (`POST /{ig-user-id}/media`), `get_container_status` (`GET /{creation-id}?fields=status_code`), `publish_container` (`POST /{ig-user-id}/media_publish`), and permalink fetch (`GET /{media-id}?fields=permalink`) with the long-lived access token (decrypted server-side). `video_url` is a Supabase Storage signed URL (same transport as TikTok).
6.2 Map errors (190/permission → `MissingScope`, professional-account errors → `NotProfessional`, 4 (rate-limit code) → `RateLimited`, else `NetworkOrServer`).
6.3 **Integration tests** against a Meta **test app / Instagram test user** (creds-gated/`#[ignore]`): create container with a sandbox video URL, poll, publish, assert permalink. Failure-injection: bad `video_url` → `ERROR` status → Failed mapping.

**Prerequisite (external):** Meta app with Instagram Graph API, a connected Instagram Business/Creator test account, `instagram_content_publish` permission (in dev mode the app can publish to admin/test users before App Review).

**Verify:** creds-gated integration job.

---

### Task 7 — Worker adapter selection + privacy clamp integration

**Files:**
- The cron worker / job-execution entrypoint built in Phase 4 (server crate). Wherever `UploadService::execute_claimed_job` and `UploadStatusService::poll_processing_job` are invoked per provider, extend the adapter-selection match arm beyond YouTube to: for an **eligible** TikTok/IG account use the real adapter; otherwise use `BlockedUploadAdapter` (`crates/social/src/upload_adapter.rs:99`) so ineligible/pre-audit jobs land in `RequiresAction` (already handled by `UploadService::complete_error`).
- Enforce the **sandbox privacy clamp**: until the platform's audit clears (a per-provider config flag, e.g. `tiktok_public_enabled=false`), force `UploadPrivacy::Private` regardless of requested privacy, and surface this in a job event. This is the "build to work in sandbox first; flip on per-platform" requirement (design lines 164–165).

7.1 Desktop dev path: `apps/desktop/src-tauri/src/commands/social.rs:26` currently only imports `MockUploadAdapter`. No production change required here for Phase 6 (firing is server-side per the architecture), but confirm the desktop status-poll/display surfaces TikTok/IG `RequiresAction` reasons and provider URLs the same way it does YouTube — verified by the existing desktop `node:assert` client tests under `apps/desktop/src/` (the facade already redacts tokens, design line 154).

**Verify:** server-crate worker integration tests with the mock clients exercising a full TikTok job (scheduled → claim → init → processing → poll complete → published) and a full IG job; assert the privacy clamp event appears; assert ineligible accounts → `RequiresAction`.

---

### Task 8 — Full verification & review

8.1 `cargo test -p montage-social` (all domain tests green, YouTube tests unchanged — FSM untouched).
8.2 Server-crate tests: unit green in CI; sandbox integration jobs (creds-gated) run manually and recorded.
8.3 `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `git diff --check`.
8.4 Dispatch a focused reviewer (scope: token exposure in new adapters, privacy-clamp correctness, error→FSM mapping parity with YouTube, no FSM duplication, IG-flow matches the Step-0 verified contract).

---

### What is reused vs newly built

**Reused (do not reimplement):** job FSM (`job.rs`), `UploadService` (`upload_service.rs`), `UploadStatusService` (`upload_status.rs`), `UploadAdapter`/`UploadStatusAdapter` traits + `UploadRequest`/`UploadResult`/error types (`upload_adapter.rs`, `upload_status.rs`), `BlockedUploadAdapter`, token decryption (`token.rs`), eligibility fns (`eligibility.rs`), `ProviderRegistry` TikTok/IG slots (`provider.rs`), the desktop facade/redaction.

**Newly built:** `crates/social/src/tiktok_upload.rs`, `crates/social/src/instagram_upload.rs` (domain adapters + mock clients + tests); live HTTP `*Client` impls in the server crate; worker adapter-selection arms + privacy clamp; per-provider audit/sandbox config flags; the IG-API verification (Step 0).


## Open risks
- Instagram content-publishing flow is unverified in research; if Step 0 reveals the real flow differs from the assumed container-then-publish (e.g. Reels-specific fields, mandatory container-status polling before publish, or a different async model), the IG adapter request/response shapes and status-adapter orchestration in Task 4 must be revised before tests are written.
- Whether the live HTTP `*Client` implementations belong in a separate server crate vs. a feature-gated module of `montage-social` depends on where Phase 3 placed the real YouTube client; this plan assumes a server crate to keep the domain crate HTTP-free (it currently has no reqwest dependency). Confirm Phase 3's choice.
- TikTok privacy_level mapping for `Unlisted` has no exact equivalent; the chosen mapping (MUTUAL_FOLLOW_FRIENDS or clamp to SELF_ONLY) needs confirmation against current TikTok docs.
- Both TikTok and IG use a PULL model requiring a publicly fetchable signed URL; if Phase 1/5 chose a different file-transport mechanism, Tasks 3/6 must adapt (chunked FILE_UPLOAD for TikTok adds significant complexity and is the fallback if PULL is unavailable).
- Daily rate/quota caps (TikTok per-app, IG ~25/24h) interact with the bounded-retry backoff from Phase 4; ensure retries do not burn the daily cap. Multi-user quota is explicitly out of scope (design line 203).
- Sandbox integration tests require live credentials and external test users that may not be provisioned when code is ready; the domain adapters and their unit tests must be fully landable and demoable without them, with live tests creds-gated/#[ignore].
