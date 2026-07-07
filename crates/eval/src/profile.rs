//! House style profiles: versioned, per-house editorial targets that gates
//! score against (docs/post-house-pipeline.md §1). Every threshold traces
//! to a measured number in a named profile document, not a constant in code.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors loading a house profile.
#[derive(Debug, Error)]
pub enum ProfileError {
    /// Underlying file read failed.
    #[error("reading profile: {0}")]
    Io(#[from] std::io::Error),
    /// The document is not a valid profile.
    #[error("parsing profile: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Cold-open requirement: peak-density minute early plus a blitz opening.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdOpenSpec {
    /// Opening window measured, in seconds.
    pub window_secs: f64,
    /// Minimum cuts/min over the opening window.
    pub min_rate: f64,
    /// Peak-density minute must be at or before this minute index.
    pub last_peak_minute: usize,
}

/// Editorial-dead-zone floor: no `window_secs` stretch may run under
/// `min_rate` cuts/min.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FloorSpec {
    /// Window length in seconds.
    pub window_secs: f64,
    /// Minimum cuts/min every window must clear.
    pub min_rate: f64,
}

/// Pacing targets for one content archetype (e.g. informational, emotional).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetypeTargets {
    /// Body cuts/min band as `(min, max)`, measured after the cold open.
    pub body_band: (f64, f64),
    /// Dead-zone floor; `None` disables the floor gate for this archetype
    /// (emotional cuts legitimately hold shots for minutes).
    pub floor: Option<FloorSpec>,
}

/// A house's editorial targets. Serialized as a versioned JSON document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HouseProfile {
    /// Profile identifier (e.g. "doac").
    pub name: String,
    /// Document version, bumped on target changes.
    pub version: u32,
    /// Cold-open requirement; `None` for formats that open cold into the
    /// show (live-chrome desk formats).
    pub cold_open: Option<ColdOpenSpec>,
    /// Pacing targets keyed by archetype name.
    pub archetypes: BTreeMap<String, ArchetypeTargets>,
}

impl HouseProfile {
    /// Load a profile from a JSON document.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// Targets for `archetype`, if the profile defines it.
    pub fn archetype(&self, archetype: &str) -> Option<&ArchetypeTargets> {
        self.archetypes.get(archetype)
    }
}
