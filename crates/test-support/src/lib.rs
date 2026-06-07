//! Cross-crate test helpers for montage. See `README.md` for placement
//! conventions (per-crate vs. cross-crate).
//!
//! This crate is `publish = false`; it's a workspace dev-dep used only from
//! `tests/` and `#[cfg(test)]` blocks.

#![allow(clippy::expect_used)]

pub mod assert;
pub mod fixture;
pub mod mcp;
