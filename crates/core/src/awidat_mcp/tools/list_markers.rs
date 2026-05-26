//! `list_markers` — structured marker inventory across the project
//! timeline. Ported from `crates/core/src/tools/list_markers.rs` to
//! the in-process MCP server in step 5 of the codex-harness
//! migration.

use awidat_proto::awidat_meta::{TimelineMarker, TimelineMarkerCategory};
use awidat_proto::otio::{Clip, Marker, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `list_markers`. All optional with reasonable defaults.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListMarkersArgs {
    /// Window start in seconds. Defaults to 0.0.
    #[serde(default)]
    pub start_s: Option<f64>,
    /// Window end in seconds. Omit for no upper bound.
    #[serde(default)]
    pub end_s: Option<f64>,
    /// Exact marker id filter.
    #[serde(default)]
    pub marker_id: Option<String>,
    /// Exact label filter.
    #[serde(default)]
    pub label: Option<String>,
    /// Exact category filter.
    #[serde(default)]
    pub category: Option<String>,
    /// Include clip-level OTIO markers. Defaults true.
    #[serde(default)]
    pub include_clip: Option<bool>,
    /// Include timeline-level metadata markers. Defaults true.
    #[serde(default)]
    pub include_timeline: Option<bool>,
    /// Include guide-track metadata markers. Defaults true.
    #[serde(default)]
    pub include_guides: Option<bool>,
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

/// Run `list_markers` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; argument or
/// project-read errors return `Err(String)` which the MCP layer wraps
/// as a tool error result the model can act on.
pub fn run(args: ListMarkersArgs, ctx: McpToolCtx) -> Result<String, String> {
    validate_args(&args)?;

    let project = Project::read(&ctx.project_root).map_err(|e| {
        format!(
            "list_markers: failed to read project at {}: {e}",
            ctx.project_root.display()
        )
    })?;

    let mut markers = collect_markers(&project.timeline, &args);
    markers.sort_by(|a, b| {
        a.time_s
            .total_cmp(&b.time_s)
            .then_with(|| a.scope.cmp(b.scope))
            .then_with(|| a.label.cmp(&b.label))
    });
    serde_json::to_string_pretty(&ListMarkersResponse { markers })
        .map_err(|e| format!("list_markers: failed to serialize marker response: {e}"))
}

fn validate_args(args: &ListMarkersArgs) -> Result<(), String> {
    let start_s = args.start_s.unwrap_or(0.0);
    if !start_s.is_finite() || start_s < 0.0 {
        return Err(format!(
            "list_markers: start_s must be finite and >= 0, got {start_s}"
        ));
    }
    if let Some(end_s) = args.end_s {
        if !end_s.is_finite() || end_s < 0.0 {
            return Err(format!(
                "list_markers: end_s must be finite and >= 0, got {end_s}"
            ));
        }
        if end_s < start_s {
            return Err(format!(
                "list_markers: end_s ({end_s}) must be >= start_s ({start_s})"
            ));
        }
    }
    for (field, value) in [
        ("marker_id", args.marker_id.as_deref()),
        ("label", args.label.as_deref()),
        ("category", args.category.as_deref()),
    ] {
        if value.is_some_and(|s| s.trim().is_empty()) {
            return Err(format!("list_markers: {field} must not be empty"));
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

/// Tool description, served to the model via MCP `tools/list`.
pub const DESCRIPTION: &str = "\
List markers across the project timeline as JSON. Includes clip-level OTIO \
markers, timeline-level metadata markers, and guide-track markers by default. \
Use `start_s`/`end_s` for a timeline window, `marker_id`/`label`/`category` \
for exact matching, and the include_* flags to limit scopes. The result is \
read-only and suitable for finding marker ids before UpdateMarker/DeleteMarker \
EDL operations.\
";
