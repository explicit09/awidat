//! EDL (Edit Decision List) parser + applier.
//!
//! `apply_edl` is the load-bearing tool of the v1 surface — see
//! `PLAN.md` §6.2. The freeform Lark-shaped envelope is parsed into a
//! typed [`EdlEnvelope`], anchors are resolved against a [`Timeline`]
//! via the 5-tier cascade in [`anchor`], then [`apply::apply`] produces
//! a new timeline + an [`apply::ApplyOutcome`] log.
//!
//! Streaming parser + diff consumer for live-preview UI lands in a
//! follow-on batch.

pub mod anchor;
pub mod apply;
pub mod bundle;
pub mod intent;
pub mod op;
pub mod parser;

pub use anchor::{AnchorContext, AnchorMiss, ClipLocator, resolve};
pub use apply::{AppliedOp, ApplyError, ApplyOutcome, apply};
pub use bundle::{DEFAULT_DISSOLVE_S, bundle_with_dissolve, envelope_needs_continuity_check};
pub use intent::{EditorialCommand, EditorialIntent, EditorialIntentResolver};
pub use op::{
    Anchor, BRollPosition, EdlEnvelope, EdlOp, InsertTrackKind, PiPCorner,
    ProfessionalTimelineEdit, TransitionBetween,
};
pub use parser::{EdlParseError, parse};
