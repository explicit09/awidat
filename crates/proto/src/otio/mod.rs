//! Typed OTIO 1.x superset.
//!
//! Models the subset of OpenTimelineIO 1.x types we need for v1, plus the
//! `metadata.awidat` namespace. See `OTIO_NOTES.md` for the rationale on
//! which types are in scope and which are deliberately skipped.
//!
//! # Schema versioning
//!
//! Every OTIO node carries an `OTIO_SCHEMA` string of the form `Name.Major`.
//! Per `PLAN.md` Concern B:
//!
//! - **Known name + known major** → parsed as that variant.
//! - **Known name + unknown major** (e.g. we know `Clip.1`, file says
//!   `Clip.2`) → forward-compat: parsed as the latest known major, and a
//!   warning is logged via [`SchemaWarning`].
//! - **Unknown name** → hard fail with [`crate::ProtoError::UnknownOtioSchema`].
//!
//! Forward compat is forgiving on purpose. Hand-edited files from a future
//! Awidat or a future OTIO adapter should not crash the engine; they should
//! load with a warning the user can act on.

mod nodes;
mod schema;
mod time;

pub use nodes::{
    Clip, ClipMetadata, Effect, ExternalReference, Gap, Marker, MarkerMetadata, MediaReference,
    MissingReference, Stack, StackChild, Timeline, TimelineMetadata, Track, TrackChild, TrackKind,
    Transition,
};
pub use schema::{
    OtioSchemaName, SUPPORTED_SCHEMA_NAMES, SchemaParseOutcome, SchemaWarning, parse_schema_string,
};
pub use time::{RationalTime, TimeRange};
