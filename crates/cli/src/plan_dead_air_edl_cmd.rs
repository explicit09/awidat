//! `montage plan-dead-air-edl` — generate a deterministic silence-cleanup EDL.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use montage_proto::project::Project;
use montage_render::SilenceRange;
use tokio_util::sync::CancellationToken;

use crate::plan_clip::{SelectedClip, resolve_asset_path, select_clip};
use crate::plan_ranges::{
    SourceRange, build_kept_ranges_edl, kept_ranges_after_removing, push_non_empty_range,
};

/// Arguments for `montage plan-dead-air-edl`.
pub struct PlanDeadAirEdlArgs {
    /// Project directory.
    pub project_root: PathBuf,
    /// Optional asset, clip name, clip uuid, file name, or file stem selector.
    pub asset: Option<String>,
    /// Minimum silence duration to remove, in seconds.
    pub min_duration_s: f64,
    /// Silence threshold in dBFS.
    pub silence_threshold_db: f64,
    /// Seconds to preserve at each side of each detected silence.
    pub keep_padding_s: f64,
}

pub fn run(args: PlanDeadAirEdlArgs) -> Result<()> {
    validate_args(&args)?;
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let clip = select_clip(&project.timeline.tracks, args.asset.as_deref())?;
    let asset_path = resolve_asset_path(&args.project_root, &clip.asset);
    let silences = detect_silences(&asset_path, args.silence_threshold_db, args.min_duration_s)?;
    let removed_ranges = silence_cuts(&clip, &silences, args.keep_padding_s);
    let kept_ranges = kept_ranges_after_removing(&clip, removed_ranges);
    let edl = build_kept_ranges_edl(&clip, &kept_ranges, "after-dead-air");
    print!("{edl}");
    Ok(())
}

fn validate_args(args: &PlanDeadAirEdlArgs) -> Result<()> {
    if !args.min_duration_s.is_finite() || args.min_duration_s <= 0.0 {
        bail!("--min-duration-s must be finite and > 0");
    }
    if !args.silence_threshold_db.is_finite() || args.silence_threshold_db >= 0.0 {
        bail!("--silence-threshold-db must be finite and < 0");
    }
    if !args.keep_padding_s.is_finite() || args.keep_padding_s < 0.0 {
        bail!("--keep-padding-s must be finite and >= 0");
    }
    Ok(())
}

fn detect_silences(
    asset_path: &Path,
    threshold_db: f64,
    min_duration_s: f64,
) -> Result<Vec<SilenceRange>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime
        .block_on(montage_render::generate_silences(
            asset_path,
            threshold_db,
            min_duration_s,
            CancellationToken::new(),
        ))
        .with_context(|| format!("failed to detect silences in {}", asset_path.display()))
}

fn silence_cuts(
    clip: &SelectedClip,
    silences: &[SilenceRange],
    keep_padding_s: f64,
) -> Vec<SourceRange> {
    let mut cuts = Vec::new();
    for silence in silences {
        let start_s = (silence.start_s + keep_padding_s).max(clip.source_start_s);
        let end_s = (silence.end_s - keep_padding_s).min(clip.source_end_s);
        push_non_empty_range(&mut cuts, start_s, end_s);
    }
    cuts
}
