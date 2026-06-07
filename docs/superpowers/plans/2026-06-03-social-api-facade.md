# Social API Facade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 6 of the server-backed social publishing pipeline: framework-neutral API methods that map the planned server routes onto the verified `montage-social` domain services.

**Architecture:** Keep Phase 6 inside `montage-social` until the repository has a dedicated web-server crate. Add a route-shaped API facade with request/response DTOs, actor/owner authorization, account and publishing operations, and worker entrypoints for upload/status execution. The facade must never expose provider token material and must remain usable from an Axum/Tauri/Next server wrapper later.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, existing `SocialStore`, existing account/publish/upload/status/team services, mocked provider adapters in tests.

---

## Scope

Included:
- Route-shaped DTOs and service methods for the initial server API surface from `docs/superpowers/specs/2026-06-02-server-backed-social-oauth-design.md`.
- Auth context that separates Montage user/workspace authorization from social OAuth credentials.
- Provider/account/OAuth account methods.
- Campaign variant target bind, validate, schedule, job lookup, cancel, and retry methods.
- Worker-facing upload execution and status polling entrypoints over existing adapters.
- Token-redaction tests proving API responses do not include token material.

Excluded:
- A concrete HTTP framework or route macro layer.
- Live Google/TikTok/Meta HTTP calls beyond existing mocked adapter traits.
- Desktop/web UI.
- Production database migrations outside `SqliteSocialStore`.
- Provider app-review work for TikTok/Instagram direct publishing.

## File Structure

- Create `crates/social/src/api.rs`
  - Public API facade, auth context, request DTOs, response DTOs, and API errors.
- Modify `crates/social/src/lib.rs`
  - Export `api`.
- Modify `crates/social/src/store.rs`
  - Add only store methods needed by API job lookup if existing methods are insufficient.
- Modify `crates/social/src/sqlite_store.rs`
  - Mirror any new store methods.

## Task 1: Account API Facade

**Files:**
- Create: `crates/social/src/api.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing account API tests**

Create `crates/social/src/api.rs` with tests named:
- `account_api_lists_providers_and_accounts_without_tokens`
- `account_api_starts_and_completes_oauth`
- `account_api_disconnect_checks_owner`

Use `InMemorySocialStore`, `ProviderRegistry::default_multi_platform()`, `SocialAccountService`, and `TestKeyProvider`.

Expected behavior:
- `providers()` returns YouTube, TikTok, and Instagram provider slots.
- `accounts()` returns connected account display/capability/eligibility data only.
- `oauth_start()` returns an authorization URL and persists an OAuth connection.
- `oauth_complete()` persists a connected account and token secret without returning token fields.
- `disconnect_account()` rejects owner mismatch and disables the account for the correct owner.

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social api::tests::account_api
```

Expected: FAIL because `api` is not exported.

- [ ] **Step 3: Implement account facade**

Add:
- `ApiActor { user_id, workspace_roles }`
- `ApiOwner { owner: OwnerRef }`
- `SocialApi`
- `AccountSummary`
- `ProviderSummary`
- `OAuthStartRequest`
- `OAuthCompleteRequest`
- `OAuthStartResponse`
- `OAuthCompleteResponse`
- `SocialApiError::{Store, Account, Publish, Upload, Status, Team, Unauthorized}`

Methods:
- `SocialApi::providers(registry) -> Vec<ProviderSummary>`
- `SocialApi::accounts(store, actor, owner) -> Result<Vec<AccountSummary>, SocialApiError>`
- `SocialApi::oauth_start(store, registry, actor, request) -> Result<OAuthStartResponse, SocialApiError>`
- `SocialApi::oauth_complete(store, key_provider, actor, request) -> Result<OAuthCompleteResponse, SocialApiError>`
- `SocialApi::disconnect_account(store, actor, owner, account_id, now) -> Result<AccountSummary, SocialApiError>`

Authorization:
- `OwnerRef::User(id)` is allowed only when `actor.user_id == id`.
- `OwnerRef::Workspace(id)` is allowed when `actor.workspace_roles` contains that workspace with `TeamRole::Owner` or `TeamRole::Admin` for account management.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social api::tests::account_api
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/api.rs crates/social/src/lib.rs
git commit -m "feat(social): add account api facade"
```

## Task 2: Publish Route Facade

**Files:**
- Modify: `crates/social/src/api.rs`

- [ ] **Step 1: Write failing publish API tests**

Add tests named:
- `publish_api_binds_validates_and_schedules_target`
- `publish_api_cancel_and_retry_are_authorized`
- `publish_api_returns_job_without_token_material`
- `publish_api_workspace_publisher_can_schedule_but_not_disconnect_accounts`

Expected behavior:
- `bind_target()` checks account ownership and saves a pending target.
- `validate_target()` uses provider capability/eligibility rules.
- `schedule_target()` creates a scheduled publish job and audit event.
- `publish_job()` returns job state by id for the same owner only.
- `cancel_job()` and `retry_job()` use the same owner/team policy as Phase 5.

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social api::tests::publish_api
```

Expected: FAIL because publish facade methods do not exist.

- [ ] **Step 3: Implement publish facade**

Add DTOs:
- `BindTargetRequest`
- `ValidateTargetRequest`
- `ScheduleTargetRequest`
- `PublishJobResponse`
- `PublishJobEventResponse`

Add methods:
- `SocialApi::bind_target(store, actor, request) -> Result<CampaignVariantTarget, SocialApiError>`
- `SocialApi::validate_target(store, registry, actor, target_id, now) -> Result<CampaignVariantTarget, SocialApiError>`
- `SocialApi::schedule_target(store, registry, actor, target_id, created_by, now) -> Result<PublishJobResponse, SocialApiError>`
- `SocialApi::publish_job(store, actor, owner, job_id) -> Result<PublishJobResponse, SocialApiError>`
- `SocialApi::cancel_job(store, actor, owner, job_id, now) -> Result<PublishJobResponse, SocialApiError>`
- `SocialApi::retry_job(store, actor, owner, job_id, now) -> Result<PublishJobResponse, SocialApiError>`

Authorization:
- User-owned jobs require matching `actor.user_id`.
- Workspace-owned schedule/cancel/retry require `TeamRole::Owner`, `TeamRole::Admin`, or `TeamRole::Publisher`.
- Viewer cannot mutate publish state.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social api::tests::publish_api
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/api.rs
git commit -m "feat(social): add publish api facade"
```

## Task 3: Worker API Facade

**Files:**
- Modify: `crates/social/src/api.rs`

- [ ] **Step 1: Write failing worker API tests**

Add tests named:
- `worker_api_executes_claimed_upload_job`
- `worker_api_polls_processing_job_to_published`
- `worker_api_rejects_status_post_id_mismatch`
- `worker_api_events_do_not_include_raw_token_material`

Expected behavior:
- `execute_claimed_upload_job()` delegates to `UploadService::execute_claimed_job`.
- `poll_upload_status()` delegates to `UploadStatusService::poll_processing_job`.
- The API layer does not add token material to requests or returned metadata.
- The Phase 5 `ProviderPostIdMismatch` guard is preserved through the API.

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social api::tests::worker_api
```

Expected: FAIL because worker facade methods do not exist.

- [ ] **Step 3: Implement worker facade**

Add methods:
- `SocialApi::execute_claimed_upload_job(store, adapter, job_id, now) -> Result<PublishJobResponse, SocialApiError>`
- `SocialApi::poll_upload_status(store, adapter, job_id, now) -> Result<PublishJobResponse, SocialApiError>`

Use existing adapter traits:
- `UploadAdapter`
- `UploadStatusAdapter`

Rules:
- These methods are worker-facing and do not accept `ApiActor`.
- They only operate on jobs in the state required by the underlying services.
- Returned responses include job status, provider post id/url, normalized error, and raw error reference only; never token secret fields.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social api::tests::worker_api
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/api.rs
git commit -m "feat(social): add worker api facade"
```

## Task 4: SQLite API Parity

**Files:**
- Modify: `crates/social/src/api.rs`
- Modify: `crates/social/src/store.rs` only if Task 2 needs a new lookup method.
- Modify: `crates/social/src/sqlite_store.rs` only if Task 2 needs a new lookup method.

- [ ] **Step 1: Write failing SQLite API tests**

Add tests named:
- `api_round_trips_account_routes_with_sqlite_store`
- `api_round_trips_publish_routes_with_sqlite_store`
- `api_round_trips_worker_routes_with_sqlite_store`

Expected behavior:
- The same API flows tested with `InMemorySocialStore` also work with `SqliteSocialStore::new_in_memory()`.
- SQLite-backed account and publish responses do not include token material.

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social api::tests::sqlite_api
```

Expected: FAIL until the API tests are wired to SQLite fixtures.

- [ ] **Step 3: Implement SQLite parity**

If no new store method is needed, this step only adds SQLite-backed API fixtures.

If `publish_job()` needs owner-safe lookup and the current store methods are insufficient, add:
- `SocialStore::publish_job_events`
- Existing method can be reused for event loading.
- Do not add broad query methods unless a test proves the API needs them.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social api::tests::sqlite_api
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/api.rs crates/social/src/store.rs crates/social/src/sqlite_store.rs
git commit -m "feat(social): add sqlite api parity"
```

## Task 5: Phase 6 Verification

**Files:**
- No new files unless review fixes are required.

- [ ] **Step 1: Run focused API tests**

Run:

```bash
cargo test -p montage-social api::tests
```

Expected: PASS.

- [ ] **Step 2: Run full social crate verification**

Run:

```bash
cargo test -p montage-social
cargo clippy -p montage-social --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Expected:
- All social tests pass.
- Clippy exits 0.
- Format exits 0. The stable-toolchain `imports_granularity` warning may appear and is non-blocking when exit code is 0.
- `git diff --check` exits 0.

- [ ] **Step 3: Fresh review**

Ask a fresh reviewer to inspect Phase 6 for:
- auth/owner/team policy holes
- token leakage in API responses/events
- route-surface mismatch with the approved spec
- overreach into concrete HTTP/UI/server choices
- broken SQLite/in-memory parity

- [ ] **Step 4: Commit review fixes if any**

If review finds a blocker, reproduce it with a focused regression test and commit the fix:

```bash
git add crates/social
git commit -m "fix(social): tighten api facade"
```

- [ ] **Step 5: Completion status**

Report:
- implemented commits
- verification commands and outcomes
- review outcome
- remaining work: concrete HTTP framework routes, production database migrations, live provider clients, server worker scheduling, and UI.

## Self-Review

- Spec coverage: This plan maps the approved initial server API surface to framework-neutral methods. It covers provider listing, account listing, OAuth start/callback, disconnect, target bind/validate/schedule, job lookup, cancel/retry, upload execution, and status polling. It intentionally does not add concrete HTTP routes, live provider clients, or UI.
- Placeholder scan: no task uses placeholder wording or unspecified tests.
- Type consistency: all tasks use existing `SocialStore`, `SocialAccountService`, `PublishService`, `UploadService`, `UploadStatusService`, `TeamPolicy`, `UploadAdapter`, and `UploadStatusAdapter` boundaries.
