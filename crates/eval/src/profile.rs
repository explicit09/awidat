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

/// Delivery loudness requirement for the sound pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundSpec {
    /// Integrated loudness target, LUFS (e.g. -14 for YouTube delivery).
    pub target_lufs: f64,
    /// Allowed deviation from the target, LU.
    pub tolerance_lu: f64,
    /// Maximum true peak, dBFS.
    pub max_true_peak_db: f64,
    /// Minimum J/L-cut coverage at speaker-turn boundaries (0.0–1.0);
    /// `None` disables the split-edit gate.
    #[serde(default)]
    pub min_split_edit_coverage: Option<f64>,
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
    /// Delivery loudness requirement; `None` skips the sound loudness gate.
    #[serde(default)]
    pub sound: Option<SoundSpec>,
}

impl HouseProfile {
    /// Load a profile from a JSON document.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, ProfileError> {
        let content = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&content)?)
    }

    /// A built-in profile by name (`doac`, `tbpn`, `technologia`).
    ///
    /// Builtins are compiled from the canonical fixture documents so prod
    /// callers need no filesystem access; `None` for unknown names.
    pub fn builtin(name: &str) -> Option<Self> {
        let content = match name {
            "doac" => include_str!("../fixtures/profiles/doac.json"),
            "tbpn" => include_str!("../fixtures/profiles/tbpn.json"),
            "technologia" => include_str!("../fixtures/profiles/technologia.json"),
            _ => return None,
        };
        // Compile-time-included canonical documents; a parse failure is a
        // build defect caught by the sync test, not a runtime condition.
        serde_json::from_str(content).ok()
    }

    /// Targets for `archetype`, if the profile defines it.
    pub fn archetype(&self, archetype: &str) -> Option<&ArchetypeTargets> {
        self.archetypes.get(archetype)
    }
}
