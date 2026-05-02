//! Per-crate-only test fixtures for MCP integration tests. Cross-crate
//! helpers live in `crates/test-support/`.
//!
//! Cargo's standard `tests/common/mod.rs` shape: shared via `mod common;`
//! in each integration test file. The leading `_` on the file's `use`
//! prevents Cargo from compiling it as its own integration-test binary.

#![allow(dead_code)]

use std::path::PathBuf;

use awidat_mcp::{ClientInfo, ServerConfig};
use awidat_test_support::mcp::test_server_config;

/// Absolute path to the `awidat-mcp-test-server` binary Cargo built for
/// this test run. Cargo only sets `CARGO_BIN_EXE_*` for tests inside the
/// declaring crate, so this lookup must live here (not in test-support).
pub fn test_server_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_awidat-mcp-test-server"))
}

/// `ServerConfig` for the test-server in the given mode.
pub fn cfg(mode: &str) -> ServerConfig {
    test_server_config(&test_server_path(), mode)
}

/// Standard `ClientInfo` for tests.
pub fn client_info() -> ClientInfo {
    ClientInfo {
        name: "awidat-test".into(),
        version: "0.0.1".into(),
    }
}
