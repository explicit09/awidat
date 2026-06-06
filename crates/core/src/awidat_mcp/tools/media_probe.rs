//! Shared media-probe helpers for planner tools.
//!
//! Both `plan_color_grade` and `plan_speed_ramp` need a clip's source
//! duration from ffprobe. The helper lives here (DRY) so neither tool owns
//! a private copy.

use std::path::Path;

/// Probe a clip's duration in seconds via ffprobe. Returns `None` on any
/// failure (ffprobe missing, non-zero exit, unparsable / non-positive
/// output) so the caller decides how loudly to fail.
pub async fn probe_duration(asset_path: &Path) -> Option<f64> {
    let ffprobe = awidat_render::ffprobe_path().ok()?;
    let output = tokio::process::Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(asset_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d > 0.0)
}
