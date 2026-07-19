pub(crate) mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod model_presets;
pub mod test_support;

pub use codex_protocol::auth::AuthMode;
pub use config::ModelsManagerConfig;

// Montage fork edit: keep the Codex protocol/model discovery client version
// separate from Montage's workspace package version (0.1.0) so `/models`
// filtering remains compatible with the vendored Codex engine. Keep in sync
// with `model-provider-info`'s `OPENAI_PROVIDER_VERSION`.
const CODEX_MODEL_DISCOVERY_CLIENT_VERSION: &str = "0.144.5";

/// Load the bundled model catalog shipped with `codex-models-manager`.
pub fn bundled_models_response()
-> std::result::Result<codex_protocol::openai_models::ModelsResponse, serde_json::Error> {
    serde_json::from_str(include_str!("../models.json"))
}

/// Convert the client version string to a whole version string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    CODEX_MODEL_DISCOVERY_CLIENT_VERSION.to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_discovery_uses_codex_protocol_version() {
        assert_eq!(super::client_version_to_whole(), "0.144.5");
    }
}
