//! Sets `TS_RS_EXPORT_DIR` for ts-rs export tests.
//!
//! Normal `cargo test` writes generated bindings into Cargo's output
//! directory so tests never dirty the source tree. To intentionally refresh
//! `apps/desktop/src/protocol/generated/`, run:
//!
//! `MONTAGE_EXPORT_TS=1 cargo test -p montage-desktop-protocol`.

use std::path::PathBuf;

fn main() {
    // Workspace root is two levels up from this crate's manifest dir
    // (crates/desktop-protocol/ -> montage/).
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let export_dir = if std::env::var_os("MONTAGE_EXPORT_TS").is_some() {
        workspace_root.join("apps/desktop/src/protocol/generated")
    } else {
        PathBuf::from(std::env::var("OUT_DIR").unwrap_or_else(|_| "target".into()))
            .join("ts-bindings")
    };

    // Tell rustc to set TS_RS_EXPORT_DIR for the test binaries that run
    // the auto-generated `export_bindings_*` tests.
    println!("cargo:rustc-env=TS_RS_EXPORT_DIR={}", export_dir.display());

    // Re-run if either the manifest or this script changes (path
    // assumptions are baked in here).
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MONTAGE_EXPORT_TS");
}
