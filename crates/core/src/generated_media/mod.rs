//! Provider-neutral generated-media records and local registry support.

pub mod openrouter;
pub mod provider;
pub mod registry;
pub mod seedance;

#[cfg(test)]
/// Test helpers for generated-media tool unit tests.
pub mod test_support {
    use std::path::Path;
    use std::sync::Arc;

    use tokio::sync::broadcast;

    /// Build a minimal tool context rooted at `root`.
    pub fn ctx_at(root: &Path) -> crate::tool::ToolContext {
        let (tx, _) = broadcast::channel(8);
        crate::tool::ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_path_is_project_local_montage_file() {
        let root = std::path::Path::new("/tmp/episode");
        assert_eq!(
            registry::registry_path(root),
            root.join(".montage")
                .join("generated-media")
                .join("registry.json")
        );
    }

    #[test]
    fn generated_output_paths_reject_traversal() {
        assert!(registry::validate_generated_output_path("raw/generated/mock/job.mp4").is_ok());
        assert!(registry::validate_generated_output_path("../outside.mp4").is_err());
        assert!(registry::validate_generated_output_path("raw/generated/../outside.mp4").is_err());
        assert!(registry::validate_generated_output_path("/tmp/outside.mp4").is_err());
    }
}
