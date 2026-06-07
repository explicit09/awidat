//! Color scope computation tests.

use montage_core::tools::color_scopes::{ColorScopeInput, compute_color_scopes};

#[test]
fn color_scopes_compute_histogram_waveform_parade_and_vectorscope() {
    let input = ColorScopeInput {
        width: 2,
        height: 2,
        bins: 4,
        rgb: vec![
            0, 0, 0, // black
            255, 255, 255, // white
            255, 0, 0, // red
            0, 0, 255, // blue
        ],
    };

    let scopes = match compute_color_scopes(&input) {
        Ok(scopes) => scopes,
        Err(err) => panic!("scope computation should succeed: {err}"),
    };

    assert_eq!(scopes.sample_count, 4);
    assert_eq!(scopes.luma_histogram.iter().sum::<u32>(), 4);
    assert_eq!(scopes.rgb_histogram.red.iter().sum::<u32>(), 4);
    assert_eq!(scopes.rgb_histogram.green.iter().sum::<u32>(), 4);
    assert_eq!(scopes.rgb_histogram.blue.iter().sum::<u32>(), 4);
    assert_eq!(scopes.rgb_parade.red.iter().flatten().sum::<u32>(), 4);
    assert_eq!(scopes.rgb_parade.green.iter().flatten().sum::<u32>(), 4);
    assert_eq!(scopes.rgb_parade.blue.iter().flatten().sum::<u32>(), 4);
    assert_eq!(scopes.waveform.iter().flatten().sum::<u32>(), 4);
    assert_eq!(scopes.vectorscope.iter().flatten().sum::<u32>(), 4);
}

#[test]
fn color_scopes_reject_malformed_rgb_buffers() {
    let input = ColorScopeInput {
        width: 2,
        height: 2,
        bins: 4,
        rgb: vec![0, 0, 0],
    };

    let err = match compute_color_scopes(&input) {
        Ok(_) => panic!("short buffer should fail"),
        Err(err) => err,
    };

    assert!(err.contains("expected 12 RGB bytes"));
}
