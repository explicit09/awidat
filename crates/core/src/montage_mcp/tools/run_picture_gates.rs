//! `run_picture_gates` — deterministic picture-pass gates over the current
//! timeline, scored against a house style profile (montage-eval). The
//! agent's pre-publish self-check: cold open, editorial-dead-zone floor,
//! and body pacing band.

use montage_eval::{HouseProfile, PacingStats, gates};
use montage_proto::otio::{Stack, StackChild, TrackChild, TrackKind};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `run_picture_gates`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RunPictureGatesArgs {
    /// Content archetype to score against (e.g. "informational",
    /// "emotional"); must be defined by the profile.
    pub archetype: String,
    /// House profile name ("doac", "tbpn", "technologia"). Defaults to
    /// "technologia".
    #[serde(default)]
    pub profile: Option<String>,
}

/// Cut boundary times (seconds) and total duration of the first video
/// track: boundaries between consecutive duration-bearing children
/// (clips and gaps), excluding the program end. Transitions decorate an
/// existing boundary and are skipped. Shared with `plan_cold_open` for
/// gate projections.
pub(crate) fn first_video_track_cuts(stack: &Stack) -> Option<(Vec<f64>, f64)> {
    for child in &stack.children {
        match child {
            StackChild::Track(track) if track.kind == TrackKind::Video => {
                let mut cuts = Vec::new();
                let mut cursor = 0.0;
                for item in &track.children {
                    let dur = match item {
                        TrackChild::Clip(clip) => clip
                            .source_range
                            .as_ref()
                            .map(|r| r.duration.to_seconds())
                            .unwrap_or(0.0),
                        TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
                        _ => continue,
                    };
                    if cursor > 0.0 {
                        cuts.push(cursor);
                    }
                    cursor += dur;
                }
                return Some((cuts, cursor));
            }
            StackChild::Stack(inner) => {
                if let Some(found) = first_video_track_cuts(inner) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Run `run_picture_gates` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`.
pub fn run(args: RunPictureGatesArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("run_picture_gates: failed to read project: {e}"))?;

    let profile_name = args.profile.as_deref().unwrap_or("technologia");
    let profile = HouseProfile::builtin(profile_name).ok_or_else(|| {
        format!(
            "run_picture_gates: unknown profile {profile_name:?} \
             (builtins: doac, tbpn, technologia)"
        )
    })?;

    let (cuts, duration_secs) = first_video_track_cuts(&project.timeline.tracks)
        .ok_or_else(|| "run_picture_gates: no video track in timeline".to_string())?;
    let stats = PacingStats::from_cut_times(&cuts, duration_secs)
        .map_err(|e| format!("run_picture_gates: {e}"))?;

    let reports = vec![
        gates::cold_open(&stats, &profile),
        gates::floor(&stats, &profile, &args.archetype),
        gates::body_pacing(&stats, &profile, &args.archetype),
    ];
    let passed = reports.iter().all(|r| r.passed);
    let body = serde_json::json!({
        "profile": profile.name,
        "archetype": args.archetype,
        "cut_count": cuts.len(),
        "duration_secs": duration_secs,
        "reports": reports,
        "passed": passed,
    });
    serde_json::to_string(&body).map_err(|e| format!("run_picture_gates serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Run deterministic picture-pass gates over the current timeline, scored \
against a house style profile: cold_open (peak-density minute early plus a \
blitz opening window), floor (no dead editorial zone — catches shipping a \
raw composite), and body_pacing (cuts/min inside the archetype band). \
Returns per-gate reports with measured values and an overall `passed`. \
Read-only; run before export/publish and after major picture changes.\
";
