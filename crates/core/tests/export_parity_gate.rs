//! Cross-renderer export-parity gate.
//!
//! The stage harness (`apps/desktop/tests/stage-harness.mjs`) pins the
//! BROWSER preview of each scene to committed per-platform goldens. This
//! gate closes the other half of the loop: the ffmpeg EXPORT of the same
//! scene must match the same golden — so "preview = export" is machine-
//! enforced, not argued from shared code paths.
//!
//! Scene of record: the `progress_bar` template (see
//! `harness_scene_progress_bar_matches_expander` in
//! `motion_template_goldens.rs`) — deliberately shapes-only, because
//! font rasterization differs between CSS and drawtext and would swamp
//! an SSIM comparison; a flat-color bar over the shared fixture clip
//! compares cleanly. Its `overlay.scale` ramp is exactly the geometry
//! channel the render lowering ships via lavfi solids, and its
//! synthesized opacity fade rides the shared alpha stage — one scene
//! exercises both channels.
//!
//! The gate renders the scene over the SAME 1280x720 fixture clip the
//! harness composites, extracts the frame at the golden's timestamp,
//! and SSIM-compares. Cross-renderer SSIM can't hit the harness's 0.98
//! (browser vs ffmpeg differ in scaling/AA/color pipelines), so the
//! threshold is calibrated with margin — see `MIN_CROSS_RENDERER_SSIM`.
//! A vacuity check compares a WRONG-timestamp frame against the same
//! golden and requires it to score below the threshold, proving the
//! comparison can actually fail (the lesson of the R25 vacuous-gate
//! episode: a gate that cannot fail is not a gate).
//!
//! Skips (with a note) when ffmpeg is unavailable, mirroring the
//! ffmpeg-smoke tests. A MISSING GOLDEN is a hard failure on platforms
//! the harness supports — never a skip — so the gate cannot silently
//! self-disable; bootstrap goldens via the stage harness
//! (`STAGE_GOLDEN_BOOTSTRAP=1`) per its header comment.

// Test-binary pragmatism, same allowance as motion_template_goldens.rs.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use montage_core::motion_scene::{MotionScenePlanRequest, plan_motion_scene_request};
use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
    TimeRange as OtioRange, Timeline, Track, TrackChild, TrackKind,
};
use montage_proto::project::files;
use montage_render::build_timeline_render_spec;
use montage_render::ffmpeg::ffmpeg_path;

/// Timestamp of the golden this gate compares against. Must match a
/// `scene-progress-bar` case in `stage-harness.mjs` CASES, chosen away
/// from animation edges (bar settled at full width/opacity) so the
/// comparison is insensitive to sub-frame time alignment.
const GOLDEN_T: f64 = 1.5;

/// Wrong-timestamp frame for the vacuity check: bar near its start
/// scale and almost fully transparent, and the underlying clip frame
/// differs — an honest comparison must reject it.
const VACUITY_T: f64 = 0.05;

/// Minimum SSIM for the export frame vs the browser golden.
///
/// Calibration (2026-07-25, darwin ffmpeg 8.1 export vs the linux
/// browser golden — a cross-platform comparison, so same-platform CI
/// scores should be at least this good): pass frame 0.9746, vacuity
/// frame 0.8352. The 0.90 threshold sits between the populations with
/// ~0.07 margin both sides. During bring-up this gate caught three
/// real export bugs (scale2ref canvas-constant misuse, frozen animated
/// scale from per-frame link renegotiation, rotate hypot-padding
/// offset) that scored 0.77–0.96 across intermediate states — the
/// margins are meaningful, not decorative. The dual-sided assertion
/// (pass > MIN AND vacuity < MIN) makes a mis-set threshold fail
/// loudly rather than pass vacuously. If the gate fails after an
/// intentional renderer change, re-run the stage harness to refresh
/// the golden first; recalibrate here only if the scene itself changed.
const MIN_CROSS_RENDERER_SSIM: f64 = 0.90;

fn desktop_fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop")
        .join(rel)
}

fn golden_platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("darwin")
    } else if cfg!(target_os = "linux") {
        Some("linux")
    } else {
        None
    }
}

/// Parse the `All:` SSIM score from ffmpeg's ssim filter stderr line,
/// e.g. `[Parsed_ssim_0 @ ...] SSIM Y:0.99 U:0.99 V:0.99 All:0.991234 (…)`.
fn parse_ssim_all(stderr: &str) -> Option<f64> {
    let idx = stderr.rfind("All:")?;
    let rest = &stderr[idx + 4..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn run_ffmpeg(ffmpeg: &Path, args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new(ffmpeg)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn ffmpeg")
}

/// Extract the frame at `t` from `video` as a PNG, scaled to the
/// golden's 1280x720. The export conforms to the 16:9 1920x1080 master
/// canvas (`timeline_render_canvas` has no 720p option), so the frame
/// takes a deterministic bicubic downscale back to golden size; the
/// clip's 720->1080->720 round trip softens the background slightly,
/// which the threshold calibration absorbs. Output seeking (`-ss`
/// before `-i` on a decoded stream) is frame-accurate in modern ffmpeg.
fn extract_frame(ffmpeg: &Path, cwd: &Path, video: &Path, t: f64, out: &Path) {
    let t = format!("{t}");
    let output = run_ffmpeg(
        ffmpeg,
        &[
            "-y",
            "-ss",
            &t,
            "-i",
            &video.to_string_lossy(),
            "-frames:v",
            "1",
            "-vf",
            "scale=1280:720:flags=bicubic",
            &out.to_string_lossy(),
        ],
        cwd,
    );
    assert!(
        output.status.success(),
        "frame extraction at t={t} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// SSIM of two same-sized images via ffmpeg's ssim filter.
fn ssim(ffmpeg: &Path, cwd: &Path, a: &Path, b: &Path) -> f64 {
    let output = run_ffmpeg(
        ffmpeg,
        &[
            "-i",
            &a.to_string_lossy(),
            "-i",
            &b.to_string_lossy(),
            "-filter_complex",
            "[0:v][1:v]ssim",
            "-f",
            "null",
            "-",
        ],
        cwd,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "ssim comparison failed:\n{stderr}");
    parse_ssim_all(&stderr)
        .unwrap_or_else(|| panic!("no SSIM All: score in ffmpeg stderr:\n{stderr}"))
}

/// Build a minimal project: the shared 3s harness fixture clip on one
/// video track, plus the progress-bar MotionScene from the SAME
/// production planning entry point the harness fixture derives from.
fn write_parity_project(dir: &Path, ffmpeg: &Path) {
    let asset_rel = "raw/x.mp4";
    std::fs::create_dir_all(dir.join("raw")).expect("mkdir raw");
    // The harness fixture clip is VIDEO-ONLY, but the timeline
    // filtergraph maps `[0:a:0]` into its concat — a video-only source
    // fails with "Stream specifier ':a:0' matches no streams". Remux
    // with a silent track, stream-copying video so the pixels the
    // golden was captured from are byte-identical.
    let src = desktop_fixture("public/fixtures/stage/clip.mp4");
    let output = run_ffmpeg(
        ffmpeg,
        &[
            "-y",
            "-i",
            &src.to_string_lossy(),
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=stereo",
            "-shortest",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            &dir.join(asset_rel).to_string_lossy(),
        ],
        dir,
    );
    assert!(
        output.status.success(),
        "remuxing fixture clip with silent audio failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut clip = Clip::empty("clip-0".to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
    clip.source_range = Some(OtioRange::new(
        RationalTime::new(0.0, 30.0),
        RationalTime::new(3.0 * 30.0, 30.0),
    ));

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));

    let mut tl = Timeline::empty("parity");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    tl.tracks = stack;

    let plan = plan_motion_scene_request(&MotionScenePlanRequest {
        request: "progress bar for the challenge".into(),
        scene_id: Some("progress-bar".into()),
        duration_s: Some(3.0),
        fps: Some(30.0),
        width: Some(1280),
        height: Some(720),
        template: Some("progress_bar".into()),
        progress: Some((0.2, 0.9, 0.08)),
        color: Some("#22D3EE".into()),
        ..MotionScenePlanRequest::default()
    })
    .expect("progress bar should plan");
    tl.metadata
        .montage
        .as_mut()
        .expect("Timeline::empty initializes montage metadata")
        .motion_scenes
        .push(plan.scene);

    std::fs::write(
        dir.join(files::OTIO),
        serde_json::to_string_pretty(&tl).expect("serialize timeline"),
    )
    .expect("write project OTIO");
}

#[test]
fn exported_frame_matches_stage_preview_golden() {
    let Ok(ffmpeg) = ffmpeg_path() else {
        eprintln!("skipping export parity gate: ffmpeg unavailable");
        return;
    };
    let Some(platform) = golden_platform() else {
        eprintln!("skipping export parity gate: no stage goldens for this platform");
        return;
    };
    let golden = desktop_fixture(&format!(
        "tests/fixtures/stage-golden/scene-progress-bar-t{GOLDEN_T}-{platform}.png"
    ));
    // Fail closed on a missing golden — mirroring the stage harness. A
    // skip here would let the gate silently self-disable on exactly the
    // platform where the comparison broke.
    assert!(
        golden.exists(),
        "stage golden missing: {}\n\
         Bootstrap it by running the stage harness once with \
         STAGE_GOLDEN_BOOTSTRAP=1 (darwin locally; linux from the CI \
         `stage-harness-screenshots` artifact) and commit the golden.",
        golden.display()
    );

    let dir = tempfile::tempdir().expect("tempdir");
    write_parity_project(dir.path(), &ffmpeg);

    let spec = build_timeline_render_spec(dir.path()).expect("render spec builds");
    let render = Command::new(&ffmpeg)
        .args(&spec.args)
        .current_dir(dir.path())
        .output()
        .expect("spawn ffmpeg render");
    assert!(
        render.status.success(),
        "export render failed\nargv: {}\nstderr:\n{}",
        spec.args.join(" "),
        String::from_utf8_lossy(&render.stderr)
    );

    let pass_frame = dir.path().join("export-frame.png");
    let vacuity_frame = dir.path().join("vacuity-frame.png");
    extract_frame(
        &ffmpeg,
        dir.path(),
        &spec.output_path,
        GOLDEN_T,
        &pass_frame,
    );
    extract_frame(
        &ffmpeg,
        dir.path(),
        &spec.output_path,
        VACUITY_T,
        &vacuity_frame,
    );

    let pass_ssim = ssim(&ffmpeg, dir.path(), &pass_frame, &golden);
    let vacuity_ssim = ssim(&ffmpeg, dir.path(), &vacuity_frame, &golden);

    // Preserve the compared frames OUTSIDE the tempdir (which drops on
    // panic) so a failure leaves inspectable evidence, mirroring the
    // stage harness keeping its screenshots.
    let debug_dir = std::env::temp_dir().join("montage-export-parity-debug");
    std::fs::create_dir_all(&debug_dir).expect("create debug dir");
    let kept_pass = debug_dir.join(format!("export-t{GOLDEN_T}.png"));
    let kept_vacuity = debug_dir.join(format!("export-t{VACUITY_T}.png"));
    std::fs::copy(&pass_frame, &kept_pass).expect("keep pass frame");
    std::fs::copy(&vacuity_frame, &kept_vacuity).expect("keep vacuity frame");
    eprintln!(
        "export parity: t={GOLDEN_T} SSIM {pass_ssim:.4} (min {MIN_CROSS_RENDERER_SSIM}); \
         vacuity t={VACUITY_T} SSIM {vacuity_ssim:.4}; frames kept in {}",
        debug_dir.display()
    );

    assert!(
        pass_ssim >= MIN_CROSS_RENDERER_SSIM,
        "exported frame diverged from the stage preview golden: \
         SSIM {pass_ssim:.4} < {MIN_CROSS_RENDERER_SSIM} \
         (export renderer no longer matches the preview for the \
         progress-bar scene — inspect {} vs {})",
        pass_frame.display(),
        golden.display()
    );
    assert!(
        vacuity_ssim < MIN_CROSS_RENDERER_SSIM,
        "vacuity check failed: a WRONG-timestamp frame scored \
         {vacuity_ssim:.4} >= {MIN_CROSS_RENDERER_SSIM} against the golden, \
         so this gate could not detect a real divergence — the scene or \
         threshold is not discriminative enough"
    );
}

#[test]
fn parse_ssim_all_reads_ffmpeg_summary_line() {
    let line = "[Parsed_ssim_0 @ 0x7f8] SSIM Y:0.987 U:0.992 V:0.991 All:0.988123 (19.2)";
    assert_eq!(parse_ssim_all(line), Some(0.988123));
    assert_eq!(parse_ssim_all("no score here"), None);
}
