//! Integration tests for the `start_render` export-preset wiring.
//!
//! Ensures that when a caller supplies an export preset (e.g. "hevc" or
//! "prores"), the resulting ffmpeg argv selects the matching codec rather
//! than the legacy hardcoded libx264 path. The default — no preset — must
//! continue to produce libx264 args (backward compatibility).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use montage_core::FunctionCallError;
use montage_core::tools::start_render::build_render_argv;

fn argv_contains_pair(argv: &[String], a: &str, b: &str) -> bool {
    argv.windows(2).any(|w| w[0] == a && w[1] == b)
}

fn argv_or_panic(label: &str, result: Result<Vec<String>, FunctionCallError>) -> Vec<String> {
    match result {
        Ok(argv) => argv,
        Err(err) => panic!("{label}: expected Ok argv, got error {err:?}"),
    }
}

#[test]
fn full_scope_with_hevc_preset_selects_libx265() {
    let asset = Path::new("/proj/raw/clip.mp4");
    let out = Path::new("/proj/renders/full-clip-000000.mp4");

    let argv = argv_or_panic(
        "hevc preset",
        build_render_argv("full", asset, out, None, Some("hevc")),
    );

    assert!(
        argv_contains_pair(&argv, "-c:v", "libx265"),
        "expected '-c:v libx265' in argv, got {argv:?}"
    );
    assert!(
        !argv_contains_pair(&argv, "-c:v", "libx264"),
        "should NOT also contain libx264, got {argv:?}"
    );
}

#[test]
fn full_scope_with_prores_preset_selects_prores_ks() {
    let asset = Path::new("/proj/raw/clip.mp4");
    let out = Path::new("/proj/renders/full-clip-000000.mov");

    let argv = argv_or_panic(
        "prores preset",
        build_render_argv("full", asset, out, None, Some("prores")),
    );

    assert!(
        argv_contains_pair(&argv, "-c:v", "prores_ks"),
        "expected '-c:v prores_ks' in argv, got {argv:?}"
    );
    // ProRes HQ uses 10-bit 4:2:2 — the preset should request yuv422p10le
    // somewhere in the argv so downstream ffmpeg honors the profile.
    assert!(
        argv.iter().any(|a| a == "yuv422p10le"),
        "expected yuv422p10le pixel format in argv, got {argv:?}"
    );
}

#[test]
fn full_scope_without_preset_keeps_libx264_default() {
    let asset = Path::new("/proj/raw/clip.mp4");
    let out = Path::new("/proj/renders/full-clip-000000.mp4");

    let argv = argv_or_panic(
        "default scope",
        build_render_argv("full", asset, out, None, None),
    );

    assert!(
        argv.contains(&"libx264".to_string()),
        "expected legacy libx264 path without a preset, got {argv:?}"
    );
}

#[test]
fn preview_scope_with_hevc_preset_still_selects_libx265() {
    // Previewing with an HEVC preset should also honor the codec choice —
    // the preset is the authoritative codec source, scope only chooses the
    // size/quality envelope.
    let asset = Path::new("/proj/raw/clip.mp4");
    let out = Path::new("/proj/renders/preview-clip-000000.mp4");

    let argv = argv_or_panic(
        "hevc preview",
        build_render_argv("preview", asset, out, None, Some("hevc")),
    );

    assert!(
        argv_contains_pair(&argv, "-c:v", "libx265"),
        "expected libx265 in preview argv when hevc preset is set, got {argv:?}"
    );
}

#[test]
fn unknown_preset_returns_respond_to_model_error() {
    let asset = Path::new("/proj/raw/clip.mp4");
    let out = Path::new("/proj/renders/full-clip-000000.mp4");

    let err = match build_render_argv("full", asset, out, None, Some("av1")) {
        Ok(argv) => panic!("unknown preset should be rejected loudly, got {argv:?}"),
        Err(err) => err,
    };

    let msg = format!("{err:?}");
    assert!(
        msg.contains("av1") || msg.contains("preset"),
        "expected a descriptive error mentioning the preset, got {msg}"
    );
}
