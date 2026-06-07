# Social Upload Status And Team Controls Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Phase 4B upload lifecycle contracts and Phase 5 team/agency controls for the server-backed social publishing domain.

**Architecture:** Keep this as framework-neutral `montage-social` domain code. Phase 4B adds mocked status polling/finalization contracts after a provider upload returns `Processing`; Phase 5 adds role policy, per-account defaults, and usage audit storage/services without adding a web-server or UI layer.

**Tech Stack:** Rust, `serde`, `serde_json`, existing `SocialStore`, in-memory store, SQLite store, mocked provider clients.

---

## Scope

Included:
- YouTube-first upload status polling contract for `Processing` publish jobs.
- Final URL recording when a provider status response becomes published.
- Provider failure mapping for status polling.
- Workspace/team role policy for connect, disconnect, schedule, cancel, and retry decisions.
- Account usage audit data built from publish jobs and events.
- Brand/channel defaults per connected account.

Excluded:
- Live Google/TikTok/Meta HTTP calls.
- Web server route handlers.
- Desktop UI.
- Production migrations outside the in-memory SQLite schema used by `SqliteSocialStore`.
- TikTok/Instagram live upload/status implementation before app permissions.

## File Structure

- Create `crates/social/src/upload_status.rs`
  - Shared status polling trait, request/result/error types, and upload status service.
- Modify `crates/social/src/youtube_upload.rs`
  - Add mockable YouTube status client boundary and adapter implementation.
- Modify `crates/social/src/model.rs`
  - Add `PublishJobEventType::StatusPolled`, team role/action models, account defaults, and usage audit DTOs.
- Modify `crates/social/src/store.rs`
  - Add storage methods for publish jobs by account, account defaults, and role records.
- Modify `crates/social/src/sqlite_store.rs`
  - Persist new account defaults and role records; list publish jobs by account; map new event type.
- Create `crates/social/src/team_service.rs`
  - Role checks and account audit/default service functions.
- Modify `crates/social/src/lib.rs`
  - Export `upload_status` and `team_service`.

## Task 1: Upload Status Polling Contract

**Files:**
- Create: `crates/social/src/upload_status.rs`
- Modify: `crates/social/src/model.rs`
- Modify: `crates/social/src/sqlite_store.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing status service tests**

Add tests in `upload_status.rs` for:
- `poll_processing_job_publishes_when_provider_status_is_ready`
- `poll_processing_job_keeps_processing_when_provider_is_processing`
- `poll_processing_job_fails_on_provider_failure`
- `poll_processing_job_rejects_non_processing_jobs_before_provider_call`

The tests should seed an `InMemorySocialStore` with a `Processing` job that already has `provider_post_id`, connected account, and token secret. Use a recording `UploadStatusAdapter` to prove no raw token is serialized in request/event JSON.

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social upload_status::tests
```

Expected: FAIL because `upload_status` does not exist.

- [ ] **Step 3: Implement status contract and service**

Add:
- `UploadProcessingStatus::{Processing, Published, Failed}`
- `UploadStatusRequest`
- `UploadStatusResult`
- `UploadStatusAdapterError`
- `UploadStatusAdapter`
- `UploadStatusService::poll_processing_job`

Rules:
- Only `PublishJobStatus::Processing` can be polled.
- Adapter provider must match the job/account provider.
- Token secret is loaded only to prove the account has a token; request uses `token_secret:<account_id>`.
- `Processing` result saves the job unchanged except `updated_at`.
- `Published` result calls `PublishJob::publish`.
- `Failed` result calls `PublishJob::fail`.
- Every provider result appends `PublishJobEventType::StatusPolled`.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social upload_status::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/upload_status.rs crates/social/src/model.rs crates/social/src/sqlite_store.rs crates/social/src/lib.rs
git commit -m "feat(social): add upload status polling"
```

## Task 2: YouTube Status Boundary

**Files:**
- Modify: `crates/social/src/youtube_upload.rs`

- [ ] **Step 1: Write failing YouTube status tests**

Add tests for:
- mapping a YouTube `processing` status to `UploadProcessingStatus::Processing`
- mapping a YouTube `processed` status to `UploadProcessingStatus::Published` with final watch URL
- mapping a YouTube rejected/failed status to `UploadProcessingStatus::Failed`
- rejecting wrong provider and blank provider post id

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social youtube_upload::tests::youtube_status
```

Expected: FAIL because the status client boundary does not exist.

- [ ] **Step 3: Implement YouTube status adapter**

Add:
- `YouTubeStatusRequest`
- `YouTubeStatusResponse`
- `YouTubeProcessingState`
- `YouTubeStatusClient`
- `YouTubeStatusAdapter<C>`

Implement `UploadStatusAdapter` for `YouTubeStatusAdapter<C>` using mocked clients only.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social youtube_upload::tests::youtube_status
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/youtube_upload.rs
git commit -m "feat(social): add youtube status adapter"
```

## Task 3: Team Role Policy

**Files:**
- Modify: `crates/social/src/model.rs`
- Create: `crates/social/src/team_service.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing role policy tests**

Add tests for:
- workspace owner/admin can connect and disconnect accounts
- publisher can schedule, cancel, and retry but cannot connect/disconnect
- viewer cannot mutate publishing state
- direct `OwnerRef::User` remains allowed for user-owned accounts

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social team_service::tests::role_policy
```

Expected: FAIL because team service/types do not exist.

- [ ] **Step 3: Implement role policy**

Add:
- `TeamRole::{Owner, Admin, Publisher, Viewer}`
- `TeamAction::{ConnectAccount, DisconnectAccount, SchedulePublish, CancelPublish, RetryPublish}`
- `WorkspaceMemberRole`
- `TeamPolicy::can_perform(owner, actor_user_id, action, roles)`

Rules:
- `OwnerRef::User(user_id)` allows only the same user for all actions.
- Workspace owner/admin can perform all actions.
- Publisher can schedule/cancel/retry only.
- Viewer cannot mutate.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social team_service::tests::role_policy
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/model.rs crates/social/src/team_service.rs crates/social/src/lib.rs
git commit -m "feat(social): add team role policy"
```

## Task 4: Account Defaults And Usage Audit

**Files:**
- Modify: `crates/social/src/model.rs`
- Modify: `crates/social/src/store.rs`
- Modify: `crates/social/src/sqlite_store.rs`
- Modify: `crates/social/src/team_service.rs`

- [ ] **Step 1: Write failing defaults/audit tests**

Add tests for:
- saving and loading per-account `AccountPublishDefaults`
- defaults reject owner mismatch
- usage audit returns only jobs for accounts owned by the requested owner
- audit summary counts scheduled, processing, published, failed, and requires-action jobs
- audit responses/events do not include token material

- [ ] **Step 2: Run red test**

Run:

```bash
cargo test -p montage-social team_service::tests::account_defaults_and_audit
```

Expected: FAIL because defaults/audit storage does not exist.

- [ ] **Step 3: Implement defaults and audit storage/services**

Add:
- `AccountPublishDefaults`
- `AccountUsageAudit`
- `PublishJobStatusCounts`
- `SocialStore::save_account_publish_defaults`
- `SocialStore::account_publish_defaults`
- `SocialStore::publish_jobs_for_account`
- `SocialStore::save_workspace_member_role`
- `SocialStore::workspace_member_roles`
- `TeamService::save_account_defaults`
- `TeamService::account_defaults`
- `TeamService::account_usage_audit`

SQLite should store defaults and member roles as JSON payload tables and list publish jobs by `connected_account_id`.

- [ ] **Step 4: Run green test**

Run:

```bash
cargo test -p montage-social team_service::tests::account_defaults_and_audit
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/model.rs crates/social/src/store.rs crates/social/src/sqlite_store.rs crates/social/src/team_service.rs
git commit -m "feat(social): add account defaults and usage audit"
```

## Task 5: Phase 4B/5 Verification

**Files:**
- No new files unless review fixes are required.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test -p montage-social upload_status::tests youtube_upload::tests::youtube_status team_service::tests
```

- [ ] **Step 2: Run full social verification**

Run:

```bash
cargo test -p montage-social
cargo clippy -p montage-social --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

- [ ] **Step 3: Final review**

Ask a fresh reviewer to inspect Phase 4B/5 for spec alignment, role-policy holes, status retry/idempotency problems, token leakage, and overreach.

- [ ] **Step 4: Commit review fixes if any**

If review requires fixes, make them with failing regression tests first and commit:

```bash
git add crates/social
git commit -m "fix(social): tighten upload status team controls"
```

- [ ] **Step 5: Completion status**

Report:
- implemented commits
- verification commands and outcomes
- review outcome
- remaining work, especially live HTTP/server routes/UI if still excluded
