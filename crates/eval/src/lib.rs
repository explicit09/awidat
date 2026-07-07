//! montage-eval: deterministic editorial gates and eval harness.
//!
//! Gates are Rust checks over timeline-derived data (cut boundaries,
//! durations, keep maps) scored against house-style targets — see
//! `docs/post-house-pipeline.md`. The same checks serve the production
//! pipeline (department gates) and the eval loop (tier checks).

mod cuts_io;
pub mod gates;
mod pacing;
pub mod suite;

pub use cuts_io::{CutsIoError, load_cut_times};
pub use pacing::{PacingError, PacingStats};
