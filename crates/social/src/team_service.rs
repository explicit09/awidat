use crate::model::{
    AccountPublishDefaults, AccountUsageAudit, OwnerRef, PublishJob, PublishJobEvent,
    PublishJobStatus, PublishJobStatusCounts, TeamAction, TeamRole, WorkspaceMemberRole,
};
use crate::store::{SocialStore, SocialStoreError};
use thiserror::Error;

pub struct TeamPolicy;

impl TeamPolicy {
    pub fn can_perform(
        owner: &OwnerRef,
        actor_user_id: &str,
        action: TeamAction,
        roles: &[WorkspaceMemberRole],
    ) -> bool {
        match owner {
            OwnerRef::User(user_id) => user_id == actor_user_id,
            OwnerRef::Workspace(workspace_id) => roles.iter().any(|role| {
                role.workspace_id == *workspace_id
                    && role.user_id == actor_user_id
                    && role_allows_action(&role.role, action)
            }),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TeamServiceError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("connected account belongs to a different owner")]
    OwnerMismatch,
}

pub struct TeamService;

impl TeamService {
    pub fn save_account_defaults(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        defaults: AccountPublishDefaults,
    ) -> Result<AccountPublishDefaults, TeamServiceError> {
        let account = store.connected_account(&defaults.connected_account_id)?;
        if account.owner != *owner {
            return Err(TeamServiceError::OwnerMismatch);
        }

        store.save_account_publish_defaults(defaults.clone())?;
        Ok(defaults)
    }

    pub fn account_defaults(
        store: &impl SocialStore,
        owner: &OwnerRef,
        connected_account_id: &str,
    ) -> Result<AccountPublishDefaults, TeamServiceError> {
        let account = store.connected_account(connected_account_id)?;
        if account.owner != *owner {
            return Err(TeamServiceError::OwnerMismatch);
        }

        Ok(store.account_publish_defaults(connected_account_id)?)
    }

    pub fn account_usage_audit(
        store: &impl SocialStore,
        owner: &OwnerRef,
        connected_account_id: &str,
    ) -> Result<AccountUsageAudit, TeamServiceError> {
        let account = store.connected_account(connected_account_id)?;
        if account.owner != *owner {
            return Err(TeamServiceError::OwnerMismatch);
        }

        let jobs = store.publish_jobs_for_account(connected_account_id)?;
        let events = events_for_jobs(store, &jobs)?;
        let status_counts = status_counts(&jobs);
        Ok(AccountUsageAudit {
            connected_account_id: connected_account_id.into(),
            owner: owner.clone(),
            jobs,
            events,
            status_counts,
        })
    }
}

fn events_for_jobs(
    store: &impl SocialStore,
    jobs: &[PublishJob],
) -> Result<Vec<PublishJobEvent>, TeamServiceError> {
    let mut events = Vec::new();
    for job in jobs {
        events.extend(store.publish_job_events(&job.id)?);
    }
    Ok(events)
}

fn status_counts(jobs: &[PublishJob]) -> PublishJobStatusCounts {
    let mut counts = PublishJobStatusCounts::default();
    for job in jobs {
        match job.status {
            PublishJobStatus::Scheduled => counts.scheduled += 1,
            PublishJobStatus::Processing => counts.processing += 1,
            PublishJobStatus::Published => counts.published += 1,
            PublishJobStatus::Failed => counts.failed += 1,
            PublishJobStatus::RequiresAction => counts.requires_action += 1,
            PublishJobStatus::Draft
            | PublishJobStatus::Validated
            | PublishJobStatus::Uploading
            | PublishJobStatus::Cancelled => {}
        }
    }
    counts
}

impl WorkspaceMemberRole {
    pub fn new(
        workspace_id: impl Into<String>,
        user_id: impl Into<String>,
        role: TeamRole,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            user_id: user_id.into(),
            role,
        }
    }
}

fn role_allows_action(role: &TeamRole, action: TeamAction) -> bool {
    match role {
        TeamRole::Owner | TeamRole::Admin => true,
        TeamRole::Publisher => matches!(
            action,
            TeamAction::SchedulePublish | TeamAction::CancelPublish | TeamAction::RetryPublish
        ),
        TeamRole::Viewer => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, AccountPublishDefaults, ConnectedAccount,
        ConnectedAccountStatus, OwnerRef, Provider, ProviderCapabilities, PublishJob,
        PublishJobEvent, PublishJobEventType, PublishJobStatus, TeamAction, TeamRole,
        WorkspaceMemberRole,
    };
    use crate::store::{InMemorySocialStore, SocialStore};

    mod role_policy {
        use super::*;

        #[test]
        fn workspace_owner_and_admin_can_connect_and_disconnect_accounts() {
            let roles = vec![
                WorkspaceMemberRole::new("workspace_1", "owner_user", TeamRole::Owner),
                WorkspaceMemberRole::new("workspace_1", "admin_user", TeamRole::Admin),
            ];
            let owner = OwnerRef::Workspace("workspace_1".into());

            for actor in ["owner_user", "admin_user"] {
                assert!(TeamPolicy::can_perform(
                    &owner,
                    actor,
                    TeamAction::ConnectAccount,
                    &roles
                ));
                assert!(TeamPolicy::can_perform(
                    &owner,
                    actor,
                    TeamAction::DisconnectAccount,
                    &roles
                ));
            }
        }

        #[test]
        fn publisher_can_schedule_cancel_and_retry_but_cannot_manage_accounts() {
            let roles = vec![WorkspaceMemberRole::new(
                "workspace_1",
                "publisher_user",
                TeamRole::Publisher,
            )];
            let owner = OwnerRef::Workspace("workspace_1".into());

            assert!(TeamPolicy::can_perform(
                &owner,
                "publisher_user",
                TeamAction::SchedulePublish,
                &roles
            ));
            assert!(TeamPolicy::can_perform(
                &owner,
                "publisher_user",
                TeamAction::CancelPublish,
                &roles
            ));
            assert!(TeamPolicy::can_perform(
                &owner,
                "publisher_user",
                TeamAction::RetryPublish,
                &roles
            ));
            assert!(!TeamPolicy::can_perform(
                &owner,
                "publisher_user",
                TeamAction::ConnectAccount,
                &roles
            ));
            assert!(!TeamPolicy::can_perform(
                &owner,
                "publisher_user",
                TeamAction::DisconnectAccount,
                &roles
            ));
        }

        #[test]
        fn viewer_cannot_mutate_publishing_state() {
            let roles = vec![WorkspaceMemberRole::new(
                "workspace_1",
                "viewer_user",
                TeamRole::Viewer,
            )];
            let owner = OwnerRef::Workspace("workspace_1".into());

            for action in [
                TeamAction::ConnectAccount,
                TeamAction::DisconnectAccount,
                TeamAction::SchedulePublish,
                TeamAction::CancelPublish,
                TeamAction::RetryPublish,
            ] {
                assert!(!TeamPolicy::can_perform(
                    &owner,
                    "viewer_user",
                    action,
                    &roles
                ));
            }
        }

        #[test]
        fn user_owned_accounts_allow_only_the_same_user() {
            let owner = OwnerRef::User("user_1".into());
            let roles = Vec::new();

            assert!(TeamPolicy::can_perform(
                &owner,
                "user_1",
                TeamAction::ConnectAccount,
                &roles
            ));
            assert!(TeamPolicy::can_perform(
                &owner,
                "user_1",
                TeamAction::SchedulePublish,
                &roles
            ));
            assert!(!TeamPolicy::can_perform(
                &owner,
                "user_2",
                TeamAction::ConnectAccount,
                &roles
            ));
        }
    }

    mod account_defaults_and_audit {
        use super::*;

        #[test]
        fn saves_and_loads_account_publish_defaults() {
            let mut store = store_with_account(owner());
            let defaults = AccountPublishDefaults {
                connected_account_id: "acct_1".into(),
                default_privacy: Some("unlisted".into()),
                default_tags: vec!["montage".into(), "launch".into()],
                title_prefix: Some("Montage: ".into()),
                description_suffix: Some("Built with Montage.".into()),
                updated_at: 2_000,
            };

            TeamService::save_account_defaults(&mut store, &owner(), defaults.clone())
                .unwrap_or_else(|err| panic!("save defaults: {err}"));

            assert_eq!(
                TeamService::account_defaults(&store, &owner(), "acct_1"),
                Ok(defaults)
            );
        }

        #[test]
        fn defaults_reject_owner_mismatch() {
            let mut store = store_with_account(owner());
            let defaults = AccountPublishDefaults {
                connected_account_id: "acct_1".into(),
                default_privacy: Some("private".into()),
                default_tags: Vec::new(),
                title_prefix: None,
                description_suffix: None,
                updated_at: 2_000,
            };

            assert_eq!(
                TeamService::save_account_defaults(&mut store, &other_owner(), defaults),
                Err(TeamServiceError::OwnerMismatch)
            );
        }

        #[test]
        fn usage_audit_returns_only_jobs_for_requested_owner_account() {
            let mut store = store_with_account(owner());
            store
                .save_connected_account(connected_account("acct_2", other_owner()))
                .unwrap_or_else(|err| panic!("save other account: {err}"));
            save_job_and_event(&mut store, "job_1", "acct_1", PublishJobStatus::Published);
            save_job_and_event(&mut store, "job_2", "acct_1", PublishJobStatus::Failed);
            save_job_and_event(&mut store, "job_3", "acct_2", PublishJobStatus::Published);

            let audit = TeamService::account_usage_audit(&store, &owner(), "acct_1")
                .unwrap_or_else(|err| panic!("audit: {err}"));

            assert_eq!(audit.connected_account_id, "acct_1");
            assert_eq!(audit.jobs.len(), 2);
            assert!(
                audit
                    .jobs
                    .iter()
                    .all(|job| job.connected_account_id == "acct_1")
            );
            assert_eq!(audit.status_counts.published, 1);
            assert_eq!(audit.status_counts.failed, 1);
            assert_eq!(audit.events.len(), 2);
        }

        #[test]
        fn usage_audit_counts_all_publish_job_states() {
            let mut store = store_with_account(owner());
            save_job_and_event(
                &mut store,
                "scheduled",
                "acct_1",
                PublishJobStatus::Scheduled,
            );
            save_job_and_event(
                &mut store,
                "processing",
                "acct_1",
                PublishJobStatus::Processing,
            );
            save_job_and_event(
                &mut store,
                "published",
                "acct_1",
                PublishJobStatus::Published,
            );
            save_job_and_event(&mut store, "failed", "acct_1", PublishJobStatus::Failed);
            save_job_and_event(
                &mut store,
                "requires_action",
                "acct_1",
                PublishJobStatus::RequiresAction,
            );

            let audit = TeamService::account_usage_audit(&store, &owner(), "acct_1")
                .unwrap_or_else(|err| panic!("audit: {err}"));

            assert_eq!(audit.status_counts.scheduled, 1);
            assert_eq!(audit.status_counts.processing, 1);
            assert_eq!(audit.status_counts.published, 1);
            assert_eq!(audit.status_counts.failed, 1);
            assert_eq!(audit.status_counts.requires_action, 1);
        }

        #[test]
        fn usage_audit_does_not_include_token_material() {
            let mut store = store_with_account(owner());
            save_job_and_event(&mut store, "job_1", "acct_1", PublishJobStatus::Published);

            let audit = TeamService::account_usage_audit(&store, &owner(), "acct_1")
                .unwrap_or_else(|err| panic!("audit: {err}"));
            let json = serde_json::to_string(&audit)
                .unwrap_or_else(|err| panic!("serialize audit: {err}"));

            assert!(!json.contains("access_token"));
            assert!(!json.contains("refresh_token"));
        }

        fn store_with_account(owner: OwnerRef) -> InMemorySocialStore {
            let mut store = InMemorySocialStore::default();
            store
                .save_connected_account(connected_account("acct_1", owner))
                .unwrap_or_else(|err| panic!("save account: {err}"));
            store
        }

        fn connected_account(id: &str, owner: OwnerRef) -> ConnectedAccount {
            ConnectedAccount {
                id: id.into(),
                owner,
                provider: Provider::YouTube,
                provider_account_id: format!("channel_{id}"),
                display_name: "Montage Channel".into(),
                handle: Some("@montage".into()),
                avatar_url: None,
                account_kind: AccountKind::Channel,
                status: ConnectedAccountStatus::Connected,
                scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
                capabilities: ProviderCapabilities {
                    upload_video: true,
                    upload_thumbnail: true,
                    public_posting: true,
                    ..ProviderCapabilities::default()
                },
                eligibility: AccountEligibility::eligible(),
                last_verified_at: None,
                created_at: 1_000,
                updated_at: 1_000,
            }
        }

        fn save_job_and_event(
            store: &mut InMemorySocialStore,
            job_id: &str,
            account_id: &str,
            status: PublishJobStatus,
        ) {
            let job = job_with_status(job_id, account_id, status);
            store
                .save_publish_job(job.clone())
                .unwrap_or_else(|err| panic!("save job: {err}"));
            store
                .append_publish_job_event(PublishJobEvent::new(
                    format!("event_{job_id}"),
                    job.id,
                    PublishJobEventType::Scheduled,
                    crate::model::PublishJobActorType::User,
                    "scheduled",
                    serde_json::json!({}),
                    1_100,
                ))
                .unwrap_or_else(|err| panic!("save event: {err}"));
        }

        fn job_with_status(job_id: &str, account_id: &str, status: PublishJobStatus) -> PublishJob {
            let job = PublishJob::new(
                job_id,
                "campaign_1",
                job_id,
                account_id,
                Provider::YouTube,
                format!("render://{job_id}"),
                2_000,
                "user_1",
            );
            match status {
                PublishJobStatus::Scheduled => job.schedule(1_000),
                PublishJobStatus::Processing => job.processing("yt_video_1", 1_000),
                PublishJobStatus::Published => job.publish(
                    "yt_video_1",
                    "https://www.youtube.com/watch?v=yt_video_1",
                    1_000,
                ),
                PublishJobStatus::Failed => job.fail(
                    "platform_processing_failed",
                    "youtube/status/yt_video_1",
                    1_000,
                ),
                PublishJobStatus::RequiresAction => job.requires_action("missing_scope", 1_000),
                PublishJobStatus::Draft
                | PublishJobStatus::Validated
                | PublishJobStatus::Uploading
                | PublishJobStatus::Cancelled => {
                    let mut job = job;
                    job.status = status;
                    job
                }
            }
        }
    }

    fn owner() -> OwnerRef {
        OwnerRef::User("user_1".into())
    }

    fn other_owner() -> OwnerRef {
        OwnerRef::Workspace("workspace_2".into())
    }
}
