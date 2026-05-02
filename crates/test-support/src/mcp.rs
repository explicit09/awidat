//! Helpers for tests that drive the awidat MCP test-server subprocess.
//!
//! The `awidat-mcp-test-server` binary lives in `crates/mcp/test-server/`
//! and is built as a side effect of `cargo test`. We locate it via the
//! `CARGO_BIN_EXE_*` env var Cargo sets — but Cargo only sets that for
//! tests inside the same crate that declares the `[[bin]]`. Cross-crate
//! callers therefore go through this helper, which expects the absolute
//! path to be supplied by the caller (typically read from the env var in
//! the test crate that declared the bin as a dev-dep).

use std::collections::HashMap;
use std::path::Path;

use awidat_mcp::ServerConfig;

/// Build a [`ServerConfig`] that launches the awidat MCP test-server in the
/// given mode (`"normal"`, `"hang_on_call"`, `"progress_then_reply"`, …).
///
/// `binary_path` must be the absolute path to the test-server binary,
/// typically `env!("CARGO_BIN_EXE_awidat-mcp-test-server")` from a test
/// inside `crates/mcp`.
pub fn test_server_config(binary_path: &Path, mode: &str) -> ServerConfig {
    let mut env = HashMap::new();
    env.insert("AWIDAT_MCP_TEST_MODE".into(), mode.into());
    ServerConfig {
        name: format!("test-{mode}"),
        command: binary_path.to_string_lossy().into_owned(),
        args: vec![],
        env,
        cwd: None,
    }
}
