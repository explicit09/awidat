//! Sets `TS_RS_EXPORT_DIR` so generated `.ts` files land in
//! `apps/desktop/src/protocol/generated/` relative to the workspace
//! root, regardless of where `cargo test` was invoked from.
//!
//! ts-rs resolves `export_to` paths against `TS_RS_EXPORT_DIR` (default:
//! the crate's `bindings/` directory). Without overriding it, our
//! `../../apps/desktop/...` paths land at `crates/apps/desktop/...`
//! because ts-rs prepends the default `bindings/` segment.

use std::path::PathBuf;

fn main() {
    // Workspace root is two levels up from this crate's manifest dir
    // (crates/desktop-protocol/ -> awidat/).
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    let export_dir = workspace_root.join("apps/desktop/src/protocol/generated");

    // Tell rustc to set TS_RS_EXPORT_DIR for the test binaries that
    // run the auto-generated `export_bindings_*` tests.
    println!(
        "cargo:rustc-env=TS_RS_EXPORT_DIR={}",
        export_dir.display()
    );

    // Re-run if either the manifest or this script changes (path
    // assumptions are baked in here).
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
}
