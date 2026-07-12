//! montage-eval: deterministic editorial gates and eval harness.
//!
//! Gates are Rust checks over timeline-derived data (cut boundaries,
//! durations, keep maps) scored against house-style targets — see
//! `docs/post-house-pipeline.md`. The same checks serve the production
//! pipeline (department gates) and the eval loop (tier checks).

mod cuts_io;
pub mod gates;
mod measurable;
mod mechanical;
mod pacing;
mod profile;
mod run_artifacts;
mod scenario;
mod scorecard;
mod sound;
pub mod suite;

pub use cuts_io::{CutsIoError, load_cut_times};
pub use measurable::{
    AudioEnergyEvidence, CompositionEvidence, CompositionVerification, DetectorEvidence,
    DetectorParseError, DetectorRunError, FrameQualityEvidence, MeasurableReport, SidecarEvidence,
    SidecarEvidenceError, SidecarPaths, Span, ThumbnailCandidate, evaluate_measurable,
    parse_blackdetect, parse_freezedetect, parse_silencedetect, run_ffmpeg_detectors,
};
pub use mechanical::{
    CheckResult, MechanicalError, MechanicalReport, inspect_ffprobe, inspect_manifest,
    inspect_otio, run_ffprobe, run_montage_validate,
};
pub use pacing::{PacingError, PacingStats};
pub use profile::{
    ArchetypeTargets, ColdOpenSpec, FloorSpec, HouseProfile, ProfileError, SoundSpec,
};
pub use run_artifacts::{AttemptArtifacts, RunArtifactError, RunArtifacts};
pub use scenario::{
    Guards, HardGates, RepairPolicy, RepairPolicyKind, Scenario, ScenarioError, SoftGates,
};
pub use scorecard::{
    Defect, DefectSeverity, NextAction, Scorecard, ScorecardError, ScorecardStatus, StopReason,
    TierStatus, Tiers,
};
pub use sound::{LoudnessStats, SplitEditStats, parse_ebur128};
