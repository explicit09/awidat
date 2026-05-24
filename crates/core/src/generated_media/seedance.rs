//! Seedance schema/config placeholders.
//!
//! Live API details intentionally do not live here yet. The first
//! implementation path is provider-neutral registry plumbing plus an
//! offline mock provider for tests.

/// Placeholder configuration for the future live Seedance adapter.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SeedanceConfig {
    /// Model identifier to use once the live Seedance adapter lands.
    pub model: String,
}

impl Default for SeedanceConfig {
    fn default() -> Self {
        Self {
            model: "seedance".into(),
        }
    }
}
