//! `verify_render` agent tool.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, Stack, StackChild, Timeline, Track, TrackChild};
use awidat_proto::project::Project;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

const DEFAULT_DURATION_TOLERANCE_S: f64 = 0.25;
const DEFAULT_SILENCE_THRESHOLD_DB: f64 = -45.0;
const DEFAULT_MAX_UNEXPECTED_SILENCE_S: f64 = 1.5;
const DEFAULT_BLACK_MIN_DURATION_S: f64 = 0.2;
const DEFAULT_MAX_BLACK_SEGMENT_S: f64 = 0.5;
const DEFAULT_SOURCE_RANGE_TOLERANCE_S: f64 = 0.05;
const BOUNDARY_MARGIN_S: f64 = 0.2;

/// Verify an already-rendered output against the current project timeline.
pub struct VerifyRenderTool;

#[derive(Debug, Deserialize)]
struct VerifyRenderArgs {
    /// Render path. Project-relative paths are resolved from project root.
    #[serde(default)]
    output_path: Option<String>,
    /// Explicit expected duration. Defaults to the current timeline duration.
    #[serde(default)]
    expected_duration_s: Option<f64>,
    /// Allowed absolute duration drift. Default 0.25s.
    #[serde(default)]
    duration_tolerance_s: Option<f64>,
    /// quick checks near clip boundaries; thorough also adds mid-clip probes.
    #[serde(default)]
    mode: Option<String>,
    /// Optional edit_manifest.json/source-range manifest to compare to the timeline.
    #[serde(default)]
    source_range_manifest_path: Option<String>,
    /// FFmpeg silencedetect threshold in dBFS. Default -45.
    #[serde(default)]
    silence_threshold_db: Option<f64>,
    /// Silence ranges at least this long are flagged when they overlap edited content.
    #[serde(default)]
    max_unexpected_silence_s: Option<f64>,
    /// Minimum blackdetect segment duration. Default 0.2s.
    #[serde(default)]
    black_min_duration_s: Option<f64>,
    /// Black ranges longer than this fail the render. Default 0.5s.
    #[serde(default)]
    max_black_segment_s: Option<f64>,
}

#[derive(Debug, Clone)]
struct VerifyRenderOptions {
    expected_duration_s: Option<f64>,
    duration_tolerance_s: f64,
    mode: VerifyMode,
    source_range_manifest_path: Option<PathBuf>,
    silence_threshold_db: f64,
    max_unexpected_silence_s: f64,
    black_min_duration_s: f64,
    max_black_segment_s: f64,
}

impl Default for VerifyRenderOptions {
    fn default() -> Self {
        Self {
            expected_duration_s: None,
            duration_tolerance_s: DEFAULT_DURATION_TOLERANCE_S,
            mode: VerifyMode::Quick,
            source_range_manifest_path: None,
            silence_threshold_db: DEFAULT_SILENCE_THRESHOLD_DB,
            max_unexpected_silence_s: DEFAULT_MAX_UNEXPECTED_SILENCE_S,
            black_min_duration_s: DEFAULT_BLACK_MIN_DURATION_S,
            max_black_segment_s: DEFAULT_MAX_BLACK_SEGMENT_S,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VerifyMode {
    Quick,
    Thorough,
}

#[derive(Debug, Serialize)]
struct VerifyRenderReport {
    passed: bool,
    mode: VerifyMode,
    output_path: String,
    expected_duration_s: f64,
    actual_duration_s: Option<f64>,
    gates: Vec<VerificationGate>,
    timeline_manifest: TimelineManifest,
}

#[derive(Debug, Serialize)]
struct VerificationGate {
    name: String,
    passed: bool,
    message: String,
    details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
struct TimelineManifest {
    expected_duration_s: f64,
    source_ranges: Vec<SourceRangeEntry>,
    missing_media: Vec<MissingMediaEntry>,
    boundary_probes: Vec<BoundaryProbe>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SourceRangeEntry {
    clip_name: String,
    asset: String,
    timeline_start_s: f64,
    timeline_end_s: f64,
    source_start_s: f64,
    source_end_s: f64,
}

#[derive(Debug, Clone, Serialize)]
struct MissingMediaEntry {
    clip_name: String,
    reference_name: String,
    timeline_start_s: f64,
    timeline_end_s: f64,
}

#[derive(Debug, Clone, Serialize)]
struct BoundaryProbe {
    clip_name: String,
    asset: String,
    export_time_s: f64,
    expected_source_time_s: f64,
    label: String,
}

#[derive(Debug, Deserialize)]
struct SourceRangeManifest {
    #[serde(alias = "source_ranges")]
    final_kept_source_ranges: Vec<SourceRangeEntry>,
}

#[derive(Debug, Serialize)]
struct SourceRangeManifestCheck {
    passed: bool,
    mismatches: Vec<String>,
}

#[async_trait]
impl ToolHandler for VerifyRenderTool {
    fn name(&self) -> &'static str {
        "verify_render"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "verify_render".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "output_path": {
                        "type": "string",
                        "description": "Rendered MP4 to verify. Project-relative paths resolve from the project root. Defaults to the newest MP4 in renders/."
                    },
                    "expected_duration_s": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "description": "Expected rendered duration. Defaults to the current project timeline duration."
                    },
                    "duration_tolerance_s": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "description": "Allowed duration drift in seconds. Default 0.25."
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["quick", "thorough"],
                        "description": "quick checks clip boundary probes; thorough also includes mid-clip probes."
                    },
                    "source_range_manifest_path": {
                        "type": "string",
                        "description": "Optional edit_manifest.json/source-range manifest to compare with project.otio.json."
                    },
                    "silence_threshold_db": {
                        "type": "number",
                        "exclusiveMaximum": 0.0,
                        "description": "FFmpeg silencedetect threshold in dBFS. Default -45."
                    },
                    "max_unexpected_silence_s": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "description": "Fail silence ranges at least this long when they overlap edited content. Default 1.5."
                    },
                    "black_min_duration_s": {
                        "type": "number",
                        "exclusiveMinimum": 0.0,
                        "description": "Minimum blackdetect segment duration. Default 0.2."
                    },
                    "max_black_segment_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Fail black ranges longer than this. Default 0.5."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: VerifyRenderArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "verify_render: invalid args ({e}). All fields are optional."
            ))
        })?;
        let output_path = match args.output_path.as_deref() {
            Some(path) => resolve_project_path(&ctx.project_root, path, "output_path")?,
            None => newest_render(&ctx.project_root)?,
        };
        if !output_path.is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "verify_render: output_path not found at {}",
                output_path.display()
            )));
        }
        let options = VerifyRenderOptions {
            expected_duration_s: args.expected_duration_s,
            duration_tolerance_s: args
                .duration_tolerance_s
                .unwrap_or(DEFAULT_DURATION_TOLERANCE_S),
            mode: parse_mode(args.mode.as_deref())?,
            source_range_manifest_path: args
                .source_range_manifest_path
                .as_deref()
                .map(|path| {
                    resolve_project_path(&ctx.project_root, path, "source_range_manifest_path")
                })
                .transpose()?,
            silence_threshold_db: args
                .silence_threshold_db
                .unwrap_or(DEFAULT_SILENCE_THRESHOLD_DB),
            max_unexpected_silence_s: args
                .max_unexpected_silence_s
                .unwrap_or(DEFAULT_MAX_UNEXPECTED_SILENCE_S),
            black_min_duration_s: args
                .black_min_duration_s
                .unwrap_or(DEFAULT_BLACK_MIN_DURATION_S),
            max_black_segment_s: args
                .max_black_segment_s
                .unwrap_or(DEFAULT_MAX_BLACK_SEGMENT_S),
        };
        validate_options(&options)?;

        let report = verify_render_output(&ctx.project_root, &output_path, options).await?;
        serde_json::to_string_pretty(&report)
            .map(ToolOutput::text)
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("verify_render: encode report: {e}"))
            })
    }
}

async fn verify_render_output(
    project_root: &Path,
    output_path: &Path,
    options: VerifyRenderOptions,
) -> Result<VerifyRenderReport, FunctionCallError> {
    let project = Project::read(project_root).map_err(|e| {
        FunctionCallError::RespondToModel(format!("verify_render: failed to read project: {e}"))
    })?;
    let mut timeline_manifest = collect_timeline_manifest(&project.timeline);
    if options.mode == VerifyMode::Quick {
        retain_quick_probes(&mut timeline_manifest.boundary_probes);
    }

    let mut gates = Vec::new();
    add_missing_media_gate(&mut gates, project_root, &timeline_manifest);

    let probe = awidat_render::probe_media(output_path).await.map_err(|e| {
        FunctionCallError::RespondToModel(format!("verify_render: ffprobe failed: {e}"))
    })?;
    push_gate(
        &mut gates,
        "has_video_stream",
        probe.has_video,
        "rendered output contains at least one video stream",
        json!({"stream_types": probe.stream_types}),
    );
    push_gate(
        &mut gates,
        "has_audio_stream",
        probe.has_audio,
        "rendered output contains at least one audio stream",
        json!({"stream_types": probe.stream_types}),
    );

    let expected_duration_s = options
        .expected_duration_s
        .unwrap_or(timeline_manifest.expected_duration_s);
    let duration_passed = probe
        .duration_s
        .map(|actual| (actual - expected_duration_s).abs() <= options.duration_tolerance_s)
        .unwrap_or(false);
    push_gate(
        &mut gates,
        "duration_match",
        duration_passed,
        "rendered duration is within tolerance",
        json!({
            "expected_duration_s": expected_duration_s,
            "actual_duration_s": probe.duration_s,
            "tolerance_s": options.duration_tolerance_s,
        }),
    );

    add_source_duration_gate(&mut gates, project_root, &timeline_manifest.source_ranges).await;
    add_source_range_manifest_gate(&mut gates, &timeline_manifest, &options)?;

    let silence_ranges = awidat_render::generate_silences(
        output_path,
        options.silence_threshold_db,
        options.max_unexpected_silence_s,
        CancellationToken::new(),
    )
    .await
    .map_err(|e| {
        FunctionCallError::RespondToModel(format!("verify_render: silencedetect failed: {e}"))
    })?;
    let unexpected_silences = silence_ranges
        .into_iter()
        .filter(|range| {
            overlaps_content(range.start_s, range.end_s, &timeline_manifest.source_ranges)
        })
        .collect::<Vec<_>>();
    push_gate(
        &mut gates,
        "no_long_unexpected_silence",
        unexpected_silences.is_empty(),
        "no long silence range overlaps edited timeline content",
        json!({
            "threshold_db": options.silence_threshold_db,
            "max_unexpected_silence_s": options.max_unexpected_silence_s,
            "ranges": unexpected_silences.iter().map(|range| json!({
                "start_s": range.start_s,
                "end_s": range.end_s,
                "duration_s": range.end_s - range.start_s,
                "db_floor": range.db_floor,
            })).collect::<Vec<_>>(),
        }),
    );

    let black_ranges = awidat_render::generate_black_frames(
        output_path,
        0.98,
        options.black_min_duration_s,
        CancellationToken::new(),
    )
    .await
    .map_err(|e| {
        FunctionCallError::RespondToModel(format!("verify_render: blackdetect failed: {e}"))
    })?;
    let long_black_ranges = black_ranges
        .into_iter()
        .filter(|range| range.duration_s > options.max_black_segment_s)
        .collect::<Vec<_>>();
    push_gate(
        &mut gates,
        "no_long_black_segment",
        long_black_ranges.is_empty(),
        "no black-frame range exceeds max_black_segment_s",
        json!({
            "black_min_duration_s": options.black_min_duration_s,
            "max_black_segment_s": options.max_black_segment_s,
            "ranges": long_black_ranges.iter().map(|range| json!({
                "start_s": range.start_s,
                "end_s": range.end_s,
                "duration_s": range.duration_s,
            })).collect::<Vec<_>>(),
        }),
    );

    add_boundary_probe_gate(
        &mut gates,
        probe.duration_s,
        &timeline_manifest.boundary_probes,
        &unexpected_silences,
        &long_black_ranges,
    );

    Ok(VerifyRenderReport {
        passed: gates.iter().all(|gate| gate.passed),
        mode: options.mode,
        output_path: output_path.display().to_string(),
        expected_duration_s,
        actual_duration_s: probe.duration_s,
        gates,
        timeline_manifest,
    })
}

fn collect_timeline_manifest(timeline: &Timeline) -> TimelineManifest {
    let mut source_ranges = Vec::new();
    let mut missing_media = Vec::new();
    let mut boundary_probes = Vec::new();
    let expected_duration_s = collect_stack_manifest(
        &timeline.tracks,
        0.0,
        &mut source_ranges,
        &mut missing_media,
        &mut boundary_probes,
    );
    TimelineManifest {
        expected_duration_s,
        source_ranges,
        missing_media,
        boundary_probes,
    }
}

fn collect_stack_manifest(
    stack: &Stack,
    timeline_start_s: f64,
    ranges: &mut Vec<SourceRangeEntry>,
    missing: &mut Vec<MissingMediaEntry>,
    probes: &mut Vec<BoundaryProbe>,
) -> f64 {
    let mut max_duration_s = 0.0_f64;
    for child in &stack.children {
        let duration_s = match child {
            StackChild::Track(track) => {
                collect_track_manifest(track, timeline_start_s, ranges, missing, probes)
            }
            StackChild::Stack(stack) => {
                collect_stack_manifest(stack, timeline_start_s, ranges, missing, probes)
            }
            StackChild::Clip(clip) => {
                collect_clip_manifest(clip, timeline_start_s, ranges, missing, probes);
                clip_duration_s(clip)
            }
            StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        };
        max_duration_s = max_duration_s.max(duration_s);
    }
    max_duration_s
}

fn collect_track_manifest(
    track: &Track,
    timeline_start_s: f64,
    ranges: &mut Vec<SourceRangeEntry>,
    missing: &mut Vec<MissingMediaEntry>,
    probes: &mut Vec<BoundaryProbe>,
) -> f64 {
    let mut cursor_s = timeline_start_s;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                collect_clip_manifest(clip, cursor_s, ranges, missing, probes);
                cursor_s += clip_duration_s(clip);
            }
            TrackChild::Gap(gap) => cursor_s += gap.source_range.duration.to_seconds(),
            TrackChild::Transition(_) => {}
            TrackChild::Stack(stack) => {
                cursor_s += collect_stack_manifest(stack, cursor_s, ranges, missing, probes);
            }
        }
    }
    cursor_s - timeline_start_s
}

fn collect_clip_manifest(
    clip: &awidat_proto::otio::Clip,
    timeline_start_s: f64,
    ranges: &mut Vec<SourceRangeEntry>,
    missing: &mut Vec<MissingMediaEntry>,
    probes: &mut Vec<BoundaryProbe>,
) {
    if !clip.active {
        return;
    }
    let Some(source_range) = &clip.source_range else {
        return;
    };
    let duration_s = source_range.duration.to_seconds();
    if duration_s <= 0.0 {
        return;
    }
    if let MediaReference::Missing(reference) = &clip.media_reference {
        missing.push(MissingMediaEntry {
            clip_name: clip.name.clone(),
            reference_name: reference.name.clone(),
            timeline_start_s,
            timeline_end_s: timeline_start_s + duration_s,
        });
        return;
    }
    let MediaReference::External(reference) = &clip.media_reference else {
        return;
    };
    let source_start_s = source_range.start_time.to_seconds();
    let source_end_s = source_start_s + duration_s;
    let timeline_end_s = timeline_start_s + duration_s;
    ranges.push(SourceRangeEntry {
        clip_name: clip.name.clone(),
        asset: reference.target_url.clone(),
        timeline_start_s,
        timeline_end_s,
        source_start_s,
        source_end_s,
    });

    let margin_s = BOUNDARY_MARGIN_S.min(duration_s * 0.2);
    push_probe(
        probes,
        clip,
        reference.target_url.as_str(),
        timeline_start_s + margin_s,
        source_start_s + margin_s,
        "start",
    );
    if duration_s > margin_s * 2.0 + 0.1 {
        push_probe(
            probes,
            clip,
            reference.target_url.as_str(),
            timeline_end_s - margin_s,
            source_end_s - margin_s,
            "end",
        );
    }
    if duration_s > 1.0 {
        let mid_offset_s = duration_s / 2.0;
        push_probe(
            probes,
            clip,
            reference.target_url.as_str(),
            timeline_start_s + mid_offset_s,
            source_start_s + mid_offset_s,
            "mid",
        );
    }
}

fn push_probe(
    probes: &mut Vec<BoundaryProbe>,
    clip: &awidat_proto::otio::Clip,
    asset: &str,
    export_time_s: f64,
    expected_source_time_s: f64,
    label: &str,
) {
    probes.push(BoundaryProbe {
        clip_name: clip.name.clone(),
        asset: asset.into(),
        export_time_s,
        expected_source_time_s,
        label: label.into(),
    });
}

fn retain_quick_probes(probes: &mut Vec<BoundaryProbe>) {
    probes.retain(|probe| probe.label != "mid");
}

fn clip_duration_s(clip: &awidat_proto::otio::Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.duration.to_seconds())
        .unwrap_or(0.0)
}

fn add_missing_media_gate(
    gates: &mut Vec<VerificationGate>,
    project_root: &Path,
    timeline_manifest: &TimelineManifest,
) {
    let mut missing = timeline_manifest
        .missing_media
        .iter()
        .map(|entry| {
            json!({
                "clip_name": entry.clip_name,
                "reference_name": entry.reference_name,
                "timeline_start_s": entry.timeline_start_s,
                "timeline_end_s": entry.timeline_end_s,
                "reason": "missing_reference",
            })
        })
        .collect::<Vec<_>>();
    missing.extend(timeline_manifest.source_ranges.iter().filter_map(|range| {
        let path = asset_path(project_root, &range.asset);
        (!path.is_file()).then(|| {
            json!({
                "clip_name": range.clip_name,
                "asset": range.asset,
                "path": path,
                "reason": "file_not_found",
            })
        })
    }));
    push_gate(
        gates,
        "no_missing_media",
        missing.is_empty(),
        "all active timeline source media files exist",
        json!({ "missing": missing }),
    );
}

async fn add_source_duration_gate(
    gates: &mut Vec<VerificationGate>,
    project_root: &Path,
    ranges: &[SourceRangeEntry],
) {
    let mut failures = Vec::new();
    for range in ranges {
        let path = asset_path(project_root, &range.asset);
        if !path.is_file() {
            continue;
        }
        match awidat_render::probe_duration_s(&path).await {
            Ok(Some(duration_s))
                if range.source_end_s > duration_s + DEFAULT_SOURCE_RANGE_TOLERANCE_S =>
            {
                failures.push(json!({
                    "clip_name": range.clip_name,
                    "asset": range.asset,
                    "source_end_s": range.source_end_s,
                    "asset_duration_s": duration_s,
                }));
            }
            Ok(_) => {}
            Err(err) => failures.push(json!({
                "clip_name": range.clip_name,
                "asset": range.asset,
                "error": err.to_string(),
            })),
        }
    }
    push_gate(
        gates,
        "source_ranges_within_media",
        failures.is_empty(),
        "timeline source ranges fit inside their source media durations",
        json!({ "failures": failures }),
    );
}

fn add_source_range_manifest_gate(
    gates: &mut Vec<VerificationGate>,
    timeline_manifest: &TimelineManifest,
    options: &VerifyRenderOptions,
) -> Result<(), FunctionCallError> {
    let Some(path) = &options.source_range_manifest_path else {
        push_gate(
            gates,
            "source_range_manifest_consistent",
            true,
            "no source_range_manifest_path supplied; checked current timeline source ranges only",
            json!({"status": "skipped"}),
        );
        return Ok(());
    };
    let bytes = std::fs::read(path).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: read source_range_manifest_path {}: {e}",
            path.display()
        ))
    })?;
    let expected: SourceRangeManifest = serde_json::from_slice(&bytes).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: parse source_range_manifest_path {}: {e}",
            path.display()
        ))
    })?;
    let check = compare_source_range_manifest(
        timeline_manifest,
        &expected,
        DEFAULT_SOURCE_RANGE_TOLERANCE_S,
    );
    push_gate(
        gates,
        "source_range_manifest_consistent",
        check.passed,
        "current timeline source ranges match supplied manifest",
        json!({
            "manifest_path": path,
            "mismatches": check.mismatches,
        }),
    );
    Ok(())
}

fn compare_source_range_manifest(
    current: &TimelineManifest,
    expected: &SourceRangeManifest,
    tolerance_s: f64,
) -> SourceRangeManifestCheck {
    let mut mismatches = Vec::new();
    if current.source_ranges.len() != expected.final_kept_source_ranges.len() {
        mismatches.push(format!(
            "range count differs: current={} manifest={}",
            current.source_ranges.len(),
            expected.final_kept_source_ranges.len()
        ));
    }
    for (idx, (left, right)) in current
        .source_ranges
        .iter()
        .zip(expected.final_kept_source_ranges.iter())
        .enumerate()
    {
        if left.clip_name != right.clip_name || left.asset != right.asset {
            mismatches.push(format!(
                "range {idx} identity differs: current={}/{} manifest={}/{}",
                left.clip_name, left.asset, right.clip_name, right.asset
            ));
        }
        for (field, a, b) in [
            (
                "timeline_start_s",
                left.timeline_start_s,
                right.timeline_start_s,
            ),
            ("timeline_end_s", left.timeline_end_s, right.timeline_end_s),
            ("source_start_s", left.source_start_s, right.source_start_s),
            ("source_end_s", left.source_end_s, right.source_end_s),
        ] {
            if (a - b).abs() > tolerance_s {
                mismatches.push(format!(
                    "range {idx} {field} differs: current={a:.3} manifest={b:.3}"
                ));
            }
        }
    }
    SourceRangeManifestCheck {
        passed: mismatches.is_empty(),
        mismatches,
    }
}

fn add_boundary_probe_gate(
    gates: &mut Vec<VerificationGate>,
    duration_s: Option<f64>,
    probes: &[BoundaryProbe],
    unexpected_silences: &[awidat_render::SilenceRange],
    long_black_ranges: &[awidat_render::BlackFrameRange],
) {
    let checks = probes
        .iter()
        .map(|probe| {
            let in_duration = duration_s
                .map(|duration| probe.export_time_s <= duration + DEFAULT_DURATION_TOLERANCE_S)
                .unwrap_or(false);
            let in_silence = unexpected_silences
                .iter()
                .any(|range| contains_time(range.start_s, range.end_s, probe.export_time_s));
            let in_black = long_black_ranges
                .iter()
                .any(|range| contains_time(range.start_s, range.end_s, probe.export_time_s));
            json!({
                "clip_name": probe.clip_name,
                "asset": probe.asset,
                "label": probe.label,
                "export_time_s": probe.export_time_s,
                "expected_source_time_s": probe.expected_source_time_s,
                "passed": in_duration && !in_silence && !in_black,
                "in_duration": in_duration,
                "in_unexpected_silence": in_silence,
                "in_long_black_segment": in_black,
            })
        })
        .collect::<Vec<_>>();
    let passed = checks
        .iter()
        .all(|check| check["passed"].as_bool().unwrap_or(false));
    push_gate(
        gates,
        "edited_boundary_probes",
        passed,
        "edited boundary probes land inside the render and avoid flagged black/silence ranges",
        json!({ "probes": checks }),
    );
}

fn overlaps_content(start_s: f64, end_s: f64, ranges: &[SourceRangeEntry]) -> bool {
    ranges
        .iter()
        .any(|range| start_s < range.timeline_end_s && end_s > range.timeline_start_s)
}

fn contains_time(start_s: f64, end_s: f64, t_s: f64) -> bool {
    t_s >= start_s && t_s <= end_s
}

fn push_gate(
    gates: &mut Vec<VerificationGate>,
    name: &str,
    passed: bool,
    message: &str,
    details: serde_json::Value,
) {
    gates.push(VerificationGate {
        name: name.into(),
        passed,
        message: message.into(),
        details,
    });
}

fn parse_mode(mode: Option<&str>) -> Result<VerifyMode, FunctionCallError> {
    match mode.unwrap_or("quick") {
        "quick" => Ok(VerifyMode::Quick),
        "thorough" => Ok(VerifyMode::Thorough),
        other => Err(FunctionCallError::RespondToModel(format!(
            "verify_render: mode must be 'quick' or 'thorough', got {other:?}"
        ))),
    }
}

fn validate_options(options: &VerifyRenderOptions) -> Result<(), FunctionCallError> {
    for (name, value) in [
        ("duration_tolerance_s", options.duration_tolerance_s),
        ("max_unexpected_silence_s", options.max_unexpected_silence_s),
        ("black_min_duration_s", options.black_min_duration_s),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(FunctionCallError::RespondToModel(format!(
                "verify_render: {name} must be finite and > 0"
            )));
        }
    }
    if !options.max_black_segment_s.is_finite() || options.max_black_segment_s < 0.0 {
        return Err(FunctionCallError::RespondToModel(
            "verify_render: max_black_segment_s must be finite and non-negative".into(),
        ));
    }
    if !options.silence_threshold_db.is_finite() || options.silence_threshold_db >= 0.0 {
        return Err(FunctionCallError::RespondToModel(
            "verify_render: silence_threshold_db must be finite and < 0".into(),
        ));
    }
    if let Some(expected) = options.expected_duration_s
        && (!expected.is_finite() || expected <= 0.0)
    {
        return Err(FunctionCallError::RespondToModel(
            "verify_render: expected_duration_s must be finite and > 0".into(),
        ));
    }
    Ok(())
}

fn resolve_project_path(
    project_root: &Path,
    value: &str,
    field: &str,
) -> Result<PathBuf, FunctionCallError> {
    if value.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(format!(
            "verify_render: {field} must not be empty"
        )));
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        return Ok(path);
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return Err(FunctionCallError::RespondToModel(format!(
            "verify_render: {field} must be project-relative and must not contain '..'"
        )));
    }
    Ok(project_root.join(path))
}

fn asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let path = PathBuf::from(asset);
    if path.is_absolute() {
        path
    } else {
        project_root.join(path)
    }
}

fn newest_render(project_root: &Path) -> Result<PathBuf, FunctionCallError> {
    let renders_dir = project_root.join("renders");
    let entries = std::fs::read_dir(&renders_dir).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: failed to read renders directory {}: {e}",
            renders_dir.display()
        ))
    })?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            FunctionCallError::RespondToModel(format!("verify_render: read renders entry: {e}"))
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("mp4") {
            continue;
        }
        let modified = entry.metadata().and_then(|m| m.modified()).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "verify_render: read metadata for {}: {e}",
                path.display()
            ))
        })?;
        candidates.push((modified, path));
    }
    candidates.sort_by_key(|(modified, _)| *modified);
    candidates.pop().map(|(_, path)| path).ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "verify_render: no MP4 found under renders/; pass output_path".into(),
        )
    })
}

const DESCRIPTION: &str = "\
Verify an existing rendered MP4 against the current Awidat timeline. \
Checks duration, audio/video stream presence, missing media, long black \
segments, long unexpected silence, source-range manifest consistency, and \
edited-boundary probes. This tool is read-only and does not start, poll, or \
change render jobs; call start_render/poll_render separately, then pass the \
finished output_path here.";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
    };
    use awidat_proto::project::Project;
    use std::process::Command;

    fn range(start_s: f64, duration_s: f64) -> TimeRange {
        TimeRange::new(
            RationalTime::new(start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        )
    }

    fn clip(name: &str, asset: &str, source_start_s: f64, duration_s: f64) -> Clip {
        let mut clip = Clip::empty(name);
        clip.source_range = Some(range(source_start_s, duration_s));
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip
    }

    #[test]
    fn collect_timeline_manifest_tracks_duration_and_boundaries() {
        let mut timeline = awidat_proto::otio::Timeline::empty("verify");
        let mut track = Track::empty("v1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Clip(clip("intro", "raw/source.mp4", 1.0, 2.0)));
        track
            .children
            .push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                0.5, 24.0,
            )));
        track
            .children
            .push(TrackChild::Clip(clip("close", "raw/source.mp4", 5.0, 1.5)));
        timeline.tracks.children.push(StackChild::Track(track));

        let manifest = collect_timeline_manifest(&timeline);

        assert!((manifest.expected_duration_s - 4.0).abs() < 1e-9);
        assert_eq!(manifest.source_ranges.len(), 2);
        assert_eq!(manifest.boundary_probes.len(), 6);
        assert_eq!(manifest.source_ranges[0].clip_name, "intro");
        assert!((manifest.source_ranges[1].timeline_start_s - 2.5).abs() < 1e-9);
        assert!((manifest.source_ranges[1].source_end_s - 6.5).abs() < 1e-9);
    }

    #[test]
    fn compare_source_range_manifest_detects_drift() {
        let current = TimelineManifest {
            expected_duration_s: 2.0,
            source_ranges: vec![SourceRangeEntry {
                clip_name: "a".into(),
                asset: "raw/a.mp4".into(),
                timeline_start_s: 0.0,
                timeline_end_s: 2.0,
                source_start_s: 0.0,
                source_end_s: 2.0,
            }],
            missing_media: Vec::new(),
            boundary_probes: Vec::new(),
        };
        let expected = SourceRangeManifest {
            final_kept_source_ranges: vec![SourceRangeEntry {
                clip_name: "a".into(),
                asset: "raw/a.mp4".into(),
                timeline_start_s: 0.0,
                timeline_end_s: 2.0,
                source_start_s: 1.0,
                source_end_s: 2.0,
            }],
        };

        let check = compare_source_range_manifest(&current, &expected, 0.01);

        assert!(!check.passed);
        assert_eq!(check.mismatches.len(), 1);
        assert!(check.mismatches[0].contains("source_start_s"));
    }

    #[test]
    fn missing_reference_clip_fails_missing_media_gate() {
        let mut timeline = awidat_proto::otio::Timeline::empty("verify");
        let mut track = Track::empty("v1", TrackKind::Video);
        let mut planned = Clip::empty("planned b-roll");
        planned.source_range = Some(range(0.0, 1.0));
        track.children.push(TrackChild::Clip(planned));
        timeline.tracks.children.push(StackChild::Track(track));

        let manifest = collect_timeline_manifest(&timeline);
        let mut gates = Vec::new();
        let dir = tempfile::tempdir().unwrap();

        add_missing_media_gate(&mut gates, dir.path(), &manifest);

        assert_eq!(manifest.missing_media.len(), 1);
        assert_eq!(manifest.missing_media[0].clip_name, "planned b-roll");
        let gate = gates
            .iter()
            .find(|gate| gate.name == "no_missing_media")
            .unwrap();
        assert!(!gate.passed);
        assert!(gate.details["missing"][0]["reason"] == "missing_reference");
    }

    #[tokio::test]
    async fn verify_render_reports_synthetic_render_gates() {
        if awidat_render::ffmpeg_path().is_err() || awidat_render::ffprobe_path().is_err() {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let raw_dir = dir.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        let source_path = raw_dir.join("source.mp4");
        synthesize_fixture(&source_path, 1.2, false).unwrap();

        let mut timeline = awidat_proto::otio::Timeline::empty("verify");
        let mut track = Track::empty("v1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Clip(clip("source", "raw/source.mp4", 0.0, 1.2)));
        timeline.tracks.children.push(StackChild::Track(track));
        project.timeline = timeline;
        project.write(dir.path()).unwrap();

        let output_path = dir.path().join("renders").join("out.mp4");
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        synthesize_fixture(&output_path, 1.2, false).unwrap();

        let report = verify_render_output(dir.path(), &output_path, VerifyRenderOptions::default())
            .await
            .unwrap();

        assert!(report.passed, "{report:#?}");
        assert!(
            report
                .gates
                .iter()
                .any(|gate| gate.name == "has_video_stream" && gate.passed)
        );
        assert!(
            report
                .gates
                .iter()
                .any(|gate| gate.name == "has_audio_stream" && gate.passed)
        );
        assert!(
            report
                .gates
                .iter()
                .any(|gate| gate.name == "edited_boundary_probes" && gate.passed)
        );
    }

    fn synthesize_fixture(
        path: &std::path::Path,
        duration_s: f64,
        black: bool,
    ) -> std::io::Result<()> {
        let video = if black {
            format!("color=c=black:size=160x90:rate=24:duration={duration_s}")
        } else {
            format!("testsrc=size=160x90:rate=24:duration={duration_s}")
        };
        let status = Command::new(awidat_render::ffmpeg_path().unwrap())
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(video)
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!(
                "sine=frequency=440:sample_rate=44100:duration={duration_s}"
            ))
            .arg("-shortest")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-c:a")
            .arg("aac")
            .arg(path)
            .status()?;
        assert!(status.success());
        Ok(())
    }
}
