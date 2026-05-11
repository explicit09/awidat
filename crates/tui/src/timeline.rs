//! Timeline pane — windowed view of `project.otio.json` on disk.
//!
//! Refreshes whenever `apply_edl` reports a successful commit (its
//! ToolResult is `Ok`). We re-read the project from disk and rebuild
//! the row list. This keeps the UI authoritative — what's painted is
//! what's on disk, no shadow-state drift.
//!
//! Render shape per row:
//!
//! ```text
//! V1 │ clip-0   alpha clip about Stripe payments  [0.000s..5.000s] (5.000s)
//! V1 │ clip-2   charlie clip about the city sky…  [5.000s..15.000s] (10.000s)
//! ```
//!
//! Tracks group by name; clips render in flow order. The pane scrolls
//! vertically when there are more clips than visible rows.

use std::path::{Path, PathBuf};

use awidat_proto::otio::{StackChild, TrackChild};
use awidat_proto::project::Project;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// One renderable row in the timeline pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Track name the clip lives on.
    pub track_name: String,
    /// Clip name (e.g. `clip-0`).
    pub clip_name: String,
    /// Optional transcript-snippet anchor for the clip.
    pub snippet: Option<String>,
    /// Start time in seconds within the timeline.
    pub start_s: f64,
    /// End time in seconds within the timeline.
    pub end_s: f64,
}

/// Timeline pane state.
pub struct Timeline {
    project_root: PathBuf,
    rows: Vec<Row>,
    /// Last successful read; carried so we can show stale data with a
    /// banner instead of a blank pane on transient read errors.
    error: Option<String>,
    total_duration_s: f64,
    /// Vertical scroll offset.
    scroll: u16,
}

impl Timeline {
    /// Build a timeline pane rooted at the project. Performs an initial
    /// read; failures land in `self.error`.
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        let mut t = Self {
            project_root: project_root.into(),
            rows: Vec::new(),
            error: None,
            total_duration_s: 0.0,
            scroll: 0,
        };
        t.refresh();
        t
    }

    /// Re-read the project from disk and rebuild the row list. Cheap
    /// enough to call on every `apply_edl` result.
    pub fn refresh(&mut self) {
        match Project::read(&self.project_root) {
            Ok(project) => {
                let (rows, total) = project_to_rows(&project);
                self.rows = rows;
                self.total_duration_s = total;
                self.error = None;
            }
            Err(e) => {
                // Don't clear `rows` — stale-data is more useful than a
                // blank pane when the read transiently fails.
                self.error = Some(format!("read error: {e}"));
            }
        }
    }

    /// Number of clips currently shown.
    pub fn clip_count(&self) -> usize {
        self.rows.len()
    }

    /// Total duration in seconds.
    pub fn total_duration_s(&self) -> f64 {
        self.total_duration_s
    }

    /// Snapshot the current rows. Used by the App to capture state
    /// before `apply_edl` runs so it can produce a before/after diff
    /// crumb after the commit.
    pub fn snapshot(&self) -> Vec<Row> {
        self.rows.clone()
    }
}

fn project_to_rows(project: &Project) -> (Vec<Row>, f64) {
    let mut rows = Vec::new();
    let mut total = 0.0_f64;
    for child in &project.timeline.tracks.children {
        if let StackChild::Track(track) = child {
            let mut cursor = 0.0_f64;
            for tchild in &track.children {
                let dur = child_duration_s(tchild);
                let start = cursor;
                let end = cursor + dur;
                cursor = end;
                if let TrackChild::Clip(clip) = tchild {
                    let snippet = clip
                        .metadata
                        .awidat
                        .as_ref()
                        .and_then(|m| m.anchor.as_ref())
                        .and_then(|a| a.transcript_snippet.as_ref())
                        .cloned();
                    rows.push(Row {
                        track_name: track.name.clone(),
                        clip_name: clip.name.clone(),
                        snippet,
                        start_s: start,
                        end_s: end,
                    });
                }
            }
            total = total.max(cursor);
        }
    }
    (rows, total)
}

fn child_duration_s(tchild: &TrackChild) -> f64 {
    match tchild {
        TrackChild::Clip(c) => c
            .source_range
            .as_ref()
            .map_or(0.0, |r| r.duration.to_seconds()),
        TrackChild::Gap(g) => g.source_range.duration.to_seconds(),
        TrackChild::Transition(t) => t.in_offset.to_seconds() + t.out_offset.to_seconds(),
        TrackChild::Stack(_) => 0.0,
    }
}

fn truncate(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(cap.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

impl Widget for &Timeline {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        // On read errors the header surfaces a stale banner instead of
        // the normal stat line.
        let header = if let Some(err) = &self.error {
            Line::from(Span::styled(
                format!("timeline (stale — {err})"),
                Style::default().fg(Color::Yellow),
            ))
        } else if self.rows.is_empty() {
            Line::from(Span::styled(
                "timeline · empty",
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(vec![
                Span::styled("timeline ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} clip", self.rows.len()),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    if self.rows.len() == 1 { "" } else { "s" },
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!(" · {:.2}s", self.total_duration_s),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        };

        let mut lines = vec![header];
        for row in &self.rows {
            let snippet = row
                .snippet
                .as_deref()
                .map(|s| truncate(s, 48))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", row.track_name),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{:<10}", truncate(&row.clip_name, 10)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:>6.2}s ", row.end_s - row.start_s),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(snippet, Style::default().fg(Color::DarkGray)),
            ]));
        }

        Paragraph::new(lines)
            .scroll((self.scroll, 0))
            .render(area, buf);
    }
}

/// Public so tests in other modules can build a Timeline against a
/// known fixture project. Callers outside the crate use [`Timeline::new`].
#[doc(hidden)]
pub fn _new_for_test(root: &Path) -> Timeline {
    Timeline::new(root)
}

/// Diff two snapshots and return human-readable change lines.
///
/// Order: deletions first, then trims, then inserts. Each line is a
/// single short sentence the chat pane can show as a crumb. The diff is
/// keyed on `clip_name` since reorders + trims preserve the name. This
/// matches how the agent thinks about edits ("trim clip-0 to 5s") and
/// keeps the diff stable across track moves.
pub fn diff_snapshots(before: &[Row], after: &[Row]) -> Vec<String> {
    use std::collections::HashMap;
    let before_by_name: HashMap<&str, &Row> =
        before.iter().map(|r| (r.clip_name.as_str(), r)).collect();
    let after_by_name: HashMap<&str, &Row> =
        after.iter().map(|r| (r.clip_name.as_str(), r)).collect();

    let mut deletions = Vec::new();
    let mut trims = Vec::new();
    let mut inserts = Vec::new();

    for r in before {
        if !after_by_name.contains_key(r.clip_name.as_str()) {
            deletions.push(format!(
                "− {} deleted (was {:.2}s)",
                r.clip_name,
                r.end_s - r.start_s
            ));
        }
    }
    for b in before {
        if let Some(a) = after_by_name.get(b.clip_name.as_str()) {
            let b_dur = b.end_s - b.start_s;
            let a_dur = a.end_s - a.start_s;
            if (a_dur - b_dur).abs() > 0.01 {
                trims.push(format!(
                    "~ {} trimmed {:.2}s → {:.2}s",
                    b.clip_name, b_dur, a_dur
                ));
            }
        }
    }
    for r in after {
        if !before_by_name.contains_key(r.clip_name.as_str()) {
            inserts.push(format!(
                "+ {} added ({:.2}s)",
                r.clip_name,
                r.end_s - r.start_s
            ));
        }
    }

    let mut out = Vec::new();
    out.extend(deletions);
    out.extend(trims);
    out.extend(inserts);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };

    fn seed_project(snippets: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, snip) in snippets.iter().enumerate() {
            let mut clip = Clip::empty(format!("clip-{i}"));
            clip.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/ep-{i}.mp4")));
            clip.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            clip.metadata = ClipMetadata {
                awidat: Some(AwidatClipMetadata {
                    anchor: Some(AwAnchor {
                        transcript_snippet: Some((*snip).to_string()),
                        ..AwAnchor::default()
                    }),
                    ..AwidatClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(clip));
        }
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();
        dir
    }

    #[test]
    fn reads_clip_count_and_total_duration() {
        let dir = seed_project(&["alpha", "bravo", "charlie"]);
        let t = Timeline::new(dir.path());
        assert_eq!(t.clip_count(), 3);
        assert!((t.total_duration_s() - 15.0).abs() < 0.01);
    }

    #[test]
    fn refresh_reflects_disk_change() {
        let dir = seed_project(&["alpha", "bravo"]);
        let mut t = Timeline::new(dir.path());
        assert_eq!(t.clip_count(), 2);

        // Manually rewrite the project with a third clip.
        let mut project = Project::read(dir.path()).unwrap();
        let StackChild::Track(track) = &mut project.timeline.tracks.children[0] else {
            panic!()
        };
        let mut new_clip = Clip::empty("clip-2");
        new_clip.media_reference = MediaReference::External(ExternalReference::new("raw/ep-2.mp4"));
        new_clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(new_clip));
        project.write(dir.path()).unwrap();

        t.refresh();
        assert_eq!(t.clip_count(), 3);
    }

    #[test]
    fn missing_project_records_error_but_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let t = Timeline::new(dir.path());
        assert_eq!(t.clip_count(), 0);
        assert!(t.error.is_some(), "missing project should set error");
    }

    fn row(name: &str, start: f64, end: f64) -> Row {
        Row {
            track_name: "V1".into(),
            clip_name: name.into(),
            snippet: None,
            start_s: start,
            end_s: end,
        }
    }

    #[test]
    fn diff_detects_deletion() {
        let before = vec![row("clip-0", 0.0, 5.0), row("clip-1", 5.0, 10.0)];
        let after = vec![row("clip-0", 0.0, 5.0)];
        let d = diff_snapshots(&before, &after);
        assert_eq!(d.len(), 1);
        assert!(d[0].contains("clip-1"));
        assert!(d[0].starts_with("−"));
    }

    #[test]
    fn diff_detects_trim() {
        let before = vec![row("clip-0", 0.0, 10.0)];
        let after = vec![row("clip-0", 0.0, 5.0)];
        let d = diff_snapshots(&before, &after);
        assert_eq!(d.len(), 1);
        assert!(d[0].contains("trimmed"));
        assert!(d[0].contains("10.00"));
        assert!(d[0].contains("5.00"));
    }

    #[test]
    fn diff_detects_insert() {
        let before = vec![row("clip-0", 0.0, 5.0)];
        let after = vec![row("clip-0", 0.0, 5.0), row("clip-2", 5.0, 7.5)];
        let d = diff_snapshots(&before, &after);
        assert_eq!(d.len(), 1);
        assert!(d[0].starts_with("+"));
        assert!(d[0].contains("clip-2"));
    }

    #[test]
    fn diff_orders_delete_then_trim_then_insert() {
        let before = vec![row("clip-0", 0.0, 10.0), row("clip-1", 10.0, 15.0)];
        let after = vec![
            row("clip-0", 0.0, 5.0), // trimmed
            row("clip-2", 5.0, 7.5), // inserted
                                     // clip-1 deleted
        ];
        let d = diff_snapshots(&before, &after);
        assert_eq!(d.len(), 3);
        assert!(
            d[0].starts_with("−") && d[0].contains("clip-1"),
            "delete first: {:?}",
            d
        );
        assert!(
            d[1].contains("trimmed") && d[1].contains("clip-0"),
            "trim second: {:?}",
            d
        );
        assert!(
            d[2].starts_with("+") && d[2].contains("clip-2"),
            "insert third: {:?}",
            d
        );
    }

    #[test]
    fn diff_empty_when_unchanged() {
        let before = vec![row("clip-0", 0.0, 5.0)];
        let after = before.clone();
        assert!(diff_snapshots(&before, &after).is_empty());
    }
}
