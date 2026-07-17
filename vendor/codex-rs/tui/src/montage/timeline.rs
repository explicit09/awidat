//! Timeline pane — windowed view of `project.otio.json` on disk.
//!
//! Ported in step 6 of the codex-harness migration from
//! `crates/tui/src/timeline.rs`. The Montage-loop-specific session
//! event wiring is dropped; refresh is now driven by codex's app
//! event loop (see `MontagePanel::refresh`). Pure ratatui widget +
//! state — reads from disk on `refresh`, never writes.

use std::path::{Path, PathBuf};

use montage_proto::otio::{StackChild, TrackChild};
use montage_proto::project::Project;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// One renderable row in the timeline pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub track_name: String,
    pub clip_name: String,
    pub snippet: Option<String>,
    pub start_s: f64,
    pub end_s: f64,
}

/// Timeline pane state.
pub struct Timeline {
    project_root: PathBuf,
    rows: Vec<Row>,
    error: Option<String>,
    total_duration_s: f64,
    scroll: u16,
}

impl Timeline {
    /// Build a timeline pane rooted at the project. Performs an
    /// initial read; failures land in `self.error` and we render a
    /// stale-data banner instead of panicking.
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

    /// Re-read the project from disk and rebuild the row list.
    pub fn refresh(&mut self) {
        match Project::read(&self.project_root) {
            Ok(project) => {
                let (rows, total) = project_to_rows(&project);
                self.rows = rows;
                self.total_duration_s = total;
                self.error = None;
            }
            Err(e) => {
                // Don't clear `rows` — stale-data is more useful than
                // a blank pane when the read transiently fails.
                self.error = Some(format!("read error: {e}"));
            }
        }
    }

    pub fn clip_count(&self) -> usize {
        self.rows.len()
    }

    pub fn total_duration_s(&self) -> f64 {
        self.total_duration_s
    }

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
                        .montage
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

#[doc(hidden)]
pub fn _new_for_test(root: &Path) -> Timeline {
    Timeline::new(root)
}
