# awidat-test-support

Cross-crate test helpers. **Where to put a test helper:**

- **Per-crate-only helper** (used by tests of one crate, never by tests of
  another) → put in `<crate>/tests/common/mod.rs` (the standard Cargo
  `tests/common/` pattern). See `crates/mcp/tests/common/mod.rs` for an
  example.
- **Cross-crate helper** (or one we know we'll want from a second crate
  soon) → put it here.

The crate is `publish = false`. It's a workspace dev-dep
(`awidat-test-support = { workspace = true }` in each consumer's
`[dev-dependencies]`).

## What's here

| Helper | Purpose |
| --- | --- |
| [`fixture::tmp_dir`](src/fixture.rs) | Unique per-test temp directory (`tempfile::TempDir`). Auto-cleans on drop. |
| [`fixture::project`](src/fixture.rs) | Initialized awidat project at a fresh path. Returns the `Project` and the temp-dir handle. |
| [`mcp::test_server_command`](src/mcp.rs) | `tokio::process::Command` for the awidat-mcp test-server with the given mode env-var. |
| [`assert::assert_json_eq`](src/assert.rs) | `pretty_assertions`-style diff for two `serde_json::Value`s. |
