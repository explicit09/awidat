//! Server-backed social publishing account foundation.
//!
//! This crate contains provider-agnostic account, OAuth, token, and publish-job
//! contracts. It does not perform live platform HTTP calls.

pub mod account_service;
pub mod api;
pub mod auth_context;
pub mod eligibility;
pub mod instagram_upload;
pub mod job;
pub mod model;
pub mod oauth;
pub mod oauth_exchange;
pub mod oauth_url;
pub mod pg_store;
pub mod platform_metadata;
pub mod provider;
pub mod publish_service;
pub mod sqlite_store;
pub mod store;
pub mod team_service;
pub mod tiktok_upload;
pub mod token;
pub mod token_bundle;
pub mod token_refresh;
pub mod twitter_x_upload;
pub mod upload_adapter;
pub mod upload_service;
pub mod upload_status;
pub mod youtube_upload;
