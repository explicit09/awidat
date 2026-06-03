//! Server-backed social publishing account foundation.
//!
//! This crate contains provider-agnostic account, OAuth, token, and publish-job
//! contracts. It does not perform live platform HTTP calls.

pub mod account_service;
pub mod eligibility;
pub mod job;
pub mod model;
pub mod oauth;
pub mod oauth_url;
pub mod provider;
pub mod publish_service;
pub mod sqlite_store;
pub mod store;
pub mod token;
pub mod token_bundle;
pub mod upload_adapter;
pub mod youtube_upload;
