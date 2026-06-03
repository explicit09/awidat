use crate::model::{OwnerRef, TeamAction, TeamRole, WorkspaceMemberRole};

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
    use crate::model::{OwnerRef, TeamAction, TeamRole, WorkspaceMemberRole};

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
}
