//! Provider-neutral generated-media records and local registry support.

pub mod openrouter;
pub mod provider;
pub mod registry;
pub mod seedance;

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
