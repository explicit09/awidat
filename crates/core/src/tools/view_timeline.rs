//! `view_timeline` tool — windowed view of the project's OTIO timeline.
//!
//! Per `PLAN.md` §6.3: "default window 60 seconds (≈ 80 lines of dense
//! text). Each line: clip-id | source | source-range | duration |
//! annotations." Per the survey of SWE-agent's windowed-file pattern,
//! the model needs a clear "where am I in the timeline" header on every
//! call — we include `window_start_s` / `window_end_s` / total_duration.
//!
//! v1 doesn't yet ship session-persisted scroll state (Codex-style
//! cursor). Each call is stateless: the model passes `start_s` + `lines`.
//! When session-state lands (week 5 TUI), we expose it via
//! [`crate::SessionEvent`] so the TUI can keep the model anchored
//! without it having to re-specify every turn.

use std::path::Path;

use async_trait::async_trait;
use awidat_proto::otio::{StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Default window length in seconds. Codex's `view_file` uses ~100 lines;
/// at ~1s per clip-line for podcast video, 60s ≈ a comfortable read.
const DEFAULT_WINDOW_S: f64 = 60.0;

/// Hard upper-cap on lines returned. Beyond this the model loses the plot.
const MAX_LINES: usize = 200;

/// Per-line text cap (transcript snippets, asset paths). Mirrors
/// Codex `list_dir`'s `MAX_ENTRY_LENGTH = 500`.
const MAX_LINE_LENGTH: usize = 500;

/// The `view_timeline` tool.
pub struct ViewTimelineTool;

#[derive(Debug, Deserialize)]
struct ViewTimelineArgs {
    /// Window start in seconds. Defaults to 0.0.
    #[serde(default)]
    start_s: Option<f64>,
    /// Window end in seconds. Optional; if omitted, `start_s + DEFAULT_WINDOW_S`.
    #[serde(default)]
    end_s: Option<f64>,
    /// Max lines to return (hard cap MAX_LINES).
    #[serde(default)]
    lines: Option<usize>,
}

#[async_trait]
impl ToolHandler for ViewTimelineTool {
    fn name(&self) -> &'static str {
        "view_timeline"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "view_timeline".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "start_s": { "type": "number", "minimum": 0.0,
                        "description": "Window start time in seconds. Default 0." },
                    "end_s": { "type": "number", "minimum": 0.0,
                        "description": "Window end. Default start_s + 60." },
                    "lines": { "type": "integer", "minimum": 1, "maximum": 200,
                        "description": "Max lines. Default 80, hard cap 200." }
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
        let args: ViewTimelineArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "view_timeline: invalid args ({e}). All fields optional; \
                 reasonable defaults are used."
            ))
        })?;

        let start_s = args.start_s.unwrap_or(0.0).max(0.0);
        let end_s = args.end_s.unwrap_or(start_s + DEFAULT_WINDOW_S);
        if end_s < start_s {
            return Err(FunctionCallError::RespondToModel(format!(
                "view_timeline: end_s ({end_s}) must be >= start_s ({start_s})"
            )));
        }
        let line_cap = args.lines.unwrap_or(80).min(MAX_LINES);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "view_timeline: failed to read project at {}: {e}",
                ctx.project_root.display()
            ))
        })?;

        Ok(ToolOutput::text(render(&project.timeline, start_s, end_s, line_cap)))
    }
}

/// Render the timeline within `[start_s, end_s)` as text. Header line +
/// per-clip lines. Lines that would land outside the window are omitted;
/// a footer reports counts.
fn render(timeline: &Timeline, start_s: f64, end_s: f64, line_cap: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut total_visible = 0usize;
    let mut total_clips = 0usize;
    let mut total_duration_s = 0.0_f64;

    for child in &timeline.tracks.children {
        if let StackChild::Track(track) = child {
            let mut t_cursor = 0.0_f64;
            for tchild in &track.children {
                let dur = child_duration_s(tchild);
                let clip_start = t_cursor;
                let clip_end = t_cursor + dur;
                t_cursor = clip_end;
                total_clips += 1;
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
        } else if let StackChild::Stack(_) | StackChild::Clip(_) | StackChild::Gap(_) = child {
            // Top-level non-track stack children: count but don't fold
            // into a track row (rare in v1).
        }
    }

    let header = format!(
        "timeline name={:?} window=[{:.3}s..{:.3}s) total_duration={:.3}s tracks={}",
        timeline.name,
        start_s,
        end_s,
        total_duration_s,
        timeline.tracks.children.len(),
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
            total_clips_in_window(timeline, start_s, end_s)
                .saturating_sub(total_visible)
        )
    };

    let mut out = String::with_capacity(header.len() + lines.iter().map(|l| l.len() + 1).sum::<usize>() + footer.len() + 8);
    out.push_str(&header);
    out.push('\n');
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    out.push_str(&footer);
    out
}

fn format_line(
    track: &awidat_proto::otio::Track,
    child: &TrackChild,
    start_s: f64,
    end_s: f64,
) -> String {
    let kind = match track.kind {
        awidat_proto::otio::TrackKind::Video => "V",
        awidat_proto::otio::TrackKind::Audio => "A",
    };
    match child {
        TrackChild::Clip(c) => {
            let media = match &c.media_reference {
                awidat_proto::otio::MediaReference::External(r) => r.target_url.clone(),
                awidat_proto::otio::MediaReference::Missing(_) => "<missing>".into(),
            };
            let active = if c.active { "" } else { " inactive" };
            format!(
                "[{kind} {:>7.3}-{:>7.3}s {:.3}s] clip {:?} → {media}{active}",
                start_s,
                end_s,
                end_s - start_s,
                c.name
            )
        }
        TrackChild::Gap(g) => format!(
            "[{kind} {:>7.3}-{:>7.3}s {:.3}s] gap {:?}",
            start_s,
            end_s,
            end_s - start_s,
            g.name
        ),
        TrackChild::Transition(t) => format!(
            "[{kind} {:>7.3}-{:>7.3}s {:.3}s] transition {:?} ({})",
            start_s,
            end_s,
            end_s - start_s,
            t.name,
            t.transition_type
        ),
        TrackChild::Stack(_) => format!(
            "[{kind} {:>7.3}-{:>7.3}s] nested-stack",
            start_s, end_s
        ),
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

const DESCRIPTION: &str = "\
Show clips in the project timeline within a time window. Each line is \
one clip/gap/transition: track-kind, time range, duration, name, media \
reference. Default window 60s starting at 0. The header shows total \
timeline duration; the footer notes how many clips are out of cap. Use \
`start_s`/`end_s`/`lines` to navigate. Stateless across calls — pass \
`start_s` to scroll.\
";

// Suppress unused-import warning for `Path` — kept for the future
// session-state methods we'll wire in week 5+.
#[allow(dead_code)]
fn _suppress_unused(p: &Path) -> &Path {
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackKind,
    };
    use awidat_proto::project::Project;
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "view_timeline".into(),
            args,
        }
    }

    /// Build a project with one video track containing 3 clips of 5s each
    /// (total 15s). Returns the temp dir guard.
    fn project_with_three_clips() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        for i in 0..3 {
            let mut c = Clip::empty(format!("clip-{i}"));
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/clip-{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::zero(24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            track.children.push(awidat_proto::otio::TrackChild::Clip(c));
        }
        project
            .timeline
            .tracks
            .children
            .push(awidat_proto::otio::StackChild::Track(track));
        project.write(dir.path()).unwrap();
        dir
    }

    #[tokio::test]
    async fn default_window_shows_everything_within_60s() {
        let dir = project_with_three_clips();
        let out = ViewTimelineTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap();
        // Header + 3 clip lines + footer.
        assert!(out.content.contains("clip \"clip-0\""));
        assert!(out.content.contains("clip \"clip-1\""));
        assert!(out.content.contains("clip \"clip-2\""));
        assert!(out.content.contains("total_duration=15.000s"));
    }

    #[tokio::test]
    async fn narrow_window_excludes_out_of_range_clips() {
        let dir = project_with_three_clips();
        let out = ViewTimelineTool
            .handle(
                invoke(serde_json::json!({"start_s": 7.0, "end_s": 8.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        // Only clip-1 (5s..10s) overlaps [7, 8).
        assert!(out.content.contains("clip \"clip-1\""));
        assert!(!out.content.contains("clip \"clip-0\""));
        assert!(!out.content.contains("clip \"clip-2\""));
    }

    #[tokio::test]
    async fn empty_window_emits_no_clips_message() {
        let dir = project_with_three_clips();
        let out = ViewTimelineTool
            .handle(
                invoke(serde_json::json!({"start_s": 100.0, "end_s": 110.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        assert!(out.content.contains("no clips in window"));
    }

    #[tokio::test]
    async fn end_before_start_is_respond_to_model() {
        let dir = project_with_three_clips();
        let err = ViewTimelineTool
            .handle(
                invoke(serde_json::json!({"start_s": 10.0, "end_s": 5.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("end_s"));
                assert!(msg.contains(">="));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[test]
    fn is_not_mutating() {
        let inv = invoke(serde_json::json!({}));
        assert!(!ViewTimelineTool.is_mutating(&inv));
    }
}
