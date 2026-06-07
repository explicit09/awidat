//! `view_timeline` — windowed view of the project's OTIO timeline.
//! Ported in step 5 from `crates/core/src/tools/view_timeline.rs`.

use montage_proto::otio::{StackChild, Timeline, TrackChild};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

const DEFAULT_WINDOW_S: f64 = 60.0;
const MAX_LINES: usize = 200;
const MAX_LINE_LENGTH: usize = 500;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ViewTimelineArgs {
    /// Window start in seconds. Defaults to 0.0.
    #[serde(default)]
    pub start_s: Option<f64>,
    /// Window end in seconds. Optional; if omitted, `start_s + 60`.
    #[serde(default)]
    pub end_s: Option<f64>,
    /// Max lines to return. Default 80, hard cap 200.
    #[serde(default)]
    pub lines: Option<usize>,
}

pub fn run(args: ViewTimelineArgs, ctx: McpToolCtx) -> Result<String, String> {
    let start_s = args.start_s.unwrap_or(0.0).max(0.0);
    let end_s = args.end_s.unwrap_or(start_s + DEFAULT_WINDOW_S);
    if end_s < start_s {
        return Err(format!(
            "view_timeline: end_s ({end_s}) must be >= start_s ({start_s})"
        ));
    }
    let line_cap = args.lines.unwrap_or(80).min(MAX_LINES);

    let project = Project::read(&ctx.project_root).map_err(|e| {
        format!(
            "view_timeline: failed to read project at {}: {e}",
            ctx.project_root.display()
        )
    })?;

    Ok(render(&project.timeline, start_s, end_s, line_cap))
}

fn render(timeline: &Timeline, start_s: f64, end_s: f64, line_cap: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut total_visible = 0usize;
    let mut total_clips = 0usize;
    let mut total_duration_s = 0.0_f64;
    let mut track_summaries: Vec<String> = Vec::new();

    for child in &timeline.tracks.children {
        if let StackChild::Track(track) = child {
            let mut t_cursor = 0.0_f64;
            let mut track_clip_count = 0usize;
            for tchild in &track.children {
                let dur = child_duration_s(tchild);
                let clip_start = t_cursor;
                let clip_end = t_cursor + dur;
                t_cursor = clip_end;
                total_clips += 1;
                track_clip_count += 1;
                total_duration_s = total_duration_s.max(t_cursor);

                if clip_end <= start_s || clip_start >= end_s {
                    continue;
                }
                if total_visible >= line_cap {
                    continue;
                }
                let line = format_line(track, tchild, clip_start, clip_end);
                lines.push(truncate(line, MAX_LINE_LENGTH));
                total_visible += 1;
            }
            let kind_label = match track.kind {
                montage_proto::otio::TrackKind::Video => "video",
                montage_proto::otio::TrackKind::Audio => "audio",
            };
            let suffix = if track_clip_count == 0 {
                " EMPTY".to_string()
            } else {
                String::new()
            };
            track_summaries.push(format!(
                "{:?}({} {}ch){}",
                track.name, kind_label, track_clip_count, suffix
            ));
        }
    }

    append_visible_timeline_markers(timeline, start_s, end_s, &mut lines, line_cap);

    let track_list = if track_summaries.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", track_summaries.join(", "))
    };
    let header = format!(
        "timeline name={:?} window=[{:.3}s..{:.3}s) total_duration={:.3}s tracks={}={}",
        timeline.name,
        start_s,
        end_s,
        total_duration_s,
        timeline.tracks.children.len(),
        track_list,
    );
    let footer = if total_visible == 0 {
        format!(
            "(no clips in window; {} clips total in timeline)",
            total_clips
        )
    } else {
        format!(
            "({total_visible} of {total_clips} clips shown; \
             window contains {} additional out-of-cap clips)",
            total_clips_in_window(timeline, start_s, end_s).saturating_sub(total_visible)
        )
    };

    let mut out = String::with_capacity(
        header.len() + lines.iter().map(|l| l.len() + 1).sum::<usize>() + footer.len() + 8,
    );
    out.push_str(&header);
    out.push('\n');
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str(&footer);
    out
}

fn append_visible_timeline_markers(
    timeline: &Timeline,
    start_s: f64,
    end_s: f64,
    lines: &mut Vec<String>,
    line_cap: usize,
) {
    let Some(metadata) = timeline.metadata.montage.as_ref() else {
        return;
    };
    for marker in &metadata.timeline_markers {
        if lines.len() >= line_cap {
            return;
        }
        if marker_overlaps_window(marker.time_s, marker.duration_s, start_s, end_s) {
            lines.push(truncate(
                format_timeline_marker("marker", None, marker),
                MAX_LINE_LENGTH,
            ));
        }
    }
    for guide_track in &metadata.guide_tracks {
        for marker in &guide_track.markers {
            if lines.len() >= line_cap {
                return;
            }
            if marker_overlaps_window(marker.time_s, marker.duration_s, start_s, end_s) {
                lines.push(truncate(
                    format_timeline_marker("guide", Some(guide_track.id.as_str()), marker),
                    MAX_LINE_LENGTH,
                ));
            }
        }
    }
}

fn marker_overlaps_window(
    marker_start_s: f64,
    marker_duration_s: Option<f64>,
    window_start_s: f64,
    window_end_s: f64,
) -> bool {
    let marker_end_s = marker_start_s + marker_duration_s.unwrap_or(0.0).max(0.0);
    if marker_duration_s.unwrap_or(0.0) == 0.0 {
        marker_start_s >= window_start_s && marker_start_s < window_end_s
    } else {
        marker_end_s > window_start_s && marker_start_s < window_end_s
    }
}

fn format_timeline_marker(
    prefix: &str,
    guide_track_id: Option<&str>,
    marker: &montage_proto::montage_meta::TimelineMarker,
) -> String {
    let marker_id = match guide_track_id {
        Some(track_id) => format!("{track_id}/{}", marker.id),
        None => marker.id.clone(),
    };
    let duration = marker
        .duration_s
        .map(|duration| format!(" duration={duration:.3}s"))
        .unwrap_or_default();
    let category = marker
        .category
        .map(|category| format!(" category={}", timeline_marker_category_label(category)))
        .unwrap_or_default();
    format!(
        "[{:>7.3}s] {prefix} {marker_id} {:?}{duration}{category}",
        marker.time_s, marker.label
    )
}

fn timeline_marker_category_label(
    category: montage_proto::montage_meta::TimelineMarkerCategory,
) -> &'static str {
    match category {
        montage_proto::montage_meta::TimelineMarkerCategory::Review => "review",
        montage_proto::montage_meta::TimelineMarkerCategory::Section => "section",
        montage_proto::montage_meta::TimelineMarkerCategory::ExportRange => "export_range",
        montage_proto::montage_meta::TimelineMarkerCategory::Note => "note",
        montage_proto::montage_meta::TimelineMarkerCategory::Guide => "guide",
    }
}

fn format_line(
    track: &montage_proto::otio::Track,
    child: &TrackChild,
    start_s: f64,
    end_s: f64,
) -> String {
    let kind = match track.kind {
        montage_proto::otio::TrackKind::Video => "V",
        montage_proto::otio::TrackKind::Audio => "A",
    };
    match child {
        TrackChild::Clip(c) => {
            let media = match &c.media_reference {
                montage_proto::otio::MediaReference::External(r) => r.target_url.clone(),
                montage_proto::otio::MediaReference::Missing(_) => "<missing>".into(),
            };
            let clip_uuid = c
                .metadata
                .montage
                .as_ref()
                .and_then(|m| m.extra.get("clip_uuid"))
                .and_then(|v| v.as_str())
                .unwrap_or(c.name.as_str());
            let active = if c.active { "" } else { " inactive" };
            let source_bounds = c
                .source_range
                .as_ref()
                .map(|r| {
                    let source_start = r.start_time.to_seconds();
                    let source_end = source_start + r.duration.to_seconds();
                    format!(" source=[{source_start:.3}..{source_end:.3}]")
                })
                .unwrap_or_default();
            format!(
                "[{kind} {:>7.3}-{:>7.3}s {:.3}s] clip {:?} anchor=clip_uuid={}{} → {media}{active}",
                start_s,
                end_s,
                end_s - start_s,
                c.name,
                clip_uuid,
                source_bounds,
            )
        }
        TrackChild::Gap(g) => format!(
            "[{kind} {:>7.3}-{:>7.3}s {:.3}s] gap {:?}",
            start_s,
            end_s,
            end_s - start_s,
            g.name
        ),
        TrackChild::Transition(t) => {
            let in_offset_s = t.in_offset.to_seconds();
            let out_offset_s = t.out_offset.to_seconds();
            let visual_duration_s = in_offset_s + out_offset_s;
            let visual_start_s = (start_s - in_offset_s).max(0.0);
            let visual_end_s = start_s + out_offset_s;
            format!(
                "[{kind} {:>7.3}-{:>7.3}s visual={visual_duration_s:.3}s cut={start_s:.3}s] transition {:?} ({})",
                visual_start_s, visual_end_s, t.name, t.transition_type
            )
        }
        TrackChild::Stack(_) => format!("[{kind} {:>7.3}-{:>7.3}s] nested-stack", start_s, end_s),
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
        TrackChild::Transition(_) => 0.0,
        TrackChild::Stack(_) => 0.0,
    }
}

fn total_clips_in_window(timeline: &Timeline, start_s: f64, end_s: f64) -> usize {
    let mut count = 0;
    for child in &timeline.tracks.children {
        if let StackChild::Track(track) = child {
            let mut cursor = 0.0_f64;
            for tchild in &track.children {
                let dur = child_duration_s(tchild);
                let cs = cursor;
                let ce = cursor + dur;
                cursor = ce;
                if ce > start_s && cs < end_s {
                    count += 1;
                }
            }
        }
    }
    count
}

fn truncate(s: String, cap: usize) -> String {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

pub const DESCRIPTION: &str = "\
Show clips in the project timeline within a time window. Each line is \
one clip/gap/transition: track-kind, timeline time range, duration, name, \
the exact `anchor=clip_uuid=<clip name>` value to use in apply_edl, \
current `source=[start..end]` bounds, and media reference. Transition \
lines show the visual range and the centered cut time (`cut=<seconds>`). For a user \
request like \"trim the first N seconds\" of an existing clip, set Trim \
Clip `start` to current source start + N; for \"trim the last N seconds\", \
set `end` to current source end - N. Default window 60s starting at 0. \
The header shows total timeline duration; the footer notes how many clips \
are out of cap. Use `start_s`/`end_s`/`lines` to navigate. Stateless across \
calls — pass `start_s` to scroll.";
