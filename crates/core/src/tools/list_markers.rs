//! `list_markers` tool — structured marker inventory for clip,
//! timeline, and guide-track markers.

use std::path::Path;

use async_trait::async_trait;
use awidat_proto::awidat_meta::{TimelineMarker, TimelineMarkerCategory};
use awidat_proto::otio::{Clip, Marker, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// The `list_markers` tool.
pub struct ListMarkersTool;

#[derive(Debug, Deserialize)]
struct ListMarkersArgs {
    /// Window start in seconds. Defaults to 0.0.
    #[serde(default)]
    start_s: Option<f64>,
    /// Window end in seconds. Optional; omitted means no upper bound.
    #[serde(default)]
    end_s: Option<f64>,
    /// Marker id filter.
    #[serde(default)]
    marker_id: Option<String>,
    /// Exact label filter.
    #[serde(default)]
    label: Option<String>,
    /// Exact category filter.
    #[serde(default)]
    category: Option<String>,
    /// Include clip-level OTIO markers. Defaults true.
    #[serde(default)]
    include_clip: Option<bool>,
    /// Include timeline-level metadata markers. Defaults true.
    #[serde(default)]
    include_timeline: Option<bool>,
    /// Include guide-track metadata markers. Defaults true.
    #[serde(default)]
    include_guides: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ListMarkersResponse {
    markers: Vec<MarkerEntry>,
}

#[derive(Debug, Serialize)]
struct MarkerEntry {
    scope: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    marker_id: Option<String>,
    time_s: f64,
    duration_s: f64,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    track_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clip_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    guide_track_id: Option<String>,
}

#[async_trait]
impl ToolHandler for ListMarkersTool {
    fn name(&self) -> &'static str {
        "list_markers"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "list_markers".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "start_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Window start time in seconds. Default 0."
                    },
                    "end_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Window end time in seconds. Omit for no upper bound."
                    },
                    "marker_id": {
                        "type": "string",
                        "description": "Exact marker id to return."
                    },
                    "label": {
                        "type": "string",
                        "description": "Exact marker label to return."
                    },
                    "category": {
                        "type": "string",
                        "description": "Exact marker category to return."
                    },
                    "include_clip": {
                        "type": "boolean",
                        "description": "Include clip-level OTIO markers. Default true."
                    },
                    "include_timeline": {
                        "type": "boolean",
                        "description": "Include timeline-level markers. Default true."
                    },
                    "include_guides": {
                        "type": "boolean",
                        "description": "Include guide-track markers. Default true."
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
        let args: ListMarkersArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_markers: invalid args ({e}). All fields optional."
            ))
        })?;
        validate_args(&args)?;

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_markers: failed to read project at {}: {e}",
                ctx.project_root.display()
            ))
        })?;

        let mut markers = collect_markers(&project.timeline, &args);
        markers.sort_by(|a, b| {
            a.time_s
                .total_cmp(&b.time_s)
                .then_with(|| a.scope.cmp(b.scope))
                .then_with(|| a.label.cmp(&b.label))
        });
        let body = serde_json::to_string_pretty(&ListMarkersResponse { markers }).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "list_markers: failed to serialize marker response: {e}"
            ))
        })?;
        Ok(ToolOutput::text(body))
    }
}

fn validate_args(args: &ListMarkersArgs) -> Result<(), FunctionCallError> {
    let start_s = args.start_s.unwrap_or(0.0);
    if !start_s.is_finite() || start_s < 0.0 {
        return Err(FunctionCallError::RespondToModel(format!(
            "list_markers: start_s must be finite and >= 0, got {start_s}"
        )));
    }
    if let Some(end_s) = args.end_s {
        if !end_s.is_finite() || end_s < 0.0 {
            return Err(FunctionCallError::RespondToModel(format!(
                "list_markers: end_s must be finite and >= 0, got {end_s}"
            )));
        }
        if end_s < start_s {
            return Err(FunctionCallError::RespondToModel(format!(
                "list_markers: end_s ({end_s}) must be >= start_s ({start_s})"
            )));
        }
    }
    for (field, value) in [
        ("marker_id", args.marker_id.as_deref()),
        ("label", args.label.as_deref()),
        ("category", args.category.as_deref()),
    ] {
        if value.is_some_and(|s| s.trim().is_empty()) {
            return Err(FunctionCallError::RespondToModel(format!(
                "list_markers: {field} must not be empty"
            )));
        }
    }
    Ok(())
}

fn collect_markers(timeline: &Timeline, args: &ListMarkersArgs) -> Vec<MarkerEntry> {
    let mut markers = Vec::new();
    if args.include_clip.unwrap_or(true) {
        collect_clip_markers(timeline, args, &mut markers);
    }
    if args.include_timeline.unwrap_or(true) || args.include_guides.unwrap_or(true) {
        collect_metadata_markers(timeline, args, &mut markers);
    }
    markers
}

fn collect_clip_markers(
    timeline: &Timeline,
    args: &ListMarkersArgs,
    markers: &mut Vec<MarkerEntry>,
) {
    for (track_index, child) in timeline.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = child else {
            continue;
        };
        let mut cursor_s = 0.0;
        for (clip_index, child) in track.children.iter().enumerate() {
            let duration_s = child_duration_s(child);
            let clip_start_s = cursor_s;
            cursor_s += duration_s;
            let TrackChild::Clip(clip) = child else {
                continue;
            };
            for marker in &clip.markers {
                let time_s = clip_start_s + marker.marked_range.start_time.to_seconds();
                let duration_s = marker.marked_range.duration.to_seconds().max(0.0);
                let entry = clip_marker_entry(
                    marker,
                    clip,
                    track.name.clone(),
                    track_index,
                    clip_index,
                    time_s,
                    duration_s,
                );
                if matches_filters(&entry, args) {
                    markers.push(entry);
                }
            }
        }
    }
}

fn collect_metadata_markers(
    timeline: &Timeline,
    args: &ListMarkersArgs,
    markers: &mut Vec<MarkerEntry>,
) {
    let Some(metadata) = timeline.metadata.awidat.as_ref() else {
        return;
    };
    if args.include_timeline.unwrap_or(true) {
        for marker in &metadata.timeline_markers {
            let entry = timeline_marker_entry("timeline", None, marker);
            if matches_filters(&entry, args) {
                markers.push(entry);
            }
        }
    }
    if args.include_guides.unwrap_or(true) {
        for guide_track in &metadata.guide_tracks {
            for marker in &guide_track.markers {
                let entry = timeline_marker_entry("guide", Some(guide_track.id.clone()), marker);
                if matches_filters(&entry, args) {
                    markers.push(entry);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn clip_marker_entry(
    marker: &Marker,
    clip: &Clip,
    track: String,
    track_index: usize,
    clip_index: usize,
    time_s: f64,
    duration_s: f64,
) -> MarkerEntry {
    let metadata = marker.metadata.awidat.as_ref();
    MarkerEntry {
        scope: "clip",
        marker_id: marker_metadata_id(marker).map(ToOwned::to_owned),
        time_s,
        duration_s,
        label: marker.name.clone(),
        category: metadata.and_then(|meta| meta.category.clone()),
        note: marker
            .comment
            .clone()
            .or_else(|| metadata.and_then(|meta| meta.note.clone())),
        track: Some(track),
        track_index: Some(track_index),
        clip_index: Some(clip_index),
        clip_name: Some(clip.name.clone()),
        guide_track_id: None,
    }
}

fn timeline_marker_entry(
    scope: &'static str,
    guide_track_id: Option<String>,
    marker: &TimelineMarker,
) -> MarkerEntry {
    MarkerEntry {
        scope,
        marker_id: Some(marker.id.clone()),
        time_s: marker.time_s,
        duration_s: marker.duration_s.unwrap_or(0.0).max(0.0),
        label: marker.label.clone(),
        category: marker
            .category
            .map(timeline_marker_category_label)
            .map(str::to_owned),
        note: marker.note.clone(),
        track: None,
        track_index: None,
        clip_index: None,
        clip_name: None,
        guide_track_id,
    }
}

fn matches_filters(entry: &MarkerEntry, args: &ListMarkersArgs) -> bool {
    let start_s = args.start_s.unwrap_or(0.0);
    let end_s = args.end_s.unwrap_or(f64::INFINITY);
    if !marker_overlaps_window(entry.time_s, entry.duration_s, start_s, end_s) {
        return false;
    }
    if let Some(marker_id) = args.marker_id.as_deref()
        && !marker_id_matches(entry, marker_id.trim())
    {
        return false;
    }
    if let Some(label) = args.label.as_deref()
        && entry.label != label.trim()
    {
        return false;
    }
    if let Some(category) = args.category.as_deref()
        && entry.category.as_deref() != Some(category.trim())
    {
        return false;
    }
    true
}

fn marker_id_matches(entry: &MarkerEntry, expected: &str) -> bool {
    if entry.marker_id.as_deref() == Some(expected) {
        return true;
    }
    if let (Some(guide_track_id), Some(marker_id)) =
        (entry.guide_track_id.as_deref(), entry.marker_id.as_deref())
    {
        return expected == format!("{guide_track_id}/{marker_id}");
    }
    false
}

fn marker_overlaps_window(
    marker_start_s: f64,
    marker_duration_s: f64,
    window_start_s: f64,
    window_end_s: f64,
) -> bool {
    if marker_duration_s == 0.0 {
        marker_start_s >= window_start_s && marker_start_s < window_end_s
    } else {
        marker_start_s + marker_duration_s > window_start_s && marker_start_s < window_end_s
    }
}

fn marker_metadata_id(marker: &Marker) -> Option<&str> {
    marker
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("id"))
        .and_then(serde_json::Value::as_str)
}

fn timeline_marker_category_label(category: TimelineMarkerCategory) -> &'static str {
    match category {
        TimelineMarkerCategory::Review => "review",
        TimelineMarkerCategory::Section => "section",
        TimelineMarkerCategory::ExportRange => "export_range",
        TimelineMarkerCategory::Note => "note",
        TimelineMarkerCategory::Guide => "guide",
    }
}

fn child_duration_s(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(c) => c
            .source_range
            .as_ref()
            .map(|r| r.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(g) => g.source_range.duration.to_seconds(),
        TrackChild::Transition(t) => {
            t.in_offset.to_seconds().max(0.0) + t.out_offset.to_seconds().max(0.0)
        }
        TrackChild::Stack(stack) => stack_duration_s(stack),
    }
}

fn stack_duration_s(stack: &awidat_proto::otio::Stack) -> f64 {
    stack
        .children
        .iter()
        .map(stack_child_duration_s)
        .fold(0.0, f64::max)
}

fn stack_child_duration_s(child: &StackChild) -> f64 {
    match child {
        StackChild::Track(track) => track.children.iter().map(child_duration_s).sum(),
        StackChild::Stack(stack) => stack_duration_s(stack),
        StackChild::Clip(clip) => clip
            .source_range
            .as_ref()
            .map(|range| range.duration.to_seconds())
            .unwrap_or(0.0),
        StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
    }
}

const DESCRIPTION: &str = "\
List markers across the project timeline as JSON. Includes clip-level OTIO \
markers, timeline-level metadata markers, and guide-track markers by default. \
Use `start_s`/`end_s` for a timeline window, `marker_id`/`label`/`category` \
for exact matching, and the include_* flags to limit scopes. The result is \
read-only and suitable for finding marker ids before UpdateMarker/DeleteMarker \
EDL operations.\
";

// Suppress unused-import warning for `Path`; tests use the same helper
// shape as sibling tools and future navigation state may need it here.
#[allow(dead_code)]
fn _suppress_unused(p: &Path) -> &Path {
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{AwidatMarkerMetadata, GuideTrack};
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackKind,
    };
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "list_markers".into(),
            args,
        }
    }

    fn project_with_markers() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/clip-0.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        let mut marker = Marker::new(
            "Clip beat",
            TimeRange::new(
                RationalTime::new(3.0 * 24.0, 24.0),
                RationalTime::new(1.5 * 24.0, 24.0),
            ),
        );
        marker.comment = Some("Clip note".into());
        let mut marker_metadata = AwidatMarkerMetadata {
            category: Some("select".into()),
            note: Some("Metadata note".into()),
            ..AwidatMarkerMetadata::default()
        };
        marker_metadata.extra.insert(
            "id".into(),
            serde_json::Value::String("marker-0-0-0".into()),
        );
        marker.metadata.awidat = Some(marker_metadata);
        clip.markers.push(marker);
        track.children.push(TrackChild::Clip(clip));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));

        let metadata = project
            .timeline
            .metadata
            .awidat
            .get_or_insert_with(Default::default);
        metadata.timeline_markers.push(TimelineMarker {
            id: "timeline-hook".into(),
            label: "Timeline hook".into(),
            time_s: 4.0,
            duration_s: None,
            category: Some(TimelineMarkerCategory::Review),
            note: Some("Timeline note".into()),
            ..TimelineMarker::default()
        });
        metadata.guide_tracks.push(GuideTrack {
            id: "sections".into(),
            label: "Sections".into(),
            markers: vec![TimelineMarker {
                id: "section-a".into(),
                label: "Section A".into(),
                time_s: 6.0,
                duration_s: Some(2.0),
                category: Some(TimelineMarkerCategory::Section),
                ..TimelineMarker::default()
            }],
            ..GuideTrack::default()
        });
        project.write(dir.path()).unwrap();
        dir
    }

    #[tokio::test]
    async fn lists_clip_timeline_and_guide_markers_as_json() {
        let dir = project_with_markers();
        let out = ListMarkersTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let markers = body["markers"].as_array().unwrap();

        assert_eq!(markers.len(), 3);
        assert!(markers.iter().any(|entry| {
            entry["scope"] == "clip"
                && entry["marker_id"] == "marker-0-0-0"
                && entry["clip_name"] == "clip-0"
                && entry["time_s"] == 3.0
                && entry["duration_s"] == 1.5
        }));
        assert!(markers.iter().any(|entry| {
            entry["scope"] == "timeline"
                && entry["marker_id"] == "timeline-hook"
                && entry["category"] == "review"
        }));
        assert!(markers.iter().any(|entry| {
            entry["scope"] == "guide"
                && entry["marker_id"] == "section-a"
                && entry["guide_track_id"] == "sections"
        }));
    }

    #[tokio::test]
    async fn marker_filters_limit_scope_and_category() {
        let dir = project_with_markers();
        let out = ListMarkersTool
            .handle(
                invoke(serde_json::json!({
                    "include_clip": false,
                    "include_timeline": false,
                    "category": "section"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let markers = body["markers"].as_array().unwrap();

        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0]["scope"], "guide");
        assert_eq!(markers[0]["marker_id"], "section-a");
    }

    #[tokio::test]
    async fn rejects_mutation_gate_for_marker_listing() {
        let invocation = invoke(serde_json::json!({}));

        assert!(!ListMarkersTool.is_mutating(&invocation));
    }

    #[tokio::test]
    async fn invalid_window_is_respond_to_model() {
        let dir = project_with_markers();
        let err = ListMarkersTool
            .handle(
                invoke(serde_json::json!({"start_s": 5.0, "end_s": 4.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("end_s")));
    }
}
