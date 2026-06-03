//! Integration tests for PgSocialStore against a real Postgres.
//!
//! Gated behind the `AWIDAT_TEST_PG_URL` environment variable so CI without
//! Postgres still passes.  Locally:
//!   AWIDAT_TEST_PG_URL=postgres://postgres:password@localhost:5432/postgres \
//!   cargo test -p awidat-social --test pg_store
//!
//! Against a local Supabase project:
//!   AWIDAT_TEST_PG_URL=$(supabase status --output env | grep DB_URL | cut -d= -f2) \
//!   cargo test -p awidat-social --test pg_store

use awidat_social::model::{
    AccountEligibility, AccountKind, AccountPublishDefaults, CampaignVariantTarget,
    ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider, ProviderCapabilities, PublishJob,
    PublishJobActorType, PublishJobEvent, PublishJobEventType, PublishJobStatus, TeamRole,
    ValidationState, WorkspaceMemberRole,
};
use awidat_social::oauth::{OAuthConnection, OAuthConnectionStatus};
use awidat_social::pg_store::PgSocialStore;
use awidat_social::store::{SocialStore, SocialStoreError};
use awidat_social::token::{TestKeyProvider, TokenSecret};

fn pg_url() -> Option<String> {
    std::env::var("AWIDAT_TEST_PG_URL").ok()
}

fn make_store(pg_url: &str) -> PgSocialStore {
    let store =
        PgSocialStore::connect(pg_url).unwrap_or_else(|e| panic!("connect PgSocialStore: {e}"));
    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    store
        .apply_migrations(&migrations_dir)
        .unwrap_or_else(|e| panic!("apply migrations: {e}"));
    store
}

/// Unique table-name suffix per test to avoid cross-test interference when
/// running in parallel against the same schema.  We isolate by using unique
/// primary-key prefixes rather than separate schemas.
fn uid(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{n}")
}

fn owner() -> OwnerRef {
    OwnerRef::User("user_pg_1".into())
}

fn other_owner() -> OwnerRef {
    OwnerRef::Workspace("workspace_pg_1".into())
}

fn oauth_connection(id: &str) -> OAuthConnection {
    OAuthConnection::start(
        id,
        owner(),
        Provider::YouTube,
        "state-secret",
        vec!["youtube.upload".into()],
        "/campaigns/campaign_1".into(),
        100,
        200,
    )
}

fn connected_account(id: &str) -> ConnectedAccount {
    ConnectedAccount {
        id: id.into(),
        owner: owner(),
        provider: Provider::YouTube,
        provider_account_id: format!("{id}_channel"),
        display_name: "Awidat PG Channel".into(),
        handle: Some("@awidat_pg".into()),
        avatar_url: None,
        account_kind: AccountKind::Channel,
        status: ConnectedAccountStatus::Connected,
        scopes: vec!["youtube.upload".into()],
        capabilities: ProviderCapabilities::default(),
        eligibility: AccountEligibility::eligible(),
        last_verified_at: None,
        created_at: 100,
        updated_at: 100,
    }
}

fn token_secret(account_id: &str) -> TokenSecret {
    TokenSecret::encrypt(
        account_id,
        "access-secret",
        Some("refresh-secret"),
        &TestKeyProvider::new("test-key-1", "local-key"),
        100,
    )
    .unwrap_or_else(|err| panic!("encrypt token secret: {err}"))
}

fn campaign_variant_target(id: &str) -> CampaignVariantTarget {
    CampaignVariantTarget::new(
        id,
        &uid("campaign"),
        &uid("variant"),
        &uid("acct"),
        Provider::YouTube,
        serde_json::json!({"privacy": "private"}),
        2_000,
        1_000,
    )
    .mark_validation(ValidationState::Valid, 1_100)
}

fn publish_job(id: &str, scheduled_for: i64) -> PublishJob {
    PublishJob::new(
        id,
        &uid("campaign"),
        &uid("variant"),
        &uid("acct"),
        Provider::YouTube,
        format!("render://{id}"),
        scheduled_for,
        "user_1",
    )
    .schedule(1_000)
}

fn publish_job_event(id: &str, publish_job_id: &str, created_at: i64) -> PublishJobEvent {
    PublishJobEvent::new(
        id,
        publish_job_id,
        PublishJobEventType::Scheduled,
        PublishJobActorType::User,
        "scheduled",
        serde_json::json!({}),
        created_at,
    )
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn pg_round_trips_oauth_account_and_token_records() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let conn_id = uid("oauth");
    let acct_id = uid("acct");
    let connection = oauth_connection(&conn_id);
    let account = connected_account(&acct_id);
    let secret = token_secret(&acct_id);

    store
        .save_oauth_connection(connection.clone())
        .unwrap_or_else(|e| panic!("save oauth connection: {e}"));
    store
        .save_connected_account(account.clone())
        .unwrap_or_else(|e| panic!("save connected account: {e}"));
    store
        .save_token_secret(secret.clone())
        .unwrap_or_else(|e| panic!("save token secret: {e}"));

    assert_eq!(store.oauth_connection(&conn_id), Ok(connection));
    assert_eq!(store.connected_account(&acct_id), Ok(account.clone()));
    let listed = store
        .connected_accounts_for_owner(&owner())
        .unwrap_or_else(|e| panic!("list accounts: {e}"));
    assert!(listed.iter().any(|a| a.id == acct_id));
    assert_eq!(store.token_secret_for_account(&acct_id), Ok(secret));
}

#[test]
fn pg_persists_targets_jobs_and_events() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let target_id = uid("target");
    let job_id = uid("job");
    let event1_id = uid("event");
    let event2_id = uid("event");
    let target = campaign_variant_target(&target_id);
    let job = publish_job(&job_id, 2_000);
    let event1 = publish_job_event(&event1_id, &job_id, 1_000);
    let event2 = publish_job_event(&event2_id, &job_id, 1_100);

    store
        .save_campaign_variant_target(target.clone())
        .unwrap_or_else(|e| panic!("save target: {e}"));
    store
        .save_publish_job(job.clone())
        .unwrap_or_else(|e| panic!("save job: {e}"));
    store
        .append_publish_job_event(event2.clone())
        .unwrap_or_else(|e| panic!("append event2: {e}"));
    store
        .append_publish_job_event(event1.clone())
        .unwrap_or_else(|e| panic!("append event1: {e}"));

    assert_eq!(store.campaign_variant_target(&target_id), Ok(target));
    assert_eq!(
        store.campaign_variant_target("missing"),
        Err(SocialStoreError::NotFound)
    );
    assert_eq!(store.publish_job(&job_id), Ok(job));
    assert_eq!(
        store.publish_job("missing"),
        Err(SocialStoreError::NotFound)
    );
    assert_eq!(store.publish_job_events(&job_id), Ok(vec![event1, event2]));

    // Duplicate event id must be rejected.
    let dup = PublishJobEvent::new(
        &event1_id,
        &job_id,
        PublishJobEventType::RetryQueued,
        PublishJobActorType::Worker,
        "retry",
        serde_json::json!({}),
        1_200,
    );
    assert!(store.append_publish_job_event(dup).is_err());
}

#[test]
fn pg_claims_due_scheduled_jobs_once_in_schedule_order() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);

    let id_later = uid("job_later");
    let id_earlier = uid("job_earlier");
    let id_future = uid("job_future");

    let later_by_schedule = {
        let mut j = publish_job(&id_later, 1_900);
        // Override the account to be shared so publish_jobs_for_account works.
        j.connected_account_id = "shared_acct_pg".into();
        j
    };
    let earlier_by_schedule = {
        let mut j = publish_job(&id_earlier, 1_000);
        j.connected_account_id = "shared_acct_pg".into();
        j
    };
    let future = {
        let mut j = publish_job(&id_future, 2_600);
        j.connected_account_id = "shared_acct_pg".into();
        j
    };

    for job in [
        later_by_schedule.clone(),
        earlier_by_schedule,
        future.clone(),
    ] {
        store
            .save_publish_job(job)
            .unwrap_or_else(|e| panic!("save job: {e}"));
    }

    let claimed = store
        .claim_due_publish_jobs(2_500, 1)
        .unwrap_or_else(|e| panic!("claim: {e}"));
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, id_earlier);
    assert_eq!(claimed[0].status, PublishJobStatus::Uploading);
    assert_eq!(claimed[0].attempt_count, 1);
    assert_eq!(claimed[0].updated_at, 2_500);
    assert_eq!(store.publish_job(&id_earlier), Ok(claimed[0].clone()));
    assert_eq!(store.publish_job(&id_future), Ok(future));

    let claimed2 = store
        .claim_due_publish_jobs(2_500, 10)
        .unwrap_or_else(|e| panic!("claim2: {e}"));
    assert_eq!(claimed2.len(), 1);
    assert_eq!(claimed2[0].id, id_later);

    let claimed3 = store
        .claim_due_publish_jobs(2_500, 10)
        .unwrap_or_else(|e| panic!("claim3: {e}"));
    assert!(claimed3.is_empty());
}

#[test]
fn pg_rejects_duplicate_provider_account_for_same_owner() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let id1 = uid("acct");
    let id2 = uid("acct");

    let account1 = connected_account(&id1);
    let mut account2 = connected_account(&id2);
    // Same provider_account_id as account1 → duplicate.
    account2.provider_account_id = account1.provider_account_id.clone();

    store
        .save_connected_account(account1)
        .unwrap_or_else(|e| panic!("save acct1: {e}"));
    assert_eq!(
        store.save_connected_account(account2.clone()),
        Err(SocialStoreError::DuplicateConnectedAccount)
    );

    // Different owner is fine.
    account2.owner = other_owner();
    assert_eq!(store.save_connected_account(account2), Ok(()));
}

#[test]
fn pg_disable_checks_owner_and_marks_account_disabled() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let id = uid("acct");
    store
        .save_connected_account(connected_account(&id))
        .unwrap_or_else(|e| panic!("save: {e}"));

    assert_eq!(
        store.disable_connected_account(&id, &other_owner(), 200),
        Err(SocialStoreError::OwnerMismatch)
    );

    let disabled = store
        .disable_connected_account(&id, &owner(), 200)
        .unwrap_or_else(|e| panic!("disable: {e}"));
    assert_eq!(disabled.status, ConnectedAccountStatus::Disabled);
    assert_eq!(disabled.updated_at, 200);
    assert_eq!(store.connected_account(&id), Ok(disabled));
}

#[test]
fn pg_oauth_status_update_persists() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let id = uid("oauth");
    store
        .save_oauth_connection(oauth_connection(&id))
        .unwrap_or_else(|e| panic!("save: {e}"));

    let updated = store
        .update_oauth_status(&id, OAuthConnectionStatus::Completed)
        .unwrap_or_else(|e| panic!("update status: {e}"));
    assert_eq!(updated.status, OAuthConnectionStatus::Completed);
    assert_eq!(store.oauth_connection(&id), Ok(updated));
}

#[test]
fn pg_account_listing_contains_no_token_material() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let id = uid("acct");
    let account = connected_account(&id);
    let secret = token_secret(&id);

    store
        .save_connected_account(account)
        .unwrap_or_else(|e| panic!("save account: {e}"));
    store
        .save_token_secret(secret.clone())
        .unwrap_or_else(|e| panic!("save secret: {e}"));

    let listed = store
        .connected_accounts_for_owner(&owner())
        .unwrap_or_else(|e| panic!("list: {e}"));
    let json = serde_json::to_string(&listed).unwrap_or_else(|e| panic!("serialize listed: {e}"));
    assert!(!json.contains(&secret.encrypted_access_token));
    if let Some(ref rt) = secret.encrypted_refresh_token {
        assert!(!json.contains(rt as &str));
    }
}

#[test]
fn pg_persists_defaults_roles_and_account_jobs() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);
    let acct_id = uid("acct");
    let ws_id = uid("workspace");
    let user_id = uid("user");
    let job_id = uid("job");

    let account = connected_account(&acct_id);
    let defaults = AccountPublishDefaults {
        connected_account_id: acct_id.clone(),
        default_privacy: Some("unlisted".into()),
        default_tags: vec!["awidat".into()],
        title_prefix: Some("Awidat: ".into()),
        description_suffix: Some("Built with Awidat.".into()),
        updated_at: 2_000,
    };
    let role = WorkspaceMemberRole::new(&ws_id, &user_id, TeamRole::Publisher);
    let mut job = publish_job(&job_id, 2_000);
    job.connected_account_id = acct_id.clone();

    store
        .save_connected_account(account)
        .unwrap_or_else(|e| panic!("save account: {e}"));
    store
        .save_account_publish_defaults(defaults.clone())
        .unwrap_or_else(|e| panic!("save defaults: {e}"));
    store
        .save_workspace_member_role(role.clone())
        .unwrap_or_else(|e| panic!("save role: {e}"));
    store
        .save_publish_job(job.clone())
        .unwrap_or_else(|e| panic!("save job: {e}"));

    assert_eq!(store.account_publish_defaults(&acct_id), Ok(defaults));
    assert_eq!(store.workspace_member_roles(&ws_id), Ok(vec![role]));
    assert_eq!(store.publish_jobs_for_account(&acct_id), Ok(vec![job]));
}

#[test]
fn pg_duplicate_publish_job_idempotency_key_rejected() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };
    let mut store = make_store(&url);

    let id1 = uid("job");
    let id2 = uid("job");
    let job1 = publish_job(&id1, 2_000);

    // Build a second job that shares the idempotency key (same artifact_ref + account).
    let job2 = PublishJob::new(
        &id2,
        &job1.campaign_id,
        &job1.variant_id,
        &job1.connected_account_id,
        Provider::YouTube,
        job1.artifact_ref.clone(), // same artifact_ref → same idempotency_key
        2_000,
        "user_1",
    )
    .schedule(1_000);

    store
        .save_publish_job(job1)
        .unwrap_or_else(|e| panic!("save job1: {e}"));

    assert_eq!(
        store.save_publish_job(job2),
        Err(SocialStoreError::Storage(
            "duplicate publish job idempotency key".into()
        ))
    );
}

// ── Postgres-specific: concurrent claim uses FOR UPDATE SKIP LOCKED ──────────

#[test]
fn pg_concurrent_claim_does_not_double_claim() {
    let url = match pg_url() {
        Some(u) => u,
        None => return,
    };

    // Insert one due job.
    let mut store = make_store(&url);
    let job_id = uid("job_concurrent");
    let mut job = publish_job(&job_id, 1_000);
    job.connected_account_id = uid("acct_concurrent");
    store
        .save_publish_job(job)
        .unwrap_or_else(|e| panic!("save: {e}"));

    // Two independent stores sharing the pool race to claim.
    let _store1 = make_store(&url);
    let _store2 = make_store(&url);

    let pg_url_clone = url.clone();
    let jid_clone = job_id.clone();

    let handle = std::thread::spawn(move || {
        let mut s =
            PgSocialStore::connect(&pg_url_clone).unwrap_or_else(|e| panic!("connect store2: {e}"));
        s.claim_due_publish_jobs(2_000, 10)
            .unwrap_or_else(|e| panic!("claim store2: {e}"))
            .into_iter()
            .filter(|j| j.id == jid_clone)
            .count()
    });

    let mut s1 = PgSocialStore::connect(&url).unwrap_or_else(|e| panic!("connect store1: {e}"));
    let s1_claimed = s1
        .claim_due_publish_jobs(2_000, 10)
        .unwrap_or_else(|e| panic!("claim store1: {e}"))
        .into_iter()
        .filter(|j| j.id == job_id)
        .count();

    let s2_claimed = handle.join().unwrap_or_else(|_| panic!("thread panicked"));

    // Exactly one of the two claimers gets the job, the other gets 0.
    assert_eq!(
        s1_claimed + s2_claimed,
        1,
        "job was claimed {} times by s1 and {} times by s2",
        s1_claimed,
        s2_claimed
    );
}
