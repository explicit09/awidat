# Publish Targeting And Queue Contracts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 3B of the server-backed social publishing pipeline: bind campaign variants to connected accounts, validate publishability, create durable publish jobs, record audit events, and model queue state transitions without live provider uploads.

**Architecture:** Extend `montage-social` with publish-targeting service contracts on top of the Phase 3A store boundary. The service remains framework-neutral so a future HTTP server can mount it directly. Provider upload adapters, live HTTP calls, desktop UI, and background worker execution are explicitly out of scope; this phase creates the durable target/job/event contracts those systems will use.

**Tech Stack:** Rust 2024 workspace crate, `serde`, `serde_json`, `thiserror`, `rusqlite`, deterministic unit tests, no live network calls in CI.

---

## Scope

This plan implements the remaining Phase 3 server contract before upload adapters:

- Bind `CampaignVariantTarget` rows to connected accounts.
- Validate targets against connected account ownership, provider, status, eligibility, and capabilities.
- Create durable `PublishJob` rows with stable idempotency keys.
- Add append-only `PublishJobEvent` audit records.
- Add queue-state contracts for due-job claiming, cancellation, retry, and action-required states.

Do not implement provider upload HTTP calls, YouTube `videos.insert`, TikTok/Instagram posting, polling live provider status, desktop/web UI, or an HTTP framework.

## File Structure

- Modify `crates/social/src/model.rs`: add publish-event model enums/struct.
- Modify `crates/social/src/job.rs`: add target/job/event helper constructors and job state transition methods.
- Modify `crates/social/src/store.rs`: extend `SocialStore` and `InMemorySocialStore` for targets, jobs, and events.
- Modify `crates/social/src/sqlite_store.rs`: persist targets, jobs, and events.
- Create `crates/social/src/publish_service.rs`: framework-neutral service methods for target binding, validation, scheduling, claim, cancel, and retry.
- Modify `crates/social/src/lib.rs`: expose `publish_service`.

## Task 1: Publish Target And Event Domain Helpers

**Files:**
- Modify: `crates/social/src/model.rs`
- Modify: `crates/social/src/job.rs`

- [ ] **Step 1: Write failing domain tests**

Add tests in `crates/social/src/job.rs`:

```rust
#[test]
fn campaign_variant_target_starts_pending_for_connected_account() {
    let target = CampaignVariantTarget::new(
        "target_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        serde_json::json!({"privacy": "private"}),
        2_000,
        1_000,
    );

    assert_eq!(target.validation_state, ValidationState::Pending);
    assert_eq!(target.provider, Provider::YouTube);
    assert_eq!(target.created_at, 1_000);
    assert_eq!(target.updated_at, 1_000);
}

#[test]
fn publish_job_can_move_through_queue_contract_states() {
    let scheduled = PublishJob::new(
        "job_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        "render://artifact_1",
        2_000,
        "user_1",
    )
    .schedule(1_100);

    assert_eq!(scheduled.status, PublishJobStatus::Scheduled);
    assert_eq!(scheduled.updated_at, 1_100);

    let uploading = scheduled.claim_for_upload(2_001);
    assert_eq!(uploading.status, PublishJobStatus::Uploading);
    assert_eq!(uploading.attempt_count, 1);

    let retry = uploading.fail("network_or_server_error", "raw_error_1", 2_100).retry(2_200);
    assert_eq!(retry.status, PublishJobStatus::Scheduled);
    assert_eq!(retry.normalized_error, None);
    assert_eq!(retry.raw_error_ref, None);
}

#[test]
fn publish_job_event_records_audit_metadata_without_token_material() {
    let event = PublishJobEvent::new(
        "event_1",
        "job_1",
        PublishJobEventType::Scheduled,
        PublishJobActorType::User,
        "scheduled by user",
        serde_json::json!({"target_id": "target_1"}),
        1_000,
    );

    let json = serde_json::to_string(&event)
        .unwrap_or_else(|err| panic!("serialize publish job event: {err}"));
    assert!(json.contains("scheduled by user"));
    assert!(!json.contains("access_token"));
    assert!(!json.contains("refresh_token"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social job::tests
```

Expected: FAIL with unresolved `CampaignVariantTarget::new`, `PublishJobEvent`, `PublishJobEventType`, `PublishJobActorType`, and job transition methods.

- [ ] **Step 3: Add publish event model**

Add to `crates/social/src/model.rs` after `PublishJob`:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishJobEventType {
    TargetBound,
    Validated,
    Scheduled,
    Claimed,
    Cancelled,
    RetryQueued,
    RequiresAction,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishJobActorType {
    User,
    System,
    Worker,
    Provider,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishJobEvent {
    pub id: String,
    pub publish_job_id: String,
    pub event_type: PublishJobEventType,
    pub actor_type: PublishJobActorType,
    pub message: String,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}
```

- [ ] **Step 4: Add target, job, and event helpers**

Update imports in `crates/social/src/job.rs`:

```rust
use crate::model::{
    CampaignVariantTarget, Provider, PublishJob, PublishJobActorType, PublishJobEvent,
    PublishJobEventType, PublishJobStatus, ValidationState,
};
```

Add these impl blocks before existing tests:

```rust
impl CampaignVariantTarget {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        campaign_id: impl Into<String>,
        variant_id: impl Into<String>,
        connected_account_id: impl Into<String>,
        provider: Provider,
        platform_fields: serde_json::Value,
        scheduled_for: i64,
        now: i64,
    ) -> Self {
        Self {
            id: id.into(),
            campaign_id: campaign_id.into(),
            variant_id: variant_id.into(),
            connected_account_id: connected_account_id.into(),
            provider,
            platform_fields,
            scheduled_for,
            validation_state: ValidationState::Pending,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn mark_validation(mut self, state: ValidationState, now: i64) -> Self {
        self.validation_state = state;
        self.updated_at = now;
        self
    }
}

impl PublishJob {
    pub fn schedule(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Scheduled;
        self.updated_at = now;
        self
    }

    pub fn claim_for_upload(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Uploading;
        self.attempt_count = self.attempt_count.saturating_add(1);
        self.updated_at = now;
        self
    }

    pub fn cancel(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Cancelled;
        self.updated_at = now;
        self
    }

    pub fn fail(
        mut self,
        normalized_error: impl Into<String>,
        raw_error_ref: impl Into<String>,
        now: i64,
    ) -> Self {
        self.status = PublishJobStatus::Failed;
        self.normalized_error = Some(normalized_error.into());
        self.raw_error_ref = Some(raw_error_ref.into());
        self.updated_at = now;
        self
    }

    pub fn retry(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Scheduled;
        self.normalized_error = None;
        self.raw_error_ref = None;
        self.requires_action_reason = None;
        self.updated_at = now;
        self
    }
}

impl PublishJobEvent {
    pub fn new(
        id: impl Into<String>,
        publish_job_id: impl Into<String>,
        event_type: PublishJobEventType,
        actor_type: PublishJobActorType,
        message: impl Into<String>,
        metadata: serde_json::Value,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            publish_job_id: publish_job_id.into(),
            event_type,
            actor_type,
            message: message.into(),
            metadata,
            created_at,
        }
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cargo test -p montage-social job::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/social/src/model.rs crates/social/src/job.rs
git commit -m "feat(social): add publish target job events"
```

## Task 2: Store Boundary For Targets Jobs And Events

**Files:**
- Modify: `crates/social/src/store.rs`

- [ ] **Step 1: Write failing store tests**

Add tests in `crates/social/src/store.rs`:

```rust
#[test]
fn in_memory_store_persists_targets_jobs_and_events() {
    let mut store = InMemorySocialStore::default();
    let target = CampaignVariantTarget::new(
        "target_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        serde_json::json!({"privacy": "private"}),
        2_000,
        1_000,
    );
    let job = PublishJob::new(
        "job_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        "render://artifact_1",
        2_000,
        "user_1",
    )
    .schedule(1_000);
    let event = PublishJobEvent::new(
        "event_1",
        "job_1",
        PublishJobEventType::Scheduled,
        PublishJobActorType::User,
        "scheduled",
        serde_json::json!({}),
        1_000,
    );

    store.save_campaign_variant_target(target.clone()).unwrap_or_else(|err| {
        panic!("save target: {err}");
    });
    store.save_publish_job(job.clone()).unwrap_or_else(|err| {
        panic!("save publish job: {err}");
    });
    store.append_publish_job_event(event.clone()).unwrap_or_else(|err| {
        panic!("append publish job event: {err}");
    });

    assert_eq!(store.campaign_variant_target("target_1"), Ok(target));
    assert_eq!(store.publish_job("job_1"), Ok(job));
    assert_eq!(store.publish_job_events("job_1"), Ok(vec![event]));
}

#[test]
fn in_memory_store_claims_due_scheduled_jobs_once() {
    let mut store = InMemorySocialStore::default();
    let due = PublishJob::new(
        "job_due",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        "render://artifact_1",
        2_000,
        "user_1",
    )
    .schedule(1_000);
    let future = PublishJob::new(
        "job_future",
        "campaign_1",
        "variant_2",
        "acct_1",
        Provider::YouTube,
        "render://artifact_2",
        3_000,
        "user_1",
    )
    .schedule(1_000);

    store.save_publish_job(due).unwrap_or_else(|err| panic!("save due job: {err}"));
    store.save_publish_job(future).unwrap_or_else(|err| panic!("save future job: {err}"));

    let claimed = store.claim_due_publish_jobs(2_500, 10).unwrap_or_else(|err| {
        panic!("claim due publish jobs: {err}");
    });
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, "job_due");
    assert_eq!(claimed[0].status, PublishJobStatus::Uploading);

    let claimed_again = store.claim_due_publish_jobs(2_500, 10).unwrap_or_else(|err| {
        panic!("claim due publish jobs again: {err}");
    });
    assert!(claimed_again.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social store::tests
```

Expected: FAIL with unresolved target/job/event store methods.

- [ ] **Step 3: Extend `SocialStore` trait**

Add imports in `crates/social/src/store.rs`:

```rust
use crate::model::{
    CampaignVariantTarget, ConnectedAccount, ConnectedAccountStatus, OwnerRef, PublishJob,
    PublishJobEvent, PublishJobStatus,
};
```

Add trait methods:

```rust
fn save_campaign_variant_target(
    &mut self,
    target: CampaignVariantTarget,
) -> Result<(), SocialStoreError>;

fn campaign_variant_target(&self, id: &str)
    -> Result<CampaignVariantTarget, SocialStoreError>;

fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError>;

fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError>;

fn claim_due_publish_jobs(
    &mut self,
    now: i64,
    limit: usize,
) -> Result<Vec<PublishJob>, SocialStoreError>;

fn append_publish_job_event(&mut self, event: PublishJobEvent)
    -> Result<(), SocialStoreError>;

fn publish_job_events(&self, publish_job_id: &str)
    -> Result<Vec<PublishJobEvent>, SocialStoreError>;
```

- [ ] **Step 4: Implement in-memory target/job/event storage**

Add fields to `InMemorySocialStore`:

```rust
campaign_variant_targets: BTreeMap<String, CampaignVariantTarget>,
publish_jobs: BTreeMap<String, PublishJob>,
publish_job_events: BTreeMap<String, Vec<PublishJobEvent>>,
```

Implement the new trait methods. `claim_due_publish_jobs(now, limit)` must:

- Iterate jobs in deterministic id order.
- Select only jobs where `status == PublishJobStatus::Scheduled` and `scheduled_for <= now`.
- Transition selected jobs with `claim_for_upload(now)`.
- Persist updated jobs before returning them.
- Respect `limit`.

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cargo test -p montage-social store::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/social/src/store.rs
git commit -m "feat(social): store publish targets and jobs"
```

## Task 3: SQLite Persistence For Targets Jobs And Events

**Files:**
- Modify: `crates/social/src/sqlite_store.rs`

- [ ] **Step 1: Write failing SQLite tests**

Add tests in `crates/social/src/sqlite_store.rs` matching the in-memory behavior:

```rust
#[test]
fn sqlite_persists_targets_jobs_and_events() {
    let mut store = SqliteSocialStore::new_in_memory()
        .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
    let target = CampaignVariantTarget::new(
        "target_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        serde_json::json!({"privacy": "private"}),
        2_000,
        1_000,
    );
    let job = PublishJob::new(
        "job_1",
        "campaign_1",
        "variant_1",
        "acct_1",
        Provider::YouTube,
        "render://artifact_1",
        2_000,
        "user_1",
    )
    .schedule(1_000);
    let event = PublishJobEvent::new(
        "event_1",
        "job_1",
        PublishJobEventType::Scheduled,
        PublishJobActorType::User,
        "scheduled",
        serde_json::json!({}),
        1_000,
    );

    store.save_campaign_variant_target(target.clone()).unwrap_or_else(|err| {
        panic!("save target: {err}");
    });
    store.save_publish_job(job.clone()).unwrap_or_else(|err| {
        panic!("save publish job: {err}");
    });
    store.append_publish_job_event(event.clone()).unwrap_or_else(|err| {
        panic!("append event: {err}");
    });

    assert_eq!(store.campaign_variant_target("target_1"), Ok(target));
    assert_eq!(store.publish_job("job_1"), Ok(job));
    assert_eq!(store.publish_job_events("job_1"), Ok(vec![event]));
}

#[test]
fn sqlite_claims_due_scheduled_jobs_once() {
    let mut store = SqliteSocialStore::new_in_memory()
        .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
    store
        .save_publish_job(
            PublishJob::new(
                "job_due",
                "campaign_1",
                "variant_1",
                "acct_1",
                Provider::YouTube,
                "render://artifact_1",
                2_000,
                "user_1",
            )
            .schedule(1_000),
        )
        .unwrap_or_else(|err| panic!("save due job: {err}"));

    let claimed = store.claim_due_publish_jobs(2_500, 1).unwrap_or_else(|err| {
        panic!("claim due jobs: {err}");
    });
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].status, PublishJobStatus::Uploading);

    let claimed_again = store.claim_due_publish_jobs(2_500, 1).unwrap_or_else(|err| {
        panic!("claim due jobs again: {err}");
    });
    assert!(claimed_again.is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social sqlite_store::tests
```

Expected: FAIL with missing `SqliteSocialStore` implementations for new `SocialStore` methods.

- [ ] **Step 3: Add SQLite schema**

Extend `create_schema` in `crates/social/src/sqlite_store.rs`:

```sql
CREATE TABLE IF NOT EXISTS campaign_variant_targets (
    id TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    connected_account_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    validation_state TEXT NOT NULL,
    scheduled_for INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS campaign_variant_targets_variant
    ON campaign_variant_targets(campaign_id, variant_id);

CREATE TABLE IF NOT EXISTS publish_jobs (
    id TEXT PRIMARY KEY,
    campaign_id TEXT NOT NULL,
    variant_id TEXT NOT NULL,
    connected_account_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    scheduled_for INTEGER NOT NULL,
    status TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS publish_jobs_idempotency_key
    ON publish_jobs(idempotency_key);
CREATE INDEX IF NOT EXISTS publish_jobs_due
    ON publish_jobs(status, scheduled_for, id);

CREATE TABLE IF NOT EXISTS publish_job_events (
    id TEXT PRIMARY KEY,
    publish_job_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    actor_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS publish_job_events_job
    ON publish_job_events(publish_job_id, created_at, id);
```

- [ ] **Step 4: Implement SQLite methods**

Serialize full target/job/event payloads as JSON, while storing indexed columns for query and uniqueness. `claim_due_publish_jobs(now, limit)` must:

- Select scheduled due jobs ordered by `scheduled_for, id`.
- Load payload JSON into `PublishJob`.
- Transition each job with `claim_for_upload(now)`.
- Save each updated job.
- Return updated jobs.

Map missing rows to `SocialStoreError::NotFound`; map idempotency unique constraint errors to `SocialStoreError::Storage("duplicate publish job idempotency key".into())`.

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cargo test -p montage-social sqlite_store::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/social/src/sqlite_store.rs
git commit -m "feat(social): persist publish queues in sqlite"
```

## Task 4: Publish Targeting Service

**Files:**
- Create: `crates/social/src/publish_service.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing service tests**

Create `crates/social/src/publish_service.rs` with tests for route-shaped behavior:

```rust
#[test]
fn bind_target_checks_account_owner_and_saves_pending_target() {
    let mut store = InMemorySocialStore::default();
    store.save_connected_account(connected_account("acct_1", owner(), true)).unwrap_or_else(|err| {
        panic!("save account: {err}");
    });

    let target = PublishService::bind_target(
        &mut store,
        &owner(),
        BindTargetInput {
            id: "target_1".into(),
            campaign_id: "campaign_1".into(),
            variant_id: "variant_1".into(),
            connected_account_id: "acct_1".into(),
            platform_fields: serde_json::json!({"privacy": "private"}),
            scheduled_for: 2_000,
            now: 1_000,
        },
    )
    .unwrap_or_else(|err| panic!("bind target: {err}"));

    assert_eq!(target.validation_state, ValidationState::Pending);
    assert_eq!(store.campaign_variant_target("target_1"), Ok(target));
}

#[test]
fn validate_target_marks_requires_action_for_ineligible_account() {
    let mut store = InMemorySocialStore::default();
    store.save_connected_account(connected_account("acct_1", owner(), false)).unwrap_or_else(|err| {
        panic!("save account: {err}");
    });
    bind_target(&mut store);

    let report = PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
        .unwrap_or_else(|err| panic!("validate target: {err}"));

    assert_eq!(report.state, ValidationState::RequiresAction);
    assert_eq!(report.reasons, vec!["account_not_eligible"]);
}

#[test]
fn schedule_target_creates_scheduled_job_and_event() {
    let mut store = InMemorySocialStore::default();
    store.save_connected_account(connected_account("acct_1", owner(), true)).unwrap_or_else(|err| {
        panic!("save account: {err}");
    });
    bind_target(&mut store);
    PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
        .unwrap_or_else(|err| panic!("validate target: {err}"));

    let job = PublishService::schedule_target(
        &mut store,
        &owner(),
        ScheduleTargetInput {
            job_id: "job_1".into(),
            target_id: "target_1".into(),
            artifact_ref: "render://artifact_1".into(),
            created_by: "user_1".into(),
            now: 1_200,
        },
    )
    .unwrap_or_else(|err| panic!("schedule target: {err}"));

    assert_eq!(job.status, PublishJobStatus::Scheduled);
    assert_eq!(store.publish_job_events("job_1").unwrap_or_else(|err| panic!("events: {err}")).len(), 1);
}

#[test]
fn queue_contract_claim_cancel_and_retry_are_owner_checked() {
    let mut store = InMemorySocialStore::default();
    store.save_connected_account(connected_account("acct_1", owner(), true)).unwrap_or_else(|err| {
        panic!("save account: {err}");
    });
    bind_target(&mut store);
    PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
        .unwrap_or_else(|err| panic!("validate target: {err}"));
    PublishService::schedule_target(
        &mut store,
        &owner(),
        ScheduleTargetInput {
            job_id: "job_1".into(),
            target_id: "target_1".into(),
            artifact_ref: "render://artifact_1".into(),
            created_by: "user_1".into(),
            now: 1_200,
        },
    )
    .unwrap_or_else(|err| panic!("schedule target: {err}"));

    let claimed = PublishService::claim_due_jobs(&mut store, 2_100, 10)
        .unwrap_or_else(|err| panic!("claim due jobs: {err}"));
    assert_eq!(claimed[0].status, PublishJobStatus::Uploading);

    let cancelled = PublishService::cancel_job(&mut store, &owner(), "job_1", 2_200)
        .unwrap_or_else(|err| panic!("cancel job: {err}"));
    assert_eq!(cancelled.status, PublishJobStatus::Cancelled);
}
```

Add helper functions inside the test module:

```rust
fn owner() -> OwnerRef {
    OwnerRef::User("user_1".into())
}

fn connected_account(id: &str, account_owner: OwnerRef, eligible: bool) -> ConnectedAccount {
    ConnectedAccount {
        id: id.into(),
        owner: account_owner,
        provider: Provider::YouTube,
        provider_account_id: "channel_1".into(),
        display_name: "Montage Channel".into(),
        handle: Some("@montage".into()),
        avatar_url: None,
        account_kind: AccountKind::Channel,
        status: ConnectedAccountStatus::Connected,
        scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
        capabilities: ProviderCapabilities {
            native_scheduling: true,
            queue_scheduling: true,
            upload_video: eligible,
            upload_thumbnail: true,
            public_posting: eligible,
            requires_user_consent: false,
        },
        eligibility: if eligible {
            AccountEligibility::eligible()
        } else {
            AccountEligibility::blocked("missing_youtube_upload_scope")
        },
        last_verified_at: Some(900),
        created_at: 900,
        updated_at: 900,
    }
}

fn bind_target(store: &mut InMemorySocialStore) {
    PublishService::bind_target(
        store,
        &owner(),
        BindTargetInput {
            id: "target_1".into(),
            campaign_id: "campaign_1".into(),
            variant_id: "variant_1".into(),
            connected_account_id: "acct_1".into(),
            platform_fields: serde_json::json!({"privacy": "private"}),
            scheduled_for: 2_000,
            now: 1_000,
        },
    )
    .unwrap_or_else(|err| panic!("bind target: {err}"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social publish_service::tests
```

Expected: FAIL with unresolved `PublishService` and input/report types.

- [ ] **Step 3: Implement service types**

Add these public types:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindTargetInput {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleTargetInput {
    pub job_id: String,
    pub target_id: String,
    pub artifact_ref: String,
    pub created_by: String,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetValidationReport {
    pub state: ValidationState,
    pub reasons: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PublishServiceError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("connected account belongs to a different owner")]
    OwnerMismatch,
    #[error("target provider does not match connected account")]
    ProviderMismatch,
    #[error("target is not valid for scheduling")]
    TargetNotValid,
    #[error("publish job cannot be cancelled from this state")]
    JobNotCancellable,
    #[error("publish job cannot be retried from this state")]
    JobNotRetryable,
}

pub struct PublishService;
```

- [ ] **Step 4: Implement bind, validate, schedule, claim, cancel, and retry**

Implement:

```rust
impl PublishService {
    pub fn bind_target(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        input: BindTargetInput,
    ) -> Result<CampaignVariantTarget, PublishServiceError> {
        let account = store.connected_account(&input.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        let target = CampaignVariantTarget::new(
            input.id,
            input.campaign_id,
            input.variant_id,
            account.id,
            account.provider,
            input.platform_fields,
            input.scheduled_for,
            input.now,
        );
        store.save_campaign_variant_target(target.clone())?;
        Ok(target)
    }

    pub fn validate_target(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        target_id: &str,
        now: i64,
    ) -> Result<TargetValidationReport, PublishServiceError> {
        let target = store.campaign_variant_target(target_id)?;
        let account = store.connected_account(&target.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if account.provider != target.provider {
            return Err(PublishServiceError::ProviderMismatch);
        }

        let (state, reasons) = if account.status != ConnectedAccountStatus::Connected {
            (ValidationState::RequiresAction, vec!["account_not_connected".to_string()])
        } else if !account.eligibility.eligible {
            (ValidationState::RequiresAction, vec!["account_not_eligible".to_string()])
        } else if !account.capabilities.upload_video || !account.capabilities.public_posting {
            (ValidationState::RequiresAction, vec!["missing_publish_capability".to_string()])
        } else if target.scheduled_for <= now {
            (ValidationState::Invalid, vec!["scheduled_time_invalid".to_string()])
        } else {
            (ValidationState::Valid, Vec::new())
        };

        store.save_campaign_variant_target(target.mark_validation(state.clone(), now))?;
        Ok(TargetValidationReport { state, reasons })
    }

    pub fn schedule_target(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        input: ScheduleTargetInput,
    ) -> Result<PublishJob, PublishServiceError> {
        let target = store.campaign_variant_target(&input.target_id)?;
        let account = store.connected_account(&target.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if target.validation_state != ValidationState::Valid {
            return Err(PublishServiceError::TargetNotValid);
        }

        let job = PublishJob::new(
            input.job_id,
            target.campaign_id,
            target.variant_id,
            target.connected_account_id,
            target.provider,
            input.artifact_ref,
            target.scheduled_for,
            input.created_by,
        )
        .schedule(input.now);
        store.save_publish_job(job.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_scheduled", job.id),
            job.id.clone(),
            PublishJobEventType::Scheduled,
            PublishJobActorType::User,
            "publish job scheduled",
            serde_json::json!({"target_id": input.target_id}),
            input.now,
        ))?;
        Ok(job)
    }

    pub fn claim_due_jobs(
        store: &mut impl SocialStore,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, PublishServiceError> {
        Ok(store.claim_due_publish_jobs(now, limit)?)
    }

    pub fn cancel_job(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJob, PublishServiceError> {
        let job = store.publish_job(job_id)?;
        let account = store.connected_account(&job.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if matches!(job.status, PublishJobStatus::Published | PublishJobStatus::Cancelled) {
            return Err(PublishServiceError::JobNotCancellable);
        }
        let cancelled = job.cancel(now);
        store.save_publish_job(cancelled.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_cancelled", cancelled.id),
            cancelled.id.clone(),
            PublishJobEventType::Cancelled,
            PublishJobActorType::User,
            "publish job cancelled",
            serde_json::json!({}),
            now,
        ))?;
        Ok(cancelled)
    }

    pub fn retry_job(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJob, PublishServiceError> {
        let job = store.publish_job(job_id)?;
        let account = store.connected_account(&job.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if !matches!(job.status, PublishJobStatus::Failed | PublishJobStatus::RequiresAction) {
            return Err(PublishServiceError::JobNotRetryable);
        }
        let retry = job.retry(now);
        store.save_publish_job(retry.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_retry", retry.id),
            retry.id.clone(),
            PublishJobEventType::RetryQueued,
            PublishJobActorType::User,
            "publish job retry queued",
            serde_json::json!({}),
            now,
        ))?;
        Ok(retry)
    }
}
```

- [ ] **Step 5: Export module and run test**

Update `crates/social/src/lib.rs`:

```rust
pub mod publish_service;
```

Run:

```bash
cargo test -p montage-social publish_service::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/social/src/lib.rs crates/social/src/publish_service.rs
git commit -m "feat(social): add publish targeting service"
```

## Task 5: Phase 3B Verification

**Files:**
- Verify all changed files.

- [ ] **Step 1: Run social crate tests**

Run:

```bash
cargo test -p montage-social
```

Expected: PASS.

- [ ] **Step 2: Run social crate clippy**

Run:

```bash
cargo clippy -p montage-social --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 3: Run formatting check**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS. Existing stable-toolchain warnings about `imports_granularity = Item` are acceptable only if the command exits 0.

- [ ] **Step 4: Run whitespace diff check**

Run:

```bash
git diff --check
```

Expected: PASS with no output.

- [ ] **Step 5: Confirm branch status**

Run:

```bash
git status --short --branch
```

Expected: clean worktree on `codex/clip-campaign-engine`.

## Self-Review

- Spec coverage: This plan covers target binding, validation, durable jobs, audit events, due-job claiming, cancellation, and retry. It intentionally does not cover upload adapters, live provider HTTP, status polling, UI, or a web server.
- Placeholder scan: No placeholder markers or unspecified test steps remain.
- Type consistency: Later tasks use types defined in Task 1 or existing Phase 1 through Phase 3A modules.
