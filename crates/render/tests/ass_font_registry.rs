//! Font-registry behaviour for the libass caption writer.
//!
//! Pinned by F3 follow-up:
//!   1. `MONTAGE_CAPTION_FONT` (when set and non-empty) wins outright.
//!   2. Otherwise the helper returns a non-empty family name (host-probed
//!      with a documented fallback list).
//!   3. Whatever the helper resolves to MUST appear verbatim as the
//!      `Fontname` of the `Style: Caption,` row in the generated ASS
//!      document — no more silent `Arial` hardcode.
//!
//! Why an injectable env getter instead of `std::env::set_var`:
//! Rust 2024 made `set_var` `unsafe` (it's thread-unsafe on Unix). The
//! workspace forbids `unsafe`, so the resolver takes an
//! `impl Fn(&str) -> Option<String>` and the public wrapper plugs in
//! `std::env::var`. Tests drive the resolver with stub closures.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use montage_render::timeline::{
    CaptionWordTiming, TextReveal, TitleAnimation, TitlePlan, TitlePosition, TitleWeight,
};
use montage_render::{
    build_ass_document_for_test, default_caption_font_name, resolve_caption_font_name,
};

fn caption_fixture() -> TitlePlan {
    TitlePlan {
        text: "hello world".into(),
        start_s: 1.0,
        end_s: 2.5,
        position: TitlePosition::Bottom,
        font_size: 64,
        color: "#FFFFFF".into(),
        font_weight: TitleWeight::Bold,
        animation: TitleAnimation::None,
        phases: None,
        reveal: TextReveal::None,
        role: "caption".into(),
        safe_area: None,
        rich_segments: Vec::new(),
        word_timings: vec![
            CaptionWordTiming {
                text: "hello".into(),
                start_s: 1.0,
                end_s: 1.6,
            },
            CaptionWordTiming {
                text: "world".into(),
                start_s: 1.6,
                end_s: 2.4,
            },
        ],
        animations: Vec::new(),
        font_path: None,
        font_family: None,
        caption_style: None,
        layout_box: None,
    }
}

#[test]
fn env_override_is_honored() {
    let resolved = resolve_caption_font_name(|key| {
        if key == "MONTAGE_CAPTION_FONT" {
            Some("Comic Sans MS".to_string())
        } else {
            None
        }
    });
    assert_eq!(resolved, "Comic Sans MS");
}

#[test]
fn empty_env_override_falls_through_to_probe() {
    // Explicit empty string must NOT win — fall through to probing.
    let resolved = resolve_caption_font_name(|key| {
        if key == "MONTAGE_CAPTION_FONT" {
            Some(String::new())
        } else {
            None
        }
    });
    assert!(
        !resolved.is_empty(),
        "probe must always resolve *something* — got empty string"
    );

    // And the production wrapper (reads real env) also always returns
    // a non-empty family. Tests run with a clean env; if a developer
    // happens to have MONTAGE_CAPTION_FONT exported, the assertion still
    // holds (non-empty).
    let production_resolved = default_caption_font_name();
    assert!(
        !production_resolved.is_empty(),
        "default_caption_font_name() returned empty string"
    );
}

#[test]
fn ass_document_uses_resolved_font_name() {
    // Force the resolver via env override so this test is hermetic
    // regardless of the host's installed fonts.
    let resolved = resolve_caption_font_name(|key| {
        if key == "MONTAGE_CAPTION_FONT" {
            Some("SentinelFontF3".to_string())
        } else {
            None
        }
    });
    assert_eq!(resolved, "SentinelFontF3");

    // build_ass_document_for_test must read from the SAME env mechanism
    // the production code uses. We can't unsafely set env from a test,
    // so this assertion uses `default_caption_font_name()` (which DOES
    // read the real env) and verifies the writer keys off the helper
    // — not a hardcoded "Arial".
    let live_font = default_caption_font_name();
    let doc = build_ass_document_for_test(&caption_fixture());
    let expected_prefix = format!("Style: Caption,{live_font},");
    assert!(
        doc.contains(&expected_prefix),
        "ASS document missing `{expected_prefix}` row:\n{doc}"
    );
}
