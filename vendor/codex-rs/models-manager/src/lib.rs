pub(crate) mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod model_presets;
pub mod test_support;

pub use codex_app_server_protocol::AuthMode;
pub use config::ModelsManagerConfig;

const CODEX_MODEL_DISCOVERY_CLIENT_VERSION: &str = "0.128.0";

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
        assert_eq!(super::client_version_to_whole(), "0.128.0");
    }
}
