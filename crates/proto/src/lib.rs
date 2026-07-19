//! Montage project format types.
//!
//! This crate is the **load-bearing schema layer** for Montage. Two contracts
//! it owns must not break:
//!
//! 1. **OTIO superset.** A typed subset of OpenTimelineIO 1.x, plus the
//!    `metadata.montage` namespace. See [`otio`] and [`montage_meta`].
//!    Background: [`OTIO_NOTES.md`](../OTIO_NOTES.md) in the crate root.
//!
//! 2. **Index sidecar contract.** The shared coordinate model every footage
//!    indexer (whisper, scenedetect, audio-energy, future v1.5 indexers...)
//!    speaks. See [`index`]. Background:
//!    [`INDEX_SCHEMA.md`](../INDEX_SCHEMA.md) in the crate root.
//!
//! Both contracts are designed to accept new entrants without engine changes.
//! The engine only ever sees the shared header types; per-indexer data is
//! `serde_json::Value` to the engine and typed only inside the indexer that
//! produced it. New OTIO types and new indexers slot in by adding variants
//! / sidecar directories, never by editing engine code paths.
//!
//! # Reading and writing projects
//!
//! ```no_run
//! use montage_proto::project::Project;
//! use std::path::Path;
//!
//! let project = Project::read(Path::new("/tmp/montage-demo")).unwrap();
//! project.write(Path::new("/tmp/montage-demo")).unwrap();
//! ```

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod error;
pub mod index;
pub mod montage_meta;
pub mod otio;
pub mod professional;
pub mod project;
pub mod serde_robust;
pub mod subtitle;
pub mod transitions;
pub mod validate;

pub use error::{JsonPath, ProtoError};

/// Schema version of the montage project format.
///
/// Bumped on breaking changes to `metadata.montage` shape. Reads accept any
/// version we know about; writes always emit [`MONTAGE_PROJECT_VERSION`].
pub const MONTAGE_PROJECT_VERSION: &str = "0.1";

/// Schema version of the `index/manifest.json` file. Independent of the
/// project version because indexers and project format evolve separately.
pub const INDEX_MANIFEST_VERSION: &str = "0.1";

/// Schema version of the `edit-plan.json` file.
pub const EDIT_PLAN_VERSION: &str = "0.1";
