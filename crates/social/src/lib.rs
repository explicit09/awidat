//! Server-backed social publishing account foundation.
//!
//! This crate contains provider-agnostic account, OAuth, token, and publish-job
//! contracts. It does not perform live platform HTTP calls.

pub mod eligibility;
pub mod job;
pub mod model;
pub mod oauth;
pub mod oauth_url;
pub mod provider;
pub mod token;
pub mod token_bundle;
