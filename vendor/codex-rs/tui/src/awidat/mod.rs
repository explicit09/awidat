//! Awidat-specific TUI surface.
//!
//! Step 6 of the codex-harness migration mounts an Awidat panel into
//! codex's chat layout. The panel auto-mounts when the
//! `AWIDAT_PROJECT_ROOT` env var is set (the same env that drives the
//! Awidat MCP server in `crates/core/src/awidat_mcp/`). When unset,
//! the panel is `None` and codex renders exactly as upstream — this
//! keeps the fork edit invisible to non-Awidat users.
//!
//! The panel stacks two read-only views:
//!   - [`project_insights::ProjectInsights`] at the top (3-4 lines):
//!     a snapshot of which indexers have run.
//!   - [`timeline::Timeline`] filling the rest: a windowed view of
//!     `project.otio.json`.
//!
//! Both views are pure ratatui widgets reading from disk. They
//! refresh via [`AwidatPanel::refresh`], which the App calls when an
//! MCP tool result indicates a destructive Awidat tool succeeded.

pub mod project_insights;
pub mod timeline;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::path::PathBuf;

/// Env var that picks the project root for the Awidat panel and the
/// in-process MCP server. Matches
/// `awidat_core::awidat_mcp::context::PROJECT_ROOT_ENV` — but we
/// copy the literal here so the codex TUI fork stays independent of
/// the awidat-core crate (it depends only on `awidat_proto`).
pub const PROJECT_ROOT_ENV: &str = "AWIDAT_PROJECT_ROOT";

/// Fixed width of the panel column. 36 cols fits track name, clip
/// name, duration, and a short transcript snippet.
pub const PANEL_WIDTH: u16 = 36;

/// Awidat side panel. Owns the two view widgets and the project
/// root resolved at construction.
pub struct AwidatPanel {
    project_root: PathBuf,
    insights: project_insights::ProjectInsights,
    timeline: timeline::Timeline,
}

impl AwidatPanel {
    /// Try to construct an Awidat panel. Returns `None` when
    /// `AWIDAT_PROJECT_ROOT` is unset, so callers can use it as a
    /// "is Awidat active in this session?" gate without raising
    /// errors in vanilla codex sessions.
    pub fn detect() -> Option<Self> {
        let value = std::env::var_os(PROJECT_ROOT_ENV)?;
        let project_root = PathBuf::from(value);
        Some(Self::for_root(project_root))
    }

    /// Build a panel rooted at the given project. Performs the
    /// initial read for both sub-panels; transient read errors
    /// surface inside each view's own banner.
    pub fn for_root(project_root: PathBuf) -> Self {
        let insights = project_insights::ProjectInsights::gather(&project_root);
        let timeline = timeline::Timeline::new(&project_root);
        Self {
            project_root,
            insights,
            timeline,
        }
    }

    /// Re-read disk state. App calls this when a destructive MCP
    /// tool succeeds — apply_edl, manage_assets, vedit_*, etc.
    /// Cheap (single OTIO load + a directory walk).
    pub fn refresh(&mut self) {
        self.insights = project_insights::ProjectInsights::gather(&self.project_root);
        self.timeline.refresh();
    }
}

impl Widget for &AwidatPanel {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Outer bordered frame so the panel reads as a discrete
        // surface next to the chat column.
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        block.render(area, buf);

        // Project header line at the top of the inner area.
        let header = Line::from(vec![
            Span::styled("project ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                project_root_label(&self.project_root),
                Style::default().fg(Color::Cyan),
            ),
        ]);

        // Insights line (compact, width-aware).
        let insights_line = self
            .insights
            .welcome_moments_line_for_width(inner.width)
            .map(|s| Line::from(Span::styled(s, Style::default().fg(Color::DarkGray))));
        let indexers_line = self
            .insights
            .welcome_indexers_line()
            .map(|s| Line::from(Span::styled(s, Style::default().fg(Color::DarkGray))));

        let mut head_lines = vec![header];
        if let Some(line) = indexers_line {
            head_lines.push(line);
        }
        if let Some(line) = insights_line {
            head_lines.push(line);
        }
        let head_height: u16 = u16::try_from(head_lines.len()).unwrap_or(0);

        // Split inner: head lines on top, timeline takes the rest.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(head_height + 1), Constraint::Min(1)])
            .split(inner);

        Paragraph::new(head_lines).render(chunks[0], buf);
        (&self.timeline).render(chunks[1], buf);
    }
}

/// Render only the last two segments of the project path (parent
/// directory + project name) so the header doesn't blow the column
/// width on deep filesystem paths. Falls back to whatever components
/// exist.
fn project_root_label(path: &std::path::Path) -> String {
    let comps: Vec<_> = path.components().collect();
    if comps.len() <= 2 {
        return path.display().to_string();
    }
    let last_two: Vec<_> = comps.iter().rev().take(2).collect();
    format!(
        ".../{}/{}",
        last_two[1].as_os_str().to_string_lossy(),
        last_two[0].as_os_str().to_string_lossy(),
    )
}
