//! Awidat project format types.
//!
//! This crate is the **load-bearing schema layer** for Awidat. Two contracts
//! it owns must not break:
//!
//! 1. **OTIO superset.** A typed subset of OpenTimelineIO 1.x, plus the
//!    `metadata.awidat` namespace. See [`otio`] and [`awidat_meta`].
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
//! use awidat_proto::project::Project;
//! use std::path::Path;
//!
//! let project = Project::read(Path::new("/tmp/awidat-demo")).unwrap();
//! project.write(Path::new("/tmp/awidat-demo")).unwrap();
//! ```

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod awidat_meta;
pub mod error;
pub mod index;
pub mod otio;
pub mod professional;
pub mod project;
pub mod transitions;
pub mod validate;

pub use error::{JsonPath, ProtoError};

/// Schema version of the awidat project format.
///
/// Bumped on breaking changes to `metadata.awidat` shape. Reads accept any
/// version we know about; writes always emit [`AWIDAT_PROJECT_VERSION`].
pub const AWIDAT_PROJECT_VERSION: &str = "0.1";

/// Schema version of the `index/manifest.json` file. Independent of the
/// project version because indexers and project format evolve separately.
pub const INDEX_MANIFEST_VERSION: &str = "0.1";

/// Schema version of the `edit-plan.json` file.
pub const EDIT_PLAN_VERSION: &str = "0.1";
