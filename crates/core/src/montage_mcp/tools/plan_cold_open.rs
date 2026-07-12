//! `plan_cold_open` — deterministic cold-open assembler. The agent
//! supplies taste (the transplanted hook line plus ordered blitz beats,
//! per study Findings 5 and 7); this tool supplies structure: it
//! validates the plan against the house profile's cold-open spec,
//! projects the `picture.cold_open` gate verdict for the WHOLE program
//! (montage + existing body), and emits an apply_edl-ready fragment
//! inserting the montage at the head of the timeline.
//!
//! The cold open is a dual-purpose asset (the study's Finding 4): after
//! applying, reuse the same span for the 9:16 short via
//! `short-form-reframing`.

use montage_eval::{HouseProfile, PacingStats, gates};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::montage_mcp::tools::run_picture_gates::first_video_track_cuts;

/// One source segment of the cold-open montage.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ColdOpenSegment {
    /// Project-relative media path (e.g. "raw/episode.mp4").
    pub asset: String,
    /// Source in-point, seconds.
    pub start: f64,
    /// Source out-point, seconds.
    pub end: f64,
    /// Clip name on the timeline; auto-derived when omitted.
    #[serde(default)]
    pub name: Option<String>,
}

/// Arguments to `plan_cold_open`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PlanColdOpenArgs {
    /// The hook: the single most provocative line of the episode,
    /// transplanted to 0:00 (it should recur naturally in the body).
    pub hook: ColdOpenSegment,
    /// Ordered blitz beats following the hook. Aim for ~2s beats; the
    /// projection tells you whether the pacing clears the profile spec.
    pub moments: Vec<ColdOpenSegment>,
    /// House profile name; defaults to "technologia".
    #[serde(default)]
    pub profile: Option<String>,
    /// Video track to insert into; defaults to "V1".
    #[serde(default)]
    pub track: Option<String>,
}

const MAX_HOOK_SECS: f64 = 8.0;

fn segment_duration(seg: &ColdOpenSegment, label: &str) -> Result<f64, String> {
    let dur = seg.end - seg.start;
    if dur <= 0.0 {
        return Err(format!(
            "plan_cold_open: {label} has non-positive duration ({} → {})",
            seg.start, seg.end
        ));
    }
    Ok(dur)
}

/// Run `plan_cold_open` against the project resolved from [`McpToolCtx`].
pub fn run(args: PlanColdOpenArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.moments.is_empty() {
        return Err(
            "plan_cold_open: moments is empty — supply the ordered blitz beats \
             (find them with find_beat / find_moment / editorial-moments indexes)"
                .to_string(),
        );
    }
    let profile_name = args.profile.as_deref().unwrap_or("technologia");
    let profile = HouseProfile::builtin(profile_name).ok_or_else(|| {
        format!(
            "plan_cold_open: unknown profile {profile_name:?} \
             (builtins: doac, tbpn, technologia)"
        )
    })?;
    let track = args.track.as_deref().unwrap_or("V1");

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("plan_cold_open: failed to read project: {e}"))?;
    let (body_cuts, body_duration) = first_video_track_cuts(&project.timeline.tracks)
        .ok_or_else(|| "plan_cold_open: no video track in timeline".to_string())?;

    // Montage timing: a cut after every segment (the body follows the
    // last beat, so the final boundary is a cut too).
    let hook_secs = segment_duration(&args.hook, "hook")?;
    let mut montage_cuts = Vec::with_capacity(args.moments.len() + 1);
    let mut cursor = hook_secs;
    montage_cuts.push(cursor);
    for (i, m) in args.moments.iter().enumerate() {
        cursor += segment_duration(m, &format!("moments[{i}]"))?;
        montage_cuts.push(cursor);
    }
    let montage_secs = cursor;

    // Project the gate over the WHOLE future program: montage cuts, then
    // the existing body's cuts shifted by the montage length.
    let mut projected_cuts = montage_cuts.clone();
    projected_cuts.extend(body_cuts.iter().map(|t| t + montage_secs));
    let projected_stats =
        PacingStats::from_cut_times(&projected_cuts, montage_secs + body_duration)
            .map_err(|e| format!("plan_cold_open: {e}"))?;
    let gate_projection = gates::cold_open(&projected_stats, &profile);

    let mut warnings = Vec::new();
    if let Some(spec) = &profile.cold_open {
        if montage_secs < spec.window_secs * 0.5 {
            warnings.push(format!(
                "montage is {montage_secs:.1}s — well under the {:.0}s cold-open window; \
                 the body's pacing will dominate the opening",
                spec.window_secs
            ));
        }
        if montage_secs > spec.window_secs * 1.5 {
            warnings.push(format!(
                "montage is {montage_secs:.1}s — much longer than the {:.0}s cold-open \
                 window; consider tightening (reference cold opens run ~90s)",
                spec.window_secs
            ));
        }
    } else {
        warnings.push(format!(
            "profile {profile_name:?} has no cold-open requirement; the projection \
             passes as skipped"
        ));
    }
    if hook_secs > MAX_HOOK_SECS {
        warnings.push(format!(
            "hook runs {hook_secs:.1}s — hooks are one transplanted line \
             (reference hooks run 2–6s)"
        ));
    }
    if !gate_projection.passed {
        warnings.push(format!(
            "projection fails the cold-open spec: {}",
            gate_projection.detail
        ));
    }

    // EDL fragment: hook at the head, beats after it, in order.
    let mut edl = String::from("*** Begin EDL\n");
    for (pos, seg) in std::iter::once(&args.hook)
        .chain(args.moments.iter())
        .enumerate()
    {
        let default_name = if pos == 0 {
            "cold-open-hook".to_string()
        } else {
            format!("cold-open-beat-{pos:02}")
        };
        let name = seg.name.clone().unwrap_or(default_name);
        edl.push_str(&format!(
            "*** Insert Clip\n+ asset: {}\n+ track: {track}\n+ at_position: {pos}\n\
             + start: {}\n+ end: {}\n+ name: {name}\n",
            seg.asset, seg.start, seg.end
        ));
    }
    edl.push_str("*** End EDL\n");

    let body = serde_json::json!({
        "profile": profile.name,
        "montage_secs": montage_secs,
        "segment_count": args.moments.len() + 1,
        "projected_opening_rate_cpm": projected_stats.rate_in_window(
            0.0,
            profile.cold_open.as_ref().map_or(90.0, |s| s.window_secs),
        ),
        "gate_projection": gate_projection,
        "warnings": warnings,
        "edl_fragment": edl,
        "next_step": "Review the projection; pass edl_fragment to apply_edl, verify with \
                      run_picture_gates, then reuse the cold-open span as the 9:16 short \
                      via the short-form-reframing skill (it is a dual-purpose asset).",
    });
    serde_json::to_string(&body).map_err(|e| format!("plan_cold_open serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Assemble a cold-open blitz montage plan: you supply the hook (the single \
most provocative line, transplanted to 0:00) and the ordered blitz beats; \
the tool validates pacing against the house profile's cold-open spec, \
projects the picture.cold_open gate verdict for the whole program, and \
returns an apply_edl-ready fragment inserting the montage at the head of \
the timeline plus warnings. Read-only planner — nothing mutates until you \
pass edl_fragment to apply_edl. The applied cold open doubles as the 9:16 \
short (reuse via short-form-reframing).\
";
