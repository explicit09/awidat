//! `verify_render` agent tool.

use std::collections::BTreeMap;
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

#[derive(Clone)]
struct VerifyRenderOptions {
    expected_duration_s: Option<f64>,
    duration_tolerance_s: f64,
    mode: VerifyMode,
    source_range_manifest_path: Option<PathBuf>,
    silence_threshold_db: f64,
    max_unexpected_silence_s: f64,
    black_min_duration_s: f64,
    max_black_segment_s: f64,
    /// Optional test hook that swaps in a deterministic frame sampler for the
    /// caption frame-pixel scorer. Production callers leave this `None` so the
    /// scorer falls back to its ffmpeg-backed sampler.
    caption_frame_sampler_override:
        Option<std::sync::Arc<dyn crate::caption_rendered_output_scorer::CaptionFrameSampler>>,
}

impl std::fmt::Debug for VerifyRenderOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifyRenderOptions")
            .field("expected_duration_s", &self.expected_duration_s)
            .field("duration_tolerance_s", &self.duration_tolerance_s)
            .field("mode", &self.mode)
            .field(
                "source_range_manifest_path",
                &self.source_range_manifest_path,
            )
            .field("silence_threshold_db", &self.silence_threshold_db)
            .field("max_unexpected_silence_s", &self.max_unexpected_silence_s)
            .field("black_min_duration_s", &self.black_min_duration_s)
            .field("max_black_segment_s", &self.max_black_segment_s)
            .field(
                "caption_frame_sampler_override",
                &self
                    .caption_frame_sampler_override
                    .as_ref()
                    .map(|_| "<sampler>"),
            )
            .finish()
    }
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
            caption_frame_sampler_override: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    render_manifest: Option<RenderManifestEvidence>,
    caption_summary: crate::captions::CaptionSummary,
    gates: Vec<VerificationGate>,
    timeline_manifest: TimelineManifest,
}

#[derive(Debug, Clone, Serialize)]
struct RenderManifestEvidence {
    manifest_path: String,
    manifest_id: String,
    backend: String,
    replay_kind: String,
    input_count: usize,
    output_count: usize,
    sidecar_count: usize,
    limitation_count: usize,
    metadata: BTreeMap<String, String>,
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
    cut_boundaries: Vec<CutBoundary>,
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

#[derive(Debug, Clone, Serialize)]
struct CutBoundary {
    from_clip_name: String,
    to_clip_name: String,
    from_asset: String,
    to_asset: String,
    at_s: f64,
    gap_s: f64,
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
        true
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
            caption_frame_sampler_override: None,
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
    let mut render_manifest = collect_render_manifest_evidence(output_path, &mut gates)?;
    let caption_summary = crate::captions::summarize_captions(&project);
    maybe_run_caption_scorer(
        output_path,
        &probe,
        render_manifest.as_mut(),
        &caption_summary,
        options.caption_frame_sampler_override.as_deref(),
    )
    .await;
    add_caption_evidence_gate(&mut gates, &caption_summary);
    add_caption_safe_area_gate(&mut gates, &caption_summary);
    add_manifest_caption_evidence_gate(&mut gates, render_manifest.as_ref(), &caption_summary);
    add_caption_rendered_output_gate(&mut gates, render_manifest.as_ref(), &caption_summary);

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
    add_cut_boundary_self_eval_gate(
        &mut gates,
        probe.duration_s,
        &timeline_manifest.cut_boundaries,
        &unexpected_silences,
        &long_black_ranges,
    );

    let report = VerifyRenderReport {
        passed: gates.iter().all(|gate| gate.passed),
        mode: options.mode,
        output_path: output_path.display().to_string(),
        expected_duration_s,
        actual_duration_s: probe.duration_s,
        render_manifest,
        caption_summary,
        gates,
        timeline_manifest,
    };
    write_verification_evidence(output_path, &report)?;
    Ok(report)
}

fn add_caption_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    summary: &crate::captions::CaptionSummary,
) {
    let consistent = summary.has_exportable_captions
        == !matches!(
            summary.selected_authority,
            crate::captions::CaptionAuthority::None
        );
    push_gate(
        gates,
        "caption_evidence_consistent",
        consistent,
        "caption/subtitle authority and counts are internally consistent",
        json!({
            "selected_authority": summary.selected_authority.as_str(),
            "editable_track_count": summary.editable_track_count,
            "editable_cue_count": summary.editable_cue_count,
            "caption_overlay_count": summary.caption_overlay_count,
            "word_timed_caption_overlay_count": summary.word_timed_caption_overlay_count,
            "safe_area_caption_overlay_count": summary.safe_area_caption_overlay_count,
            "mobile_safe_area_caption_overlay_count": summary.mobile_safe_area_caption_overlay_count,
            "missing_safe_area_caption_overlay_count": summary.missing_safe_area_caption_overlay_count,
            "sidecar_cue_count": summary.sidecar_cue_count,
            "has_exportable_captions": summary.has_exportable_captions,
            "warnings": summary.warnings,
        }),
    );
}

fn add_caption_safe_area_gate(
    gates: &mut Vec<VerificationGate>,
    summary: &crate::captions::CaptionSummary,
) {
    let passed =
        summary.caption_overlay_count == 0 || summary.missing_safe_area_caption_overlay_count == 0;
    push_gate(
        gates,
        "caption_safe_area_metadata_present",
        passed,
        "caption overlays carry safe-area metadata for layout preflight",
        json!({
            "caption_overlay_count": summary.caption_overlay_count,
            "safe_area_caption_overlay_count": summary.safe_area_caption_overlay_count,
            "mobile_safe_area_caption_overlay_count": summary.mobile_safe_area_caption_overlay_count,
            "missing_safe_area_caption_overlay_count": summary.missing_safe_area_caption_overlay_count,
        }),
    );
}

fn add_manifest_caption_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    manifest: Option<&RenderManifestEvidence>,
    summary: &crate::captions::CaptionSummary,
) {
    let Some(manifest) = manifest else {
        return;
    };
    let has_manifest_caption_metadata = manifest.metadata.contains_key("caption_authority");
    let caption_metadata_required = summary.has_exportable_captions
        && requires_caption_manifest_metadata(manifest.backend.as_str());
    if !has_manifest_caption_metadata && !caption_metadata_required {
        return;
    }
    let expected = crate::captions::caption_summary_metadata(summary);
    let mismatches = expected
        .iter()
        .filter_map(|(key, expected_value)| {
            let actual = manifest.metadata.get(key).map(String::as_str);
            (actual != Some(expected_value.as_str())).then(|| {
                json!({
                    "field": key,
                    "manifest": actual,
                    "timeline": expected_value,
                })
            })
        })
        .collect::<Vec<_>>();
    push_gate(
        gates,
        "render_manifest_caption_evidence_matches_timeline",
        mismatches.is_empty(),
        "render manifest caption metadata matches the current timeline caption summary",
        json!({
            "manifest_path": manifest.manifest_path,
            "metadata": manifest.metadata,
            "timeline": {
                "caption_authority": summary.selected_authority.as_str(),
                "caption_overlay_count": summary.caption_overlay_count,
                "word_timed_caption_overlay_count": summary.word_timed_caption_overlay_count,
                "safe_area_caption_overlay_count": summary.safe_area_caption_overlay_count,
                "mobile_safe_area_caption_overlay_count": summary.mobile_safe_area_caption_overlay_count,
                "missing_safe_area_caption_overlay_count": summary.missing_safe_area_caption_overlay_count,
                "subtitle_sidecar_cue_count": summary.sidecar_cue_count,
            },
            "mismatches": mismatches,
        }),
    );
}

fn requires_caption_manifest_metadata(backend: &str) -> bool {
    matches!(
        backend,
        "timeline_ffmpeg_reencode" | "timeline_raw_stream_gpu" | "package_export"
    )
}

async fn maybe_run_caption_scorer(
    output_path: &Path,
    probe: &awidat_render::MediaProbe,
    manifest: Option<&mut RenderManifestEvidence>,
    caption_summary: &crate::captions::CaptionSummary,
    sampler_override: Option<&dyn crate::caption_rendered_output_scorer::CaptionFrameSampler>,
) {
    if !caption_summary.has_exportable_captions {
        return;
    }
    let Some(manifest) = manifest else {
        return;
    };
    let Some(sidecar_csv) = manifest
        .metadata
        .get("libass_layout_sidecar_paths")
        .cloned()
    else {
        return;
    };
    let sidecars: Vec<PathBuf> = sidecar_csv
        .split(',')
        .filter(|segment| !segment.is_empty())
        .map(PathBuf::from)
        .collect();
    if sidecars.is_empty() {
        return;
    }

    let safe_area_profile = if caption_summary.mobile_safe_area_caption_overlay_count > 0 {
        "mobile"
    } else {
        "default"
    };
    let video_dims = (
        probe.video_width.unwrap_or(1920),
        probe.video_height.unwrap_or(1080),
    );

    let owned_sampler;
    let sampler: &dyn crate::caption_rendered_output_scorer::CaptionFrameSampler =
        if let Some(s) = sampler_override {
            s
        } else {
            owned_sampler = crate::caption_rendered_output_scorer::FfmpegFrameSampler::new(
                output_path.to_path_buf(),
            );
            &owned_sampler
        };

    match crate::caption_rendered_output_scorer::score_caption_rendered_output(
        output_path,
        &sidecars,
        video_dims,
        safe_area_profile,
        sampler,
    )
    .await
    {
        Ok(evidence) => {
            manifest.metadata.insert(
                "caption_rendered_output_source".into(),
                "frame_pixel_scorer".into(),
            );
            let status = if evidence.probe_count == 0 {
                "skipped"
            } else if evidence.safe_area_pass_count == evidence.probe_count
                && evidence.occlusion_fail_count == 0
            {
                "passed"
            } else {
                "failed"
            };
            manifest
                .metadata
                .insert("caption_rendered_output_status".into(), status.into());
            manifest.metadata.insert(
                "caption_rendered_output_probe_count".into(),
                evidence.probe_count.to_string(),
            );
            manifest.metadata.insert(
                "caption_rendered_output_safe_area_pass_count".into(),
                evidence.safe_area_pass_count.to_string(),
            );
            manifest.metadata.insert(
                "caption_rendered_output_occlusion_fail_count".into(),
                evidence.occlusion_fail_count.to_string(),
            );
            if let Some(reason) = evidence.fallback_reason {
                manifest.metadata.insert(
                    "caption_rendered_output_fallback_reason".into(),
                    reason.into(),
                );
            }
        }
        Err(err) => {
            let reason = match err {
                crate::caption_rendered_output_scorer::ScorerError::SamplerUnavailable(_) => {
                    "ffmpeg_unavailable"
                }
                crate::caption_rendered_output_scorer::ScorerError::RenderOutputMissing => {
                    "render_output_missing"
                }
                crate::caption_rendered_output_scorer::ScorerError::SidecarParseFailed => {
                    "sidecar_parse_failed"
                }
                crate::caption_rendered_output_scorer::ScorerError::Io(_) => "io_error",
            };
            manifest.metadata.insert(
                "caption_rendered_output_fallback_reason".into(),
                reason.into(),
            );
        }
    }
}

fn add_caption_rendered_output_gate(
    gates: &mut Vec<VerificationGate>,
    manifest: Option<&RenderManifestEvidence>,
    summary: &crate::captions::CaptionSummary,
) {
    if !summary.has_exportable_captions {
        return;
    }
    let expected_probe_count = expected_caption_render_probe_count(summary);
    let Some(manifest) = manifest else {
        push_gate(
            gates,
            "caption_rendered_output_readable",
            false,
            "rendered caption output has artifact-level safe-area and occlusion evidence",
            json!({
                "reason": "missing_render_manifest",
                "expected_probe_count": expected_probe_count,
            }),
        );
        return;
    };
    let status = manifest
        .metadata
        .get("caption_rendered_output_status")
        .map(String::as_str);
    let mut probe_count =
        parse_manifest_usize(&manifest.metadata, "caption_rendered_output_probe_count");
    let mut safe_area_pass_count = parse_manifest_usize(
        &manifest.metadata,
        "caption_rendered_output_safe_area_pass_count",
    );
    let mut occlusion_fail_count = parse_manifest_usize(
        &manifest.metadata,
        "caption_rendered_output_occlusion_fail_count",
    );
    let source = manifest
        .metadata
        .get("caption_rendered_output_source")
        .map(String::as_str);
    let fallback_reason = manifest
        .metadata
        .get("caption_rendered_output_fallback_reason")
        .map(String::as_str);
    let has_evidence = status.is_some()
        || probe_count.is_some()
        || safe_area_pass_count.is_some()
        || occlusion_fail_count.is_some();
    let mut reason = "missing_caption_rendered_output_evidence";
    let mut passed = status == Some("passed")
        && probe_count.is_some_and(|count| count >= expected_probe_count)
        && safe_area_pass_count.is_some_and(|count| count >= expected_probe_count)
        && occlusion_fail_count == Some(0);
    if has_evidence {
        reason = if source == Some("frame_pixel_scorer") {
            if passed {
                "frame_pixel_scorer_passed"
            } else {
                "frame_pixel_scorer_failed"
            }
        } else if passed {
            "passed"
        } else {
            "caption_rendered_output_evidence_failed"
        };
    } else if libass_layout_supports_caption_rendered_output(
        &manifest.metadata,
        expected_probe_count,
    ) {
        passed = true;
        reason = if fallback_reason.is_some() {
            "frame_pixel_scorer_unavailable_fell_back_to_libass_layout"
        } else {
            "derived_from_libass_layout_evidence"
        };
        probe_count = Some(expected_probe_count);
        safe_area_pass_count = Some(expected_probe_count);
        occlusion_fail_count = Some(0);
    }
    let status_detail = if status.is_some() {
        status
    } else if passed && reason == "derived_from_libass_layout_evidence" {
        Some("derived")
    } else {
        None
    };
    push_gate(
        gates,
        "caption_rendered_output_readable",
        passed,
        "rendered caption output has artifact-level safe-area and occlusion evidence",
        json!({
            "reason": reason,
            "manifest_path": manifest.manifest_path,
            "caption_authority": summary.selected_authority.as_str(),
            "expected_probe_count": expected_probe_count,
            "caption_rendered_output_status": status_detail,
            "caption_rendered_output_probe_count": probe_count,
            "caption_rendered_output_safe_area_pass_count": safe_area_pass_count,
            "caption_rendered_output_occlusion_fail_count": occlusion_fail_count,
        }),
    );
}

fn libass_layout_supports_caption_rendered_output(
    metadata: &BTreeMap<String, String>,
    expected_probe_count: usize,
) -> bool {
    let reason = metadata.get("timeline_backend_reason").map(String::as_str);
    let claimed_count = parse_manifest_usize(metadata, "libass_caption_count");
    let layout_sidecar_count = parse_manifest_usize(metadata, "libass_layout_sidecar_count");
    let safe_area_sidecar_count =
        parse_manifest_usize(metadata, "libass_layout_safe_area_sidecar_count");
    let wrapped_sidecar_count =
        parse_manifest_usize(metadata, "libass_layout_wrapped_sidecar_count");
    let karaoke_sidecar_count =
        parse_manifest_usize(metadata, "libass_layout_karaoke_sidecar_count");
    let playres_present = metadata
        .get("libass_layout_playres")
        .is_some_and(|value| !value.trim().is_empty());
    reason == Some("ffmpeg_with_libass_captions")
        && claimed_count.is_some_and(|count| count >= expected_probe_count)
        && layout_sidecar_count.is_some_and(|count| count >= expected_probe_count)
        && safe_area_sidecar_count.is_some_and(|count| count >= expected_probe_count)
        && wrapped_sidecar_count.is_some()
        && karaoke_sidecar_count.is_some()
        && playres_present
}

fn expected_caption_render_probe_count(summary: &crate::captions::CaptionSummary) -> usize {
    summary
        .caption_overlay_count
        .max(summary.editable_cue_count)
        .max(summary.sidecar_cue_count)
        .max(1)
}

fn parse_manifest_usize(metadata: &BTreeMap<String, String>, key: &str) -> Option<usize> {
    metadata.get(key).and_then(|value| value.parse().ok())
}

fn collect_render_manifest_evidence(
    output_path: &Path,
    gates: &mut Vec<VerificationGate>,
) -> Result<Option<RenderManifestEvidence>, FunctionCallError> {
    let manifest_path = awidat_render::manifest_path_for_output(output_path);
    if !manifest_path.is_file() {
        push_gate(
            gates,
            "render_manifest_present",
            false,
            "render manifest is present next to the rendered output",
            json!({"manifest_path": manifest_path}),
        );
        return Ok(None);
    }
    let manifest = awidat_render::read_render_manifest(&manifest_path).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: read render manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    let replay_kind = match &manifest.replay {
        awidat_render::RenderReplayPlan::FfmpegArgv { .. } => "ffmpeg_argv",
        awidat_render::RenderReplayPlan::Unsupported { .. } => "unsupported",
    };
    let backend = serde_json::to_value(&manifest.backend)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{:?}", manifest.backend));
    push_gate(
        gates,
        "render_manifest_present",
        true,
        "render manifest is present next to the rendered output",
        json!({
            "manifest_path": manifest_path,
            "manifest_id": manifest.manifest_id,
            "backend": backend,
            "replay_kind": replay_kind,
            "input_count": manifest.inputs.len(),
            "output_count": manifest.outputs.len(),
            "sidecar_count": manifest.sidecars.len(),
            "limitation_count": manifest.limitations.len(),
        }),
    );
    add_render_manifest_required_artifacts_gate(gates, &manifest, &manifest_path);
    add_render_feature_evidence_gate(gates, &manifest.backend, &manifest.metadata);
    add_render_backend_evidence_gate(gates, &backend, &manifest.metadata);
    add_libass_sidecar_evidence_gate(gates, &manifest.metadata, &manifest.sidecars);
    Ok(Some(RenderManifestEvidence {
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        manifest_id: manifest.manifest_id,
        backend,
        replay_kind: replay_kind.into(),
        input_count: manifest.inputs.len(),
        output_count: manifest.outputs.len(),
        sidecar_count: manifest.sidecars.len(),
        limitation_count: manifest.limitations.len(),
        metadata: manifest.metadata,
    }))
}

fn add_render_manifest_required_artifacts_gate(
    gates: &mut Vec<VerificationGate>,
    manifest: &awidat_render::RenderExecutionManifest,
    manifest_path: &Path,
) {
    match awidat_render::validate_replay_manifest(manifest, manifest_path) {
        Ok(()) => push_gate(
            gates,
            "render_manifest_required_artifacts_valid",
            true,
            "render manifest required inputs and sidecars exist with matching fingerprints",
            json!({
                "manifest_path": manifest_path,
                "required_input_count": manifest.inputs.iter().filter(|input| input.required).count(),
                "required_sidecar_count": manifest.sidecars.iter().filter(|sidecar| sidecar.required).count(),
            }),
        ),
        Err(error) => {
            let mut details = json!({
                "manifest_path": manifest_path,
                "error": error.to_string(),
            });
            if let Some(object) = details.as_object_mut() {
                match &error {
                    awidat_render::RenderReplayError::MissingRequiredArtifact {
                        kind,
                        artifact,
                        ..
                    }
                    | awidat_render::RenderReplayError::FingerprintMismatch {
                        kind,
                        artifact,
                        ..
                    } => {
                        object.insert("kind".into(), json!(kind));
                        object.insert("artifact".into(), json!(artifact));
                    }
                    _ => {}
                }
            }
            push_gate(
                gates,
                "render_manifest_required_artifacts_valid",
                false,
                "render manifest required inputs and sidecars exist with matching fingerprints",
                details,
            );
        }
    }
}

fn add_libass_sidecar_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    metadata: &BTreeMap<String, String>,
    sidecars: &[awidat_render::RenderSidecarFingerprint],
) {
    let claimed_count = metadata
        .get("libass_caption_count")
        .and_then(|value| value.parse::<usize>().ok());
    let reason = metadata.get("timeline_backend_reason").map(String::as_str);
    let claims_libass = reason == Some("ffmpeg_with_libass_captions")
        || claimed_count.is_some_and(|count| count > 0);
    if !claims_libass {
        return;
    }

    let required_ass_sidecar_count = sidecars
        .iter()
        .filter(|sidecar| sidecar.required && sidecar.path.to_ascii_lowercase().ends_with(".ass"))
        .count();
    let passed = claimed_count.is_some_and(|count| {
        count > 0
            && required_ass_sidecar_count >= count
            && reason == Some("ffmpeg_with_libass_captions")
    });
    push_gate(
        gates,
        "libass_sidecar_evidence_present",
        passed,
        "libass caption render manifests include required ASS sidecar fingerprints",
        json!({
            "timeline_backend_reason": reason,
            "libass_caption_count": claimed_count,
            "required_ass_sidecar_count": required_ass_sidecar_count,
            "sidecar_count": sidecars.len(),
        }),
    );
    add_libass_layout_evidence_gate(gates, metadata, claimed_count, required_ass_sidecar_count);
}

fn add_libass_layout_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    metadata: &BTreeMap<String, String>,
    claimed_count: Option<usize>,
    required_ass_sidecar_count: usize,
) {
    let layout_sidecar_count = metadata
        .get("libass_layout_sidecar_count")
        .and_then(|value| value.parse::<usize>().ok());
    let wrapped_sidecar_count = metadata
        .get("libass_layout_wrapped_sidecar_count")
        .and_then(|value| value.parse::<usize>().ok());
    let safe_area_sidecar_count = metadata
        .get("libass_layout_safe_area_sidecar_count")
        .and_then(|value| value.parse::<usize>().ok());
    let karaoke_sidecar_count = metadata
        .get("libass_layout_karaoke_sidecar_count")
        .and_then(|value| value.parse::<usize>().ok());
    let playres = metadata.get("libass_layout_playres").map(String::as_str);
    let expected_count = claimed_count.unwrap_or(required_ass_sidecar_count);
    let passed = expected_count > 0
        && layout_sidecar_count.is_some_and(|count| count >= expected_count)
        && wrapped_sidecar_count.is_some()
        && safe_area_sidecar_count.is_some_and(|count| count >= expected_count)
        && karaoke_sidecar_count.is_some()
        && playres.is_some_and(|value| !value.is_empty());
    push_gate(
        gates,
        "libass_layout_evidence_present",
        passed,
        "libass caption manifests include ASS layout/readability evidence",
        json!({
            "expected_caption_sidecar_count": expected_count,
            "required_ass_sidecar_count": required_ass_sidecar_count,
            "libass_layout_sidecar_count": layout_sidecar_count,
            "libass_layout_playres": playres,
            "libass_layout_wrapped_sidecar_count": wrapped_sidecar_count,
            "libass_layout_safe_area_sidecar_count": safe_area_sidecar_count,
            "libass_layout_karaoke_sidecar_count": karaoke_sidecar_count,
        }),
    );
}

fn add_render_feature_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    backend: &awidat_render::RenderBackendKind,
    metadata: &BTreeMap<String, String>,
) {
    let expected = crate::capabilities::render_feature_metadata_for_backend(backend);
    let expected_feature_id = expected.get("render_feature_id").map(String::as_str);
    let expected_export_supported = expected
        .get("render_feature_export_supported")
        .map(String::as_str);
    let expected_preview_supported = expected
        .get("render_feature_preview_supported")
        .map(String::as_str);
    let expected_approval_required = expected
        .get("render_feature_approval_required")
        .map(String::as_str);
    let expected_limitation_count = expected
        .get("render_feature_limitation_count")
        .map(String::as_str);
    let feature_id = metadata.get("render_feature_id").map(String::as_str);
    let preview_supported = metadata
        .get("render_feature_preview_supported")
        .map(String::as_str);
    let export_supported = metadata
        .get("render_feature_export_supported")
        .map(String::as_str);
    let approval_required = metadata
        .get("render_feature_approval_required")
        .map(String::as_str);
    let limitation_count = metadata
        .get("render_feature_limitation_count")
        .map(String::as_str);
    let passed = feature_id == expected_feature_id
        && preview_supported == expected_preview_supported
        && export_supported == expected_export_supported
        && approval_required == expected_approval_required
        && limitation_count == expected_limitation_count;
    push_gate(
        gates,
        "render_feature_evidence_present",
        passed,
        "render manifest includes capability metadata for the selected backend",
        json!({
            "expected_feature_id": expected_feature_id,
            "render_feature_id": feature_id,
            "expected_preview_supported": expected_preview_supported,
            "render_feature_preview_supported": preview_supported,
            "expected_export_supported": expected_export_supported,
            "render_feature_export_supported": export_supported,
            "expected_approval_required": expected_approval_required,
            "render_feature_approval_required": approval_required,
            "expected_limitation_count": expected_limitation_count,
            "render_feature_limitation_count": limitation_count,
        }),
    );
}

fn add_render_backend_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    backend: &str,
    metadata: &BTreeMap<String, String>,
) {
    if backend == "stream_export_remux" {
        add_stream_remux_evidence_gate(gates, metadata);
        return;
    }
    if !matches!(
        backend,
        "timeline_ffmpeg_reencode" | "timeline_raw_stream_gpu"
    ) {
        return;
    }
    let manifest_backend = metadata.get("timeline_backend").map(String::as_str);
    let reason = metadata.get("timeline_backend_reason").map(String::as_str);
    let passed = manifest_backend == Some(backend) && reason.is_some_and(|value| !value.is_empty());
    push_gate(
        gates,
        "render_backend_evidence_present",
        passed,
        "timeline render manifest includes backend-selection evidence",
        json!({
            "backend": backend,
            "timeline_backend": manifest_backend,
            "timeline_backend_reason": reason,
            "gpu_transition_count": metadata.get("gpu_transition_count"),
            "ffmpeg_transition_count": metadata.get("ffmpeg_transition_count"),
            "libass_caption_count": metadata.get("libass_caption_count"),
        }),
    );
    add_master_loudnorm_evidence_gate(gates, metadata);
}

fn add_master_loudnorm_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    metadata: &BTreeMap<String, String>,
) {
    let enabled = metadata.get("master_loudnorm_enabled").map(String::as_str) == Some("true");
    if !enabled {
        return;
    }
    let pass = metadata.get("master_loudnorm_pass").map(String::as_str);
    let output_mode = metadata
        .get("master_loudnorm_output_mode")
        .map(String::as_str);
    let passed = pass == Some("apply") && output_mode == Some("encoded_output");
    push_gate(
        gates,
        "master_loudnorm_final_pass",
        passed,
        "two-pass master loudnorm render manifest records the final encoded apply pass",
        json!({
            "master_loudnorm_enabled": true,
            "master_loudnorm_pass": pass,
            "master_loudnorm_output_mode": output_mode,
        }),
    );
}

fn add_stream_remux_evidence_gate(
    gates: &mut Vec<VerificationGate>,
    metadata: &BTreeMap<String, String>,
) {
    let remux_backend = metadata.get("remux_backend").map(String::as_str);
    let eligibility_reason = metadata.get("remux_eligibility_reason").map(String::as_str);
    let copy_stream_count = metadata.get("copy_stream_count").map(String::as_str);
    let transcode_stream_count = metadata.get("transcode_stream_count").map(String::as_str);
    let all_streams_copy = metadata.get("all_streams_copy").map(String::as_str);
    let passed = remux_backend == Some("stream_export_remux")
        && eligibility_reason.is_some_and(|value| !value.is_empty())
        && copy_stream_count.is_some()
        && transcode_stream_count.is_some()
        && all_streams_copy.is_some();
    push_gate(
        gates,
        "stream_remux_evidence_present",
        passed,
        "stream-remux manifest includes packet-copy eligibility evidence",
        json!({
            "remux_backend": remux_backend,
            "remux_eligibility_reason": eligibility_reason,
            "copy_stream_count": copy_stream_count,
            "transcode_stream_count": transcode_stream_count,
            "all_streams_copy": all_streams_copy,
        }),
    );
}

fn verification_report_path_for_output(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("render");
    output_path.with_file_name(format!("{stem}.verify-render.json"))
}

fn write_verification_evidence(
    output_path: &Path,
    report: &VerifyRenderReport,
) -> Result<PathBuf, FunctionCallError> {
    let report_path = verification_report_path_for_output(output_path);
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| {
        FunctionCallError::RespondToModel(format!("verify_render: encode report artifact: {e}"))
    })?;
    std::fs::write(&report_path, bytes).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: write report {}: {e}",
            report_path.display()
        ))
    })?;
    attach_verification_summary(output_path, &report_path, report)?;
    Ok(report_path)
}

fn attach_verification_summary(
    output_path: &Path,
    report_path: &Path,
    report: &VerifyRenderReport,
) -> Result<(), FunctionCallError> {
    let manifest_path = awidat_render::manifest_path_for_output(output_path);
    if !manifest_path.is_file() {
        return Ok(());
    }
    let mut manifest = awidat_render::read_render_manifest(&manifest_path).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: read render manifest {}: {e}",
            manifest_path.display()
        ))
    })?;
    manifest.verification = Some(awidat_render::RenderVerificationSummary {
        status: if report.passed { "passed" } else { "failed" }.into(),
        report_path: report_path.to_string_lossy().into_owned(),
    });
    awidat_render::write_render_manifest(&manifest_path, &manifest).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "verify_render: update render manifest {}: {e}",
            manifest_path.display()
        ))
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
        cut_boundaries: collect_cut_boundaries(&source_ranges),
        source_ranges,
        missing_media,
        boundary_probes,
    }
}

fn collect_cut_boundaries(ranges: &[SourceRangeEntry]) -> Vec<CutBoundary> {
    let mut ordered = ranges.to_vec();
    ordered.sort_by(|left, right| left.timeline_start_s.total_cmp(&right.timeline_start_s));
    ordered
        .windows(2)
        .filter_map(|pair| {
            let [left, right] = pair else {
                return None;
            };
            let gap_s = right.timeline_start_s - left.timeline_end_s;
            (gap_s >= -DEFAULT_SOURCE_RANGE_TOLERANCE_S).then(|| CutBoundary {
                from_clip_name: left.clip_name.clone(),
                to_clip_name: right.clip_name.clone(),
                from_asset: left.asset.clone(),
                to_asset: right.asset.clone(),
                at_s: right.timeline_start_s,
                gap_s: gap_s.max(0.0),
            })
        })
        .collect()
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
    if is_generated_overlay_clip(clip) {
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

fn is_generated_overlay_clip(clip: &awidat_proto::otio::Clip) -> bool {
    clip.effects.iter().any(|effect| {
        effect.effect_name == "awidat.title"
            || effect.effect_name == "awidat.annotation"
            || matches!(
                effect
                    .metadata
                    .get("role")
                    .and_then(serde_json::Value::as_str),
                Some("title" | "caption" | "captions" | "subtitle" | "subtitles")
            )
    })
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

fn add_cut_boundary_self_eval_gate(
    gates: &mut Vec<VerificationGate>,
    duration_s: Option<f64>,
    boundaries: &[CutBoundary],
    unexpected_silences: &[awidat_render::SilenceRange],
    long_black_ranges: &[awidat_render::BlackFrameRange],
) {
    let checks = boundaries
        .iter()
        .map(|boundary| {
            let in_duration = duration_s
                .map(|duration| boundary.at_s <= duration + DEFAULT_DURATION_TOLERANCE_S)
                .unwrap_or(false);
            let in_silence = unexpected_silences
                .iter()
                .any(|range| contains_time(range.start_s, range.end_s, boundary.at_s));
            let in_black = long_black_ranges
                .iter()
                .any(|range| contains_time(range.start_s, range.end_s, boundary.at_s));
            json!({
                "from_clip_name": boundary.from_clip_name,
                "to_clip_name": boundary.to_clip_name,
                "from_asset": boundary.from_asset,
                "to_asset": boundary.to_asset,
                "at_s": boundary.at_s,
                "gap_s": boundary.gap_s,
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
        "cut_boundary_self_eval",
        passed,
        "actual clip cut boundaries land inside the render and avoid flagged black/silence ranges",
        json!({ "boundaries": checks }),
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
edited-boundary probes. Writes a verify-render report beside the output and \
updates the adjacent render manifest when one exists. It does not start, poll, \
or change render jobs; call start_render/poll_render separately, then pass the \
finished output_path here.";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, Effect, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
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
        assert_eq!(manifest.cut_boundaries.len(), 1);
        assert_eq!(manifest.source_ranges[0].clip_name, "intro");
        assert!((manifest.source_ranges[1].timeline_start_s - 2.5).abs() < 1e-9);
        assert!((manifest.source_ranges[1].source_end_s - 6.5).abs() < 1e-9);
        assert_eq!(manifest.cut_boundaries[0].from_clip_name, "intro");
        assert_eq!(manifest.cut_boundaries[0].to_clip_name, "close");
        assert!((manifest.cut_boundaries[0].at_s - 2.5).abs() < 1e-9);
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
            cut_boundaries: Vec::new(),
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

    #[test]
    fn timeline_backend_evidence_gate_fails_without_reason() {
        let mut gates = Vec::new();

        add_render_backend_evidence_gate(
            &mut gates,
            "timeline_ffmpeg_reencode",
            &std::collections::BTreeMap::from([(
                "timeline_backend".to_string(),
                "timeline_ffmpeg_reencode".to_string(),
            )]),
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "render_backend_evidence_present")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["timeline_backend"], "timeline_ffmpeg_reencode");
        assert!(gate.details["timeline_backend_reason"].is_null());
    }

    #[test]
    fn render_feature_evidence_gate_fails_backend_mismatch() {
        let mut gates = Vec::new();

        add_render_feature_evidence_gate(
            &mut gates,
            &awidat_render::RenderBackendKind::TimelineFfmpegReencode,
            &std::collections::BTreeMap::from([
                (
                    "render_feature_id".to_string(),
                    "stream_copy_remux".to_string(),
                ),
                (
                    "render_feature_export_supported".to_string(),
                    "supported".to_string(),
                ),
            ]),
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "render_feature_evidence_present")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(
            gate.details["expected_feature_id"],
            "ffmpeg_timeline_export"
        );
        assert_eq!(gate.details["render_feature_id"], "stream_copy_remux");
    }

    #[test]
    fn render_feature_evidence_gate_fails_stale_capability_metadata() {
        let mut gates = Vec::new();

        add_render_feature_evidence_gate(
            &mut gates,
            &awidat_render::RenderBackendKind::TimelineFfmpegReencode,
            &std::collections::BTreeMap::from([
                (
                    "render_feature_id".to_string(),
                    "ffmpeg_timeline_export".to_string(),
                ),
                (
                    "render_feature_export_supported".to_string(),
                    "supported".to_string(),
                ),
                (
                    "render_feature_approval_required".to_string(),
                    "false".to_string(),
                ),
                (
                    "render_feature_limitation_count".to_string(),
                    "99".to_string(),
                ),
            ]),
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "render_feature_evidence_present")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["expected_approval_required"], "true");
        assert_eq!(gate.details["render_feature_approval_required"], "false");
        assert_eq!(gate.details["expected_limitation_count"], "0");
        assert_eq!(gate.details["render_feature_limitation_count"], "99");
    }

    #[test]
    fn stream_remux_evidence_gate_requires_copy_counts() {
        let mut gates = Vec::new();

        add_render_backend_evidence_gate(
            &mut gates,
            "stream_export_remux",
            &std::collections::BTreeMap::from([
                (
                    "remux_backend".to_string(),
                    "stream_export_remux".to_string(),
                ),
                (
                    "remux_eligibility_reason".to_string(),
                    "explicit_stream_copy_contract".to_string(),
                ),
            ]),
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "stream_remux_evidence_present")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["remux_backend"], "stream_export_remux");
        assert!(gate.details["copy_stream_count"].is_null());
    }

    #[test]
    fn loudnorm_final_pass_gate_rejects_measure_pass_manifest() {
        let mut gates = Vec::new();

        add_render_backend_evidence_gate(
            &mut gates,
            "timeline_ffmpeg_reencode",
            &std::collections::BTreeMap::from([
                (
                    "timeline_backend".to_string(),
                    "timeline_ffmpeg_reencode".to_string(),
                ),
                (
                    "timeline_backend_reason".to_string(),
                    "ffmpeg_filtergraph".to_string(),
                ),
                ("master_loudnorm_enabled".to_string(), "true".to_string()),
                ("master_loudnorm_pass".to_string(), "measure".to_string()),
                (
                    "master_loudnorm_output_mode".to_string(),
                    "null_muxer".to_string(),
                ),
            ]),
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "master_loudnorm_final_pass")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["master_loudnorm_pass"], "measure");
        assert_eq!(gate.details["master_loudnorm_output_mode"], "null_muxer");
    }

    #[test]
    fn loudnorm_final_pass_gate_accepts_apply_manifest() {
        let mut gates = Vec::new();

        add_render_backend_evidence_gate(
            &mut gates,
            "timeline_ffmpeg_reencode",
            &std::collections::BTreeMap::from([
                (
                    "timeline_backend".to_string(),
                    "timeline_ffmpeg_reencode".to_string(),
                ),
                (
                    "timeline_backend_reason".to_string(),
                    "ffmpeg_filtergraph".to_string(),
                ),
                ("master_loudnorm_enabled".to_string(), "true".to_string()),
                ("master_loudnorm_pass".to_string(), "apply".to_string()),
                (
                    "master_loudnorm_output_mode".to_string(),
                    "encoded_output".to_string(),
                ),
            ]),
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "master_loudnorm_final_pass")
            .unwrap();
        assert!(gate.passed);
        assert_eq!(gate.details["master_loudnorm_pass"], "apply");
        assert_eq!(
            gate.details["master_loudnorm_output_mode"],
            "encoded_output"
        );
    }

    #[test]
    fn cut_boundary_self_eval_gate_fails_problem_boundary() {
        let mut gates = Vec::new();
        let boundaries = vec![CutBoundary {
            from_clip_name: "intro".into(),
            to_clip_name: "close".into(),
            from_asset: "raw/a.mp4".into(),
            to_asset: "raw/b.mp4".into(),
            at_s: 2.5,
            gap_s: 0.0,
        }];
        let silences = vec![awidat_render::SilenceRange {
            start_s: 2.45,
            end_s: 2.55,
            db_floor: -80.0,
        }];

        add_cut_boundary_self_eval_gate(&mut gates, Some(4.0), &boundaries, &silences, &[]);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "cut_boundary_self_eval")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["boundaries"][0]["from_clip_name"], "intro");
        assert_eq!(gate.details["boundaries"][0]["in_unexpected_silence"], true);
    }

    #[test]
    fn caption_safe_area_gate_fails_missing_safe_area() {
        let mut gates = Vec::new();
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 1,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 0,
            mobile_safe_area_caption_overlay_count: 0,
            missing_safe_area_caption_overlay_count: 1,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: Vec::new(),
        };

        add_caption_safe_area_gate(&mut gates, &summary);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "caption_safe_area_metadata_present")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["caption_overlay_count"], 1);
        assert_eq!(gate.details["missing_safe_area_caption_overlay_count"], 1);
    }

    #[test]
    fn libass_layout_gate_fails_missing_layout_metadata() {
        let mut gates = Vec::new();
        let sidecars = vec![awidat_render::RenderSidecarFingerprint {
            path: "/tmp/caption.ass".into(),
            sha256: "abc".into(),
            size_bytes: 128,
            required: true,
        }];

        add_libass_sidecar_evidence_gate(
            &mut gates,
            &BTreeMap::from([
                (
                    "timeline_backend_reason".into(),
                    "ffmpeg_with_libass_captions".into(),
                ),
                ("libass_caption_count".into(), "1".into()),
            ]),
            &sidecars,
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "libass_layout_evidence_present")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(
            gate.details["libass_layout_sidecar_count"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn libass_layout_gate_accepts_layout_metadata() {
        let mut gates = Vec::new();
        let sidecars = vec![awidat_render::RenderSidecarFingerprint {
            path: "/tmp/caption.ass".into(),
            sha256: "abc".into(),
            size_bytes: 128,
            required: true,
        }];

        add_libass_sidecar_evidence_gate(
            &mut gates,
            &BTreeMap::from([
                (
                    "timeline_backend_reason".into(),
                    "ffmpeg_with_libass_captions".into(),
                ),
                ("libass_caption_count".into(), "1".into()),
                ("libass_layout_sidecar_count".into(), "1".into()),
                ("libass_layout_playres".into(), "1920x1080".into()),
                ("libass_layout_wrapped_sidecar_count".into(), "1".into()),
                ("libass_layout_safe_area_sidecar_count".into(), "1".into()),
                ("libass_layout_karaoke_sidecar_count".into(), "1".into()),
            ]),
            &sidecars,
        );

        let gate = gates
            .iter()
            .find(|gate| gate.name == "libass_layout_evidence_present")
            .unwrap();
        assert!(gate.passed);
        assert_eq!(gate.details["libass_layout_playres"], "1920x1080");
    }

    #[test]
    fn caption_rendered_output_gate_fails_missing_artifact_evidence() {
        let mut gates = Vec::new();
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 1,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 1,
            mobile_safe_area_caption_overlay_count: 1,
            missing_safe_area_caption_overlay_count: 0,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: Vec::new(),
        };
        let manifest = RenderManifestEvidence {
            manifest_path: "/tmp/out.render-manifest.json".into(),
            manifest_id: "m1".into(),
            backend: "timeline_ffmpeg_reencode".into(),
            replay_kind: "ffmpeg_argv".into(),
            input_count: 1,
            output_count: 1,
            sidecar_count: 1,
            limitation_count: 0,
            metadata: BTreeMap::new(),
        };

        add_caption_rendered_output_gate(&mut gates, Some(&manifest), &summary);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "caption_rendered_output_readable")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(
            gate.details["reason"],
            "missing_caption_rendered_output_evidence"
        );
    }

    #[test]
    fn caption_rendered_output_gate_accepts_artifact_evidence() {
        let mut gates = Vec::new();
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 1,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 1,
            mobile_safe_area_caption_overlay_count: 1,
            missing_safe_area_caption_overlay_count: 0,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: Vec::new(),
        };
        let manifest = RenderManifestEvidence {
            manifest_path: "/tmp/out.render-manifest.json".into(),
            manifest_id: "m1".into(),
            backend: "timeline_ffmpeg_reencode".into(),
            replay_kind: "ffmpeg_argv".into(),
            input_count: 1,
            output_count: 1,
            sidecar_count: 1,
            limitation_count: 0,
            metadata: BTreeMap::from([
                ("caption_rendered_output_status".into(), "passed".into()),
                ("caption_rendered_output_probe_count".into(), "1".into()),
                (
                    "caption_rendered_output_safe_area_pass_count".into(),
                    "1".into(),
                ),
                (
                    "caption_rendered_output_occlusion_fail_count".into(),
                    "0".into(),
                ),
            ]),
        };

        add_caption_rendered_output_gate(&mut gates, Some(&manifest), &summary);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "caption_rendered_output_readable")
            .unwrap();
        assert!(gate.passed);
        assert_eq!(gate.details["caption_rendered_output_probe_count"], 1);
    }

    #[test]
    fn caption_rendered_output_gate_derives_from_libass_layout_evidence() {
        let mut gates = Vec::new();
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 1,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 1,
            mobile_safe_area_caption_overlay_count: 1,
            missing_safe_area_caption_overlay_count: 0,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: Vec::new(),
        };
        let manifest = RenderManifestEvidence {
            manifest_path: "/tmp/out.render-manifest.json".into(),
            manifest_id: "m1".into(),
            backend: "timeline_ffmpeg_reencode".into(),
            replay_kind: "ffmpeg_argv".into(),
            input_count: 1,
            output_count: 1,
            sidecar_count: 1,
            limitation_count: 0,
            metadata: BTreeMap::from([
                (
                    "timeline_backend_reason".into(),
                    "ffmpeg_with_libass_captions".into(),
                ),
                ("libass_caption_count".into(), "1".into()),
                ("libass_layout_sidecar_count".into(), "1".into()),
                ("libass_layout_playres".into(), "1920x1080".into()),
                ("libass_layout_safe_area_sidecar_count".into(), "1".into()),
                ("libass_layout_wrapped_sidecar_count".into(), "1".into()),
                ("libass_layout_karaoke_sidecar_count".into(), "1".into()),
            ]),
        };

        add_caption_rendered_output_gate(&mut gates, Some(&manifest), &summary);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "caption_rendered_output_readable")
            .unwrap();
        assert!(gate.passed);
        assert_eq!(
            gate.details["reason"],
            "derived_from_libass_layout_evidence"
        );
        assert_eq!(gate.details["caption_rendered_output_probe_count"], 1);
        assert_eq!(
            gate.details["caption_rendered_output_safe_area_pass_count"],
            1
        );
        assert_eq!(
            gate.details["caption_rendered_output_occlusion_fail_count"],
            0
        );
    }

    #[test]
    fn render_manifest_caption_gate_fails_metadata_drift() {
        let mut gates = Vec::new();
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 1,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 1,
            mobile_safe_area_caption_overlay_count: 1,
            missing_safe_area_caption_overlay_count: 0,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: Vec::new(),
        };
        let manifest = RenderManifestEvidence {
            manifest_path: "/tmp/out.render-manifest.json".into(),
            manifest_id: "manifest-1".into(),
            backend: "timeline_ffmpeg_reencode".into(),
            replay_kind: "ffmpeg_argv".into(),
            input_count: 1,
            output_count: 1,
            sidecar_count: 0,
            limitation_count: 0,
            metadata: BTreeMap::from([
                ("caption_authority".into(), "caption_overlays".into()),
                ("caption_overlay_count".into(), "2".into()),
                ("safe_area_caption_overlay_count".into(), "1".into()),
                ("missing_safe_area_caption_overlay_count".into(), "0".into()),
            ]),
        };

        add_manifest_caption_evidence_gate(&mut gates, Some(&manifest), &summary);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "render_manifest_caption_evidence_matches_timeline")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["metadata"]["caption_overlay_count"], "2");
        assert_eq!(gate.details["timeline"]["caption_overlay_count"], 1);
    }

    #[test]
    fn render_manifest_caption_gate_fails_missing_caption_metadata() {
        let mut gates = Vec::new();
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 1,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 1,
            mobile_safe_area_caption_overlay_count: 1,
            missing_safe_area_caption_overlay_count: 0,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: Vec::new(),
        };
        let manifest = RenderManifestEvidence {
            manifest_path: "/tmp/out.render-manifest.json".into(),
            manifest_id: "manifest-1".into(),
            backend: "timeline_ffmpeg_reencode".into(),
            replay_kind: "ffmpeg_argv".into(),
            input_count: 1,
            output_count: 1,
            sidecar_count: 0,
            limitation_count: 0,
            metadata: BTreeMap::from([(
                "timeline_backend".into(),
                "timeline_ffmpeg_reencode".into(),
            )]),
        };

        add_manifest_caption_evidence_gate(&mut gates, Some(&manifest), &summary);

        let gate = gates
            .iter()
            .find(|gate| gate.name == "render_manifest_caption_evidence_matches_timeline")
            .unwrap();
        assert!(!gate.passed);
        assert_eq!(gate.details["mismatches"][0]["field"], "caption_authority");
        assert!(gate.details["metadata"]["caption_authority"].is_null());
    }

    #[test]
    fn render_manifest_evidence_gate_fails_missing_required_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let output_path = dir.path().join("renders/out.mp4");
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        std::fs::write(&output_path, b"render").unwrap();
        let missing_sidecar = dir.path().join("renders/.ass/missing.ass");
        let manifest_path = awidat_render::manifest_path_for_output(&output_path);
        let manifest = awidat_render::RenderExecutionManifest::planned(
            awidat_render::RenderExecutionManifestInput {
                created_at: "2026-05-22T10:00:00Z".into(),
                awidat_version: "test".into(),
                project_root: dir.path().to_string_lossy().into_owned(),
                project_hash: None,
                timeline_hash: None,
                backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
                replay: awidat_render::RenderReplayPlan::FfmpegArgv {
                    argv: vec!["ffmpeg".into()],
                    cwd: Some(dir.path().to_string_lossy().into_owned()),
                },
                inputs: Vec::new(),
                outputs: vec![awidat_render::output_artifact(&output_path, true)],
                sidecars: vec![awidat_render::RenderSidecarFingerprint {
                    path: missing_sidecar.to_string_lossy().into_owned(),
                    sha256: "missing".into(),
                    size_bytes: 7,
                    required: true,
                }],
                limitations: Vec::new(),
                verification: None,
                metadata: std::collections::BTreeMap::from([
                    ("timeline_backend".into(), "timeline_ffmpeg_reencode".into()),
                    (
                        "timeline_backend_reason".into(),
                        "ffmpeg_with_libass_captions".into(),
                    ),
                    ("render_feature_id".into(), "ffmpeg_timeline_export".into()),
                    (
                        "render_feature_preview_supported".into(),
                        "not_supported".into(),
                    ),
                    ("render_feature_export_supported".into(), "supported".into()),
                    ("render_feature_approval_required".into(), "true".into()),
                    ("render_feature_limitation_count".into(), "0".into()),
                ]),
            },
        );
        awidat_render::write_render_manifest(&manifest_path, &manifest).unwrap();

        let mut gates = Vec::new();
        let evidence = collect_render_manifest_evidence(&output_path, &mut gates)
            .unwrap()
            .expect("manifest evidence");

        assert_eq!(evidence.sidecar_count, 1);
        let gate = gates
            .iter()
            .find(|gate| gate.name == "render_manifest_required_artifacts_valid")
            .expect("required artifact gate");
        assert!(!gate.passed);
        assert_eq!(gate.details["kind"], "sidecar");
        assert_eq!(
            gate.details["artifact"].as_str(),
            Some(missing_sidecar.to_string_lossy().as_ref())
        );
        let libass_gate = gates
            .iter()
            .find(|gate| gate.name == "libass_sidecar_evidence_present")
            .expect("libass sidecar evidence gate");
        assert!(!libass_gate.passed);
        assert_eq!(
            libass_gate.details["timeline_backend_reason"],
            "ffmpeg_with_libass_captions"
        );
        assert_eq!(libass_gate.details["required_ass_sidecar_count"], 1);
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
        let mut titles = Track::empty("Titles", TrackKind::Video);
        titles
            .children
            .push(TrackChild::Clip(caption_clip("caption", true)));
        timeline.tracks.children.push(StackChild::Track(titles));
        project.timeline = timeline;
        project.write(dir.path()).unwrap();

        let output_path = dir.path().join("renders").join("out.mp4");
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        synthesize_fixture(&output_path, 1.2, false).unwrap();
        let ass_path = output_path
            .parent()
            .unwrap()
            .join(".ass")
            .join("caption.ass");
        std::fs::create_dir_all(ass_path.parent().unwrap()).unwrap();
        std::fs::write(&ass_path, "[Script Info]\nTitle: test\n").unwrap();
        let sidecars = awidat_render::fingerprint_ffmpeg_subtitle_sidecars(&[
            "ffmpeg".into(),
            "-vf".into(),
            format!("subtitles={}", ass_path.to_string_lossy()),
        ])
        .unwrap();
        let render_manifest_path = awidat_render::manifest_path_for_output(&output_path);
        let manifest = awidat_render::RenderExecutionManifest::planned(
            awidat_render::RenderExecutionManifestInput {
                created_at: "2026-05-22T10:00:00Z".into(),
                awidat_version: "test".into(),
                project_root: dir.path().to_string_lossy().into_owned(),
                project_hash: None,
                timeline_hash: None,
                backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
                replay: awidat_render::RenderReplayPlan::FfmpegArgv {
                    argv: vec!["ffmpeg".into()],
                    cwd: Some(dir.path().to_string_lossy().into_owned()),
                },
                inputs: Vec::new(),
                outputs: vec![awidat_render::output_artifact(&output_path, true)],
                sidecars,
                limitations: Vec::new(),
                verification: None,
                metadata: std::collections::BTreeMap::from([
                    ("timeline_backend".into(), "timeline_ffmpeg_reencode".into()),
                    (
                        "timeline_backend_reason".into(),
                        "ffmpeg_with_libass_captions".into(),
                    ),
                    ("libass_caption_count".into(), "1".into()),
                    ("libass_layout_sidecar_count".into(), "1".into()),
                    ("libass_layout_playres".into(), "1920x1080".into()),
                    ("libass_layout_wrapped_sidecar_count".into(), "1".into()),
                    ("libass_layout_safe_area_sidecar_count".into(), "1".into()),
                    ("libass_layout_karaoke_sidecar_count".into(), "1".into()),
                    ("caption_authority".into(), "caption_overlays".into()),
                    ("caption_overlay_count".into(), "1".into()),
                    ("word_timed_caption_overlay_count".into(), "1".into()),
                    ("safe_area_caption_overlay_count".into(), "1".into()),
                    ("mobile_safe_area_caption_overlay_count".into(), "1".into()),
                    ("missing_safe_area_caption_overlay_count".into(), "0".into()),
                    ("subtitle_sidecar_cue_count".into(), "0".into()),
                    ("caption_warning_count".into(), "1".into()),
                    ("caption_rendered_output_status".into(), "passed".into()),
                    ("caption_rendered_output_probe_count".into(), "1".into()),
                    (
                        "caption_rendered_output_safe_area_pass_count".into(),
                        "1".into(),
                    ),
                    (
                        "caption_rendered_output_occlusion_fail_count".into(),
                        "0".into(),
                    ),
                    ("render_feature_id".into(), "ffmpeg_timeline_export".into()),
                    (
                        "render_feature_preview_supported".into(),
                        "not_supported".into(),
                    ),
                    ("render_feature_export_supported".into(), "supported".into()),
                    ("render_feature_approval_required".into(), "true".into()),
                    ("render_feature_limitation_count".into(), "0".into()),
                ]),
            },
        );
        awidat_render::write_render_manifest(&render_manifest_path, &manifest).unwrap();

        let report = verify_render_output(dir.path(), &output_path, VerifyRenderOptions::default())
            .await
            .unwrap();

        assert!(report.passed, "{report:#?}");
        let manifest_evidence = report
            .render_manifest
            .as_ref()
            .expect("verification report should include render manifest evidence");
        assert_eq!(manifest_evidence.backend, "timeline_ffmpeg_reencode");
        assert_eq!(manifest_evidence.replay_kind, "ffmpeg_argv");
        assert_eq!(
            manifest_evidence.metadata["timeline_backend_reason"],
            "ffmpeg_with_libass_captions"
        );
        assert_eq!(
            manifest_evidence.manifest_path,
            render_manifest_path.to_string_lossy()
        );
        assert_eq!(report.caption_summary.caption_overlay_count, 1);
        assert_eq!(report.caption_summary.word_timed_caption_overlay_count, 1);
        assert_eq!(report.caption_summary.safe_area_caption_overlay_count, 1);
        assert_eq!(
            report
                .caption_summary
                .mobile_safe_area_caption_overlay_count,
            1
        );
        assert_eq!(
            report
                .caption_summary
                .missing_safe_area_caption_overlay_count,
            0
        );
        assert!(report.timeline_manifest.missing_media.is_empty());
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
        assert!(report.gates.iter().any(|gate| {
            gate.name == "caption_evidence_consistent"
                && gate.passed
                && gate.details["caption_overlay_count"] == 1
                && gate.details["word_timed_caption_overlay_count"] == 1
                && gate.details["missing_safe_area_caption_overlay_count"] == 0
        }));
        assert!(report.gates.iter().any(|gate| {
            gate.name == "caption_safe_area_metadata_present"
                && gate.passed
                && gate.details["caption_overlay_count"] == 1
                && gate.details["safe_area_caption_overlay_count"] == 1
                && gate.details["mobile_safe_area_caption_overlay_count"] == 1
                && gate.details["missing_safe_area_caption_overlay_count"] == 0
        }));
        assert!(report.gates.iter().any(|gate| {
            gate.name == "render_backend_evidence_present"
                && gate.passed
                && gate.details["timeline_backend_reason"] == "ffmpeg_with_libass_captions"
        }));
        assert!(report.gates.iter().any(|gate| {
            gate.name == "libass_sidecar_evidence_present"
                && gate.passed
                && gate.details["libass_caption_count"] == 1
                && gate.details["required_ass_sidecar_count"] == 1
        }));
        assert!(report.gates.iter().any(|gate| {
            gate.name == "render_feature_evidence_present"
                && gate.passed
                && gate.details["render_feature_id"] == "ffmpeg_timeline_export"
        }));
        assert!(report.gates.iter().any(|gate| {
            gate.name == "render_manifest_caption_evidence_matches_timeline" && gate.passed
        }));
        let verification_report_path = verification_report_path_for_output(&output_path);
        assert!(verification_report_path.is_file());
        let updated_manifest = awidat_render::read_render_manifest(&render_manifest_path).unwrap();
        let summary = updated_manifest.verification.unwrap();
        assert_eq!(summary.status, "passed");
        assert_eq!(
            summary.report_path,
            verification_report_path.to_string_lossy()
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

    fn caption_clip(name: &str, word_timed: bool) -> Clip {
        let mut clip = Clip::empty(name);
        clip.source_range = Some(range(0.0, 1.0));
        let mut effect = Effect::new("awidat.title");
        effect
            .metadata
            .insert("role".into(), serde_json::json!("caption"));
        effect
            .metadata
            .insert("safe_area".into(), serde_json::json!("mobile"));
        if word_timed {
            effect.metadata.insert(
                "word_timings".into(),
                serde_json::json!([
                    {"text": "hello", "start_s": 0.0, "end_s": 0.4},
                    {"text": "world", "start_s": 0.4, "end_s": 1.0}
                ]),
            );
        }
        clip.effects.push(effect);
        clip
    }

    /// Build a synthetic render fixture wired up so the caption frame-pixel
    /// scorer can run during `verify_render_output`. Returns the project root
    /// tempdir, the render output path, and the expected scorer bbox (for
    /// tests that need to plant high-variance pixels there).
    fn scorer_test_fixture() -> (tempfile::TempDir, std::path::PathBuf, (u32, u32, u32, u32)) {
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
        let mut titles = Track::empty("Titles", TrackKind::Video);
        titles
            .children
            .push(TrackChild::Clip(caption_clip("caption", true)));
        timeline.tracks.children.push(StackChild::Track(titles));
        project.timeline = timeline;
        project.write(dir.path()).unwrap();

        let output_path = dir.path().join("renders").join("out.mp4");
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        synthesize_fixture(&output_path, 1.2, false).unwrap();

        let ass_dir = output_path.parent().unwrap().join(".ass");
        std::fs::create_dir_all(&ass_dir).unwrap();
        let ass_path = ass_dir.join("caption.ass");
        let ass_body = "[Script Info]\n\
PlayResX: 1920\n\
PlayResY: 1080\n\
\n\
[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,100,100,120,1\n\
\n\
[Events]\n\
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,0:00:00.20,0:00:01.00,Default,,0,0,0,,hello world\n";
        std::fs::write(&ass_path, ass_body).unwrap();
        let sidecars = awidat_render::fingerprint_ffmpeg_subtitle_sidecars(&[
            "ffmpeg".into(),
            "-vf".into(),
            format!("subtitles={}", ass_path.to_string_lossy()),
        ])
        .unwrap();

        // The synthesize_fixture render is 160x90. Bbox computed at that
        // resolution from the Style+Dialogue above (alignment=2 bottom-center,
        // PlayRes 1920x1080 with margin_l=margin_r=100, margin_v=120, fontsize=40):
        //   sx = 160/1920 ≈ 0.0833, sy = 90/1080 ≈ 0.0833
        //   line_height = round(40 * 1.2 * 0.0833) = 4
        //   margin_l_px = margin_r_px = round(100*0.0833) = 8
        //   margin_v_px = round(120*0.0833) = 10
        //   width = 160 - 16 = 144, x = 8, height = 4, y = 90 - 4 - 10 = 76.
        // Mobile safe-area inset (5% L/R, 10% T/B) → bbox 76..80 sits inside 9..81.
        let bbox: (u32, u32, u32, u32) = (8, 76, 144, 4);

        let render_manifest_path = awidat_render::manifest_path_for_output(&output_path);
        let manifest = awidat_render::RenderExecutionManifest::planned(
            awidat_render::RenderExecutionManifestInput {
                created_at: "2026-05-22T10:00:00Z".into(),
                awidat_version: "test".into(),
                project_root: dir.path().to_string_lossy().into_owned(),
                project_hash: None,
                timeline_hash: None,
                backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
                replay: awidat_render::RenderReplayPlan::FfmpegArgv {
                    argv: vec!["ffmpeg".into()],
                    cwd: Some(dir.path().to_string_lossy().into_owned()),
                },
                inputs: Vec::new(),
                outputs: vec![awidat_render::output_artifact(&output_path, true)],
                sidecars,
                limitations: Vec::new(),
                verification: None,
                metadata: std::collections::BTreeMap::from([
                    ("timeline_backend".into(), "timeline_ffmpeg_reencode".into()),
                    (
                        "timeline_backend_reason".into(),
                        "ffmpeg_with_libass_captions".into(),
                    ),
                    ("libass_caption_count".into(), "1".into()),
                    ("libass_layout_sidecar_count".into(), "1".into()),
                    ("libass_layout_playres".into(), "1920x1080".into()),
                    ("libass_layout_wrapped_sidecar_count".into(), "1".into()),
                    ("libass_layout_safe_area_sidecar_count".into(), "1".into()),
                    ("libass_layout_karaoke_sidecar_count".into(), "1".into()),
                    (
                        "libass_layout_sidecar_paths".into(),
                        ass_path.to_string_lossy().into_owned(),
                    ),
                    ("caption_authority".into(), "caption_overlays".into()),
                    ("caption_overlay_count".into(), "1".into()),
                    ("word_timed_caption_overlay_count".into(), "1".into()),
                    ("safe_area_caption_overlay_count".into(), "1".into()),
                    ("mobile_safe_area_caption_overlay_count".into(), "1".into()),
                    ("missing_safe_area_caption_overlay_count".into(), "0".into()),
                    ("subtitle_sidecar_cue_count".into(), "0".into()),
                    ("caption_warning_count".into(), "1".into()),
                    ("render_feature_id".into(), "ffmpeg_timeline_export".into()),
                    (
                        "render_feature_preview_supported".into(),
                        "not_supported".into(),
                    ),
                    ("render_feature_export_supported".into(), "supported".into()),
                    ("render_feature_approval_required".into(), "true".into()),
                    ("render_feature_limitation_count".into(), "0".into()),
                ]),
            },
        );
        awidat_render::write_render_manifest(&render_manifest_path, &manifest).unwrap();
        (dir, output_path, bbox)
    }

    #[tokio::test]
    async fn verify_render_uses_frame_pixel_scorer_when_render_output_present() {
        if awidat_render::ffmpeg_path().is_err() || awidat_render::ffprobe_path().is_err() {
            return;
        }
        use crate::caption_rendered_output_scorer::test_support::{
            InMemoryFrameSampler, caption_on_flat_background_frame,
        };
        let (dir, output_path, bbox) = scorer_test_fixture();
        let sampler = std::sync::Arc::new(InMemoryFrameSampler::new());
        sampler.insert(0.6, caption_on_flat_background_frame(160, 90, bbox));
        let options = VerifyRenderOptions {
            caption_frame_sampler_override: Some(sampler),
            ..VerifyRenderOptions::default()
        };
        let report = verify_render_output(dir.path(), &output_path, options)
            .await
            .unwrap();
        let gate = report
            .gates
            .iter()
            .find(|g| g.name == "caption_rendered_output_readable")
            .expect("caption rendered output gate");
        assert!(gate.passed, "gate details: {:?}", gate.details);
        assert_eq!(gate.details["reason"], "frame_pixel_scorer_passed");
    }

    #[tokio::test]
    async fn verify_render_falls_back_to_libass_layout_when_scorer_unavailable() {
        if awidat_render::ffmpeg_path().is_err() || awidat_render::ffprobe_path().is_err() {
            return;
        }
        use crate::caption_rendered_output_scorer::test_support::AlwaysUnavailableFrameSampler;
        let (dir, output_path, _bbox) = scorer_test_fixture();
        let sampler = std::sync::Arc::new(AlwaysUnavailableFrameSampler);
        let options = VerifyRenderOptions {
            caption_frame_sampler_override: Some(sampler),
            ..VerifyRenderOptions::default()
        };
        let report = verify_render_output(dir.path(), &output_path, options)
            .await
            .unwrap();
        let gate = report
            .gates
            .iter()
            .find(|g| g.name == "caption_rendered_output_readable")
            .expect("caption rendered output gate");
        assert!(gate.passed, "gate details: {:?}", gate.details);
        assert_eq!(
            gate.details["reason"],
            "frame_pixel_scorer_unavailable_fell_back_to_libass_layout"
        );
    }

    #[tokio::test]
    async fn verify_render_reports_frame_pixel_scorer_failed_on_failed_evidence() {
        if awidat_render::ffmpeg_path().is_err() || awidat_render::ffprobe_path().is_err() {
            return;
        }
        use crate::caption_rendered_output_scorer::test_support::{
            InMemoryFrameSampler, flat_frame,
        };
        let (dir, output_path, _bbox) = scorer_test_fixture();
        let sampler = std::sync::Arc::new(InMemoryFrameSampler::new());
        sampler.insert(0.6, flat_frame(160, 90, 128));
        let options = VerifyRenderOptions {
            caption_frame_sampler_override: Some(sampler),
            ..VerifyRenderOptions::default()
        };
        let report = verify_render_output(dir.path(), &output_path, options)
            .await
            .unwrap();
        let gate = report
            .gates
            .iter()
            .find(|g| g.name == "caption_rendered_output_readable")
            .expect("caption rendered output gate");
        assert!(!gate.passed, "gate details: {:?}", gate.details);
        assert_eq!(gate.details["reason"], "frame_pixel_scorer_failed");
    }
}
