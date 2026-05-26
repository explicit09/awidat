//! Pattern extraction over captured editorial decisions (#150).
//!
//! Distills past editorial decisions into a `learned-style.md` that
//! future sessions splice into their system prompt. Pure histogram math
//! — no LLM in the extraction path. Every learned bullet cites the data
//! ("you accepted 11 of 12 cases like this") so the user can audit and
//! agree or override.
//!
//! **Step 8e/W:** the legacy `rollout::Recorder::collect_decisions`
//! reader was retired with the legacy harness. `extract_from_decisions`
//! and `render_markdown` are still useful — they're pure pattern
//! extraction over the `EditorialDecision` shape — and will be re-wired
//! to a codex-rollout-format reader in a follow-up. The `EditorialDecision`
//! struct moved here from `rollout.rs` so the extraction layer survives
//! independent of the rollout layer.
//!
//! V1 patterns:
//! - per-tool deny rate (which tools the user rejects most often)
//! - per-tool snippet patterns (substrings in `args_summary` whose
//!   accept-rate deviates from the per-tool baseline)
//!
//! Editorial-tag-aware patterns ("hooks with score>0.7 are
//! always accepted") need a structured args summary upstream — V1
//! ships the substring scaffolding so adding richer dimensions later
//! is a tags-not-rewrite change.
//!
//! Why no LLM: trust. A learned style derived from the user's data
//! is only useful if they trust it. "You denied 8 of 9 inserts with
//! score < 0.4" is checkable; an LLM-paraphrased rule isn't.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One captured editorial decision. Stable on disk: extending this
/// struct must be additive (new fields with `#[serde(default)]`), since
/// older session logs may already be on disk when a new awidat reads
/// them.
///
/// Originally lived in `crate::rollout`; moved here in step 8e/W when
/// the legacy harness's rollout layer was deleted. The codex-rollout
/// reader follow-up (#60) will re-populate `Vec<EditorialDecision>` for
/// `extract_from_decisions` from `~/.codex/sessions/` JSONL files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorialDecision {
    /// Tool name (`apply_edl`, `start_render`, `bash`, …).
    pub tool: String,
    /// Same short summary the modal showed the user. Truncated; carries
    /// just enough signal for pattern extraction (e.g. `apply_edl: 3
    /// ops, score=0.72, kind=hook`).
    pub args_summary: String,
    /// Stable editorial dimensions extracted from the tool call. These
    /// are deterministic learning inputs, not prose, so lesson
    /// extraction can learn accepted/rejected cut types, transition
    /// families, split-edit ranges, and b-roll/montage mode without
    /// relying on fragile summary substrings.
    #[serde(default)]
    pub editorial_tags: Vec<String>,
    /// Operation-scoped approval keys considered by the orchestrator.
    /// Used to debug why an AllowForSession did or did not cover a
    /// later request.
    #[serde(default)]
    pub approval_keys: Vec<crate::tool::ApprovalKey>,
    /// Present for an explicit unsandboxed retry prompt after sandbox
    /// denial. Absence means this was the first-attempt approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_reason: Option<String>,
    /// What the user picked. String form for forward-compat: a future
    /// awidat may add new decision variants and we don't want a session
    /// log to be unreadable just because the enum grew.
    pub decision: String,
    /// When the modal was shown.
    pub timestamp: DateTime<Utc>,
}

/// Minimum events for a pattern to make it into the report. Below
/// this, the histogram is too noisy to be a real signal.
pub const MIN_EVENTS: usize = 5;

/// Minimum deviation (percentage points) from baseline for a snippet
/// pattern to be surfaced. 30pp == strong signal even with N=5.
pub const MIN_DEVIATION_PP: f64 = 30.0;

/// Minimum non-trivial substring length for snippet candidates.
/// Shorter than this and we'd surface meaningless 2-3-char fragments.
const MIN_SNIPPET_LEN: usize = 4;

/// Aggregated per-tool stats. Public so the CLI can render its own
/// shape if it wants.
#[derive(Debug, Clone, Default)]
pub struct ToolStats {
    /// Total decisions involving this tool.
    pub total: usize,
    /// Counts by decision string.
    pub by_decision: HashMap<String, usize>,
}

impl ToolStats {
    /// Accept rate (`Allow` + `AllowForSession`) as a fraction in [0,1].
    pub fn accept_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let allowed = self.by_decision.get("Allow").copied().unwrap_or(0)
            + self
                .by_decision
                .get("AllowForSession")
                .copied()
                .unwrap_or(0);
        allowed as f64 / self.total as f64
    }

    /// Deny rate as a fraction in [0,1].
    pub fn deny_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        let denied = self.by_decision.get("Deny").copied().unwrap_or(0);
        denied as f64 / self.total as f64
    }
}

/// One distilled pattern. Each becomes one bullet in `learned-style.md`.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// Tool name the pattern is about (or `*` for cross-tool).
    pub tool: String,
    /// Snippet from `args_summary` the pattern matches (or `None` for
    /// tool-wide patterns).
    pub snippet: Option<String>,
    /// Counts.
    pub allow_count: usize,
    /// Number of denied tool calls matching this pattern.
    pub deny_count: usize,
    /// Human-readable rule the bullet renders.
    pub rule: String,
}

impl Pattern {
    /// Total events in this pattern.
    pub fn total(&self) -> usize {
        self.allow_count + self.deny_count
    }

    /// Accept rate as a fraction in [0,1].
    pub fn accept_rate(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        self.allow_count as f64 / total as f64
    }
}

/// Pure function over a slice of decisions — testable without disk.
/// Until the codex-rollout reader lands (step 8e/W follow-up), there is
/// no on-disk source feeding this in production; callers construct the
/// slice from tests or a future codex-format JSONL parser.
pub fn extract_from_decisions(decisions: &[EditorialDecision]) -> Vec<Pattern> {
    if decisions.is_empty() {
        return Vec::new();
    }

    let mut by_tool: HashMap<String, ToolStats> = HashMap::new();
    for d in decisions {
        let entry = by_tool.entry(d.tool.clone()).or_default();
        entry.total += 1;
        *entry.by_decision.entry(d.decision.clone()).or_insert(0) += 1;
    }

    let mut patterns = Vec::new();

    // Per-tool deny patterns: tools where you deny ≥30% of the time
    // and have at least MIN_EVENTS events.
    for (tool, stats) in &by_tool {
        if stats.total < MIN_EVENTS {
            continue;
        }
        let deny = stats.deny_rate();
        if deny >= 0.30 {
            let allow = stats.by_decision.get("Allow").copied().unwrap_or(0)
                + stats
                    .by_decision
                    .get("AllowForSession")
                    .copied()
                    .unwrap_or(0);
            let denied = stats.by_decision.get("Deny").copied().unwrap_or(0);
            patterns.push(Pattern {
                tool: tool.clone(),
                snippet: None,
                allow_count: allow,
                deny_count: denied,
                rule: format!(
                    "You deny `{tool}` calls {pct:.0}% of the time ({denied} of {total}). \
                     Be more discriminating before calling — review the args, \
                     consider `update_plan` to surface the intent first.",
                    pct = deny * 100.0,
                    total = stats.total,
                ),
            });
        }
    }

    // Per-tool snippet patterns: substrings from args_summary where the
    // accept-rate deviates from the tool's baseline by ≥MIN_DEVIATION_PP.
    for (tool, _stats) in &by_tool {
        let tool_decisions: Vec<&EditorialDecision> =
            decisions.iter().filter(|d| &d.tool == tool).collect();
        if tool_decisions.len() < MIN_EVENTS {
            continue;
        }
        // Build a candidate snippet pool from both the human approval
        // summary and stable editorial tags. Tags carry the meaningful
        // editing dimensions directly, e.g. `cut_type:j_cut`, instead
        // of relying on a truncated EDL summary substring.
        let mut snippet_counts: HashMap<String, (usize, usize)> = HashMap::new(); // (allow, deny)
        for d in &tool_decisions {
            for tok in decision_snippets(d) {
                if tok.len() < MIN_SNIPPET_LEN {
                    continue;
                }
                let entry = snippet_counts.entry(tok).or_insert((0, 0));
                if d.decision == "Allow" || d.decision == "AllowForSession" {
                    entry.0 += 1;
                } else if d.decision == "Deny" {
                    entry.1 += 1;
                }
            }
        }
        let baseline_accept = tool_decisions
            .iter()
            .filter(|d| d.decision == "Allow" || d.decision == "AllowForSession")
            .count() as f64
            / tool_decisions.len() as f64;
        let baseline_pct = baseline_accept * 100.0;
        for (snippet, (allow, deny)) in snippet_counts {
            let total = allow + deny;
            if total < MIN_EVENTS {
                continue;
            }
            let local_accept = allow as f64 / total as f64;
            let local_pct = local_accept * 100.0;
            let dev = local_pct - baseline_pct;
            if dev.abs() < MIN_DEVIATION_PP {
                continue;
            }
            let (direction, direction_count, direction_pct) = if dev > 0.0 {
                ("accept", allow, local_pct)
            } else {
                ("deny", deny, 100.0 - local_pct)
            };
            patterns.push(Pattern {
                tool: tool.clone(),
                snippet: Some(snippet.clone()),
                allow_count: allow,
                deny_count: deny,
                rule: snippet_rule(
                    tool,
                    &snippet,
                    direction,
                    direction_pct,
                    direction_count,
                    total,
                    dev,
                    baseline_pct,
                ),
            });
        }
    }

    // Stable ordering: highest signal first (largest |dev| × N).
    patterns.sort_by(|a, b| {
        let score_a = pattern_score(a);
        let score_b = pattern_score(b);
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    patterns
}

fn pattern_score(p: &Pattern) -> f64 {
    let total = p.total() as f64;
    let rate = p.accept_rate();
    let dev = (rate - 0.5).abs();
    total * dev
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn decision_snippets(decision: &EditorialDecision) -> Vec<String> {
    let mut snippets = tokenize(&decision.args_summary);
    snippets.extend(
        decision
            .editorial_tags
            .iter()
            .map(|tag| normalize_editorial_tag(tag))
            .filter(|tag| !tag.is_empty()),
    );
    snippets.sort();
    snippets.dedup();
    snippets
}

fn normalize_editorial_tag(tag: &str) -> String {
    tag.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn snippet_rule(
    tool: &str,
    snippet: &str,
    direction: &str,
    direction_pct: f64,
    direction_count: usize,
    total: usize,
    dev: f64,
    baseline_pct: f64,
) -> String {
    if let Some(rule) = editorial_tag_rule(
        snippet,
        direction,
        direction_pct,
        direction_count,
        total,
        dev,
        baseline_pct,
        tool,
    ) {
        return rule;
    }
    let subject = if snippet.contains(':') {
        format!("editorial tag `{snippet}`")
    } else {
        format!("`{tool}` args contain `{snippet}`")
    };
    format!(
        "When {subject}, you {direction} {direction_pct:.0}% of the time \
         ({direction_count} of {total}) — {dev:+.0}pp vs your overall \
         {baseline_pct:.0}% accept rate for `{tool}`."
    )
}

#[allow(clippy::too_many_arguments)]
fn editorial_tag_rule(
    snippet: &str,
    direction: &str,
    direction_pct: f64,
    direction_count: usize,
    total: usize,
    dev: f64,
    baseline_pct: f64,
    tool: &str,
) -> Option<String> {
    let (key, value) = snippet.split_once(':')?;
    let setting = match key {
        "format_aspect" => Some(format!(
            "Set Output Format aspect_ratio={}",
            value.replace('x', ":")
        )),
        "format_platform" => Some(format!("Set Output Format platform={value}")),
        "format_safe_area" => Some(format!("Set Output Format safe_area={value}")),
        _ => None,
    }?;
    Some(format!(
        "When {setting}, you {direction} {direction_pct:.0}% of the time \
         ({direction_count} of {total}) — {dev:+.0}pp vs your overall \
         {baseline_pct:.0}% accept rate for `{tool}`."
    ))
}

/// Render the patterns into a markdown body suitable for splicing
/// into the system prompt. Returns `None` when there are no patterns
/// — the caller should not write the file in that case.
pub fn render_markdown(patterns: &[Pattern], total_decisions: usize) -> Option<String> {
    if patterns.is_empty() {
        return None;
    }
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Distilled from {n} captured editorial decisions across past sessions. \
         These are *your* patterns — followed unless you say otherwise this turn.\n",
        n = total_decisions
    );
    for p in patterns {
        let _ = writeln!(s, "- {}", p.rule);
    }
    Some(s)
}

/// Default location for the rendered learned-style file. Lives under
/// the user config dir so it survives `awidat upgrade` and follows
/// XDG conventions. Override with `AWIDAT_LEARNED_STYLE` (used by
/// tests, by sandboxed dev runs, and by users who want the file
/// somewhere other than the default).
pub fn default_output_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AWIDAT_LEARNED_STYLE")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("awidat").join("learned-style.md"))
}

/// Read the learned-style markdown if it exists. Used by `Session::new`
/// to splice into the system prompt. Missing file is `Ok(None)`; a
/// failed read is a `warn!`-and-`Ok(None)` so a corrupted state file
/// doesn't break session bringup.
pub fn read_learned_style(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => Some(s),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "lessons: failed to read learned style");
            None
        }
    }
}

/// Project delivery-format defaults distilled from accepted
/// `Set Output Format` decisions in `learned-style.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LearnedProjectFormatDefaults {
    /// Preferred timeline aspect ratio, such as `9:16`.
    pub aspect_ratio: Option<String>,
    /// Preferred package/export platform, such as `youtube_shorts`.
    pub platform: Option<String>,
    /// Preferred safe-area profile, such as `mobile`.
    pub safe_area: Option<String>,
}

impl LearnedProjectFormatDefaults {
    fn has_required_format(&self) -> bool {
        self.aspect_ratio.is_some()
    }
}

/// Parse accepted project-format defaults from rendered learned-style
/// markdown. Denied rules and unsupported aspect ratios are ignored.
pub fn project_format_defaults_from_markdown(markdown: &str) -> LearnedProjectFormatDefaults {
    let mut defaults = LearnedProjectFormatDefaults::default();
    for line in markdown.lines() {
        if !line.contains("you accept") {
            continue;
        }
        if defaults.aspect_ratio.is_none()
            && let Some(value) = extract_format_value(line, "aspect_ratio")
                .or_else(|| extract_editorial_tag_value(line, "format_aspect"))
                .and_then(normalize_aspect_ratio)
        {
            defaults.aspect_ratio = Some(value);
        }
        if defaults.platform.is_none()
            && let Some(value) = extract_format_value(line, "platform")
                .or_else(|| extract_editorial_tag_value(line, "format_platform"))
                .and_then(nonempty_setting)
        {
            defaults.platform = Some(value);
        }
        if defaults.safe_area.is_none()
            && let Some(value) = extract_format_value(line, "safe_area")
                .or_else(|| extract_editorial_tag_value(line, "format_safe_area"))
                .and_then(nonempty_setting)
        {
            defaults.safe_area = Some(value);
        }
    }
    defaults
}

/// Read the configured learned-style file and return any accepted
/// project-format defaults it contains.
pub fn learned_project_format_defaults() -> LearnedProjectFormatDefaults {
    default_output_path()
        .and_then(|path| read_learned_style(&path))
        .map(|markdown| project_format_defaults_from_markdown(&markdown))
        .unwrap_or_default()
}

/// Apply learned project-format defaults to a project timeline when it
/// does not already carry an explicit `output_format` metadata block.
pub fn apply_learned_project_format_defaults(
    project_root: &Path,
) -> Result<LearnedProjectFormatDefaults, String> {
    let Some(path) = default_output_path() else {
        return Ok(LearnedProjectFormatDefaults::default());
    };
    let Some(markdown) = read_learned_style(&path) else {
        return Ok(LearnedProjectFormatDefaults::default());
    };
    apply_project_format_defaults_from_markdown(project_root, &markdown)
}

/// Testable variant of [`apply_learned_project_format_defaults`] that
/// accepts learned-style markdown directly.
pub fn apply_project_format_defaults_from_markdown(
    project_root: &Path,
    markdown: &str,
) -> Result<LearnedProjectFormatDefaults, String> {
    let defaults = project_format_defaults_from_markdown(markdown);
    if !defaults.has_required_format() {
        return Ok(LearnedProjectFormatDefaults::default());
    }
    let mut project = awidat_proto::project::Project::read(project_root)
        .map_err(|e| format!("read project for learned format defaults: {e}"))?;
    let meta = project
        .timeline
        .metadata
        .awidat
        .get_or_insert_with(awidat_proto::awidat_meta::AwidatTimelineMetadata::default);
    if meta.version.is_empty() {
        meta.version = awidat_proto::AWIDAT_PROJECT_VERSION.to_string();
    }
    if meta.extra.contains_key("output_format") {
        return Ok(LearnedProjectFormatDefaults::default());
    }
    meta.extra.insert(
        "output_format".to_string(),
        serde_json::json!({
            "aspect_ratio": defaults.aspect_ratio.clone(),
            "platform": defaults.platform.clone(),
            "safe_area": defaults.safe_area.clone(),
            "source": "learned_project_format_defaults",
        }),
    );
    project
        .write(project_root)
        .map_err(|e| format!("write learned format defaults: {e}"))?;
    Ok(defaults)
}

fn extract_format_value(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let (_, rest) = line.split_once(&needle)?;
    let value = rest
        .split(|c: char| c == ',' || c == ')' || c.is_whitespace())
        .next()?
        .trim();
    nonempty_setting(value.to_string())
}

fn extract_editorial_tag_value(line: &str, key: &str) -> Option<String> {
    let needle = format!("`{key}:");
    let (_, rest) = line.split_once(&needle)?;
    let (value, _) = rest.split_once('`')?;
    nonempty_setting(value.to_string())
}

fn normalize_aspect_ratio(value: String) -> Option<String> {
    let normalized = value.replace('x', ":");
    match normalized.as_str() {
        "16:9" | "9:16" | "1:1" | "4:5" => Some(normalized),
        _ => None,
    }
}

fn nonempty_setting(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn d(tool: &str, summary: &str, decision: &str) -> EditorialDecision {
        EditorialDecision {
            tool: tool.into(),
            args_summary: summary.into(),
            editorial_tags: vec![],
            approval_keys: vec![],
            retry_reason: None,
            decision: decision.into(),
            timestamp: Utc::now(),
        }
    }

    fn tagged(tool: &str, tag: &str, decision: &str) -> EditorialDecision {
        EditorialDecision {
            tool: tool.into(),
            args_summary: "apply_edl proposal".into(),
            editorial_tags: vec![tag.into()],
            approval_keys: vec![],
            retry_reason: None,
            decision: decision.into(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn no_decisions_yields_no_patterns() {
        assert!(extract_from_decisions(&[]).is_empty());
    }

    #[test]
    fn under_min_events_yields_no_patterns() {
        let decisions = vec![
            d("apply_edl", "kind=hook", "Deny"),
            d("apply_edl", "kind=hook", "Deny"),
        ];
        assert!(extract_from_decisions(&decisions).is_empty());
    }

    #[test]
    fn high_deny_rate_surfaces_tool_pattern() {
        let mut decisions = vec![];
        for _ in 0..5 {
            decisions.push(d("bash", "cmd=git", "Deny"));
        }
        for _ in 0..2 {
            decisions.push(d("bash", "cmd=ls", "Allow"));
        }
        let patterns = extract_from_decisions(&decisions);
        assert!(
            patterns
                .iter()
                .any(|p| p.tool == "bash" && p.snippet.is_none()),
            "expected per-tool deny pattern for bash; got {patterns:#?}"
        );
    }

    #[test]
    fn snippet_patterns_surface_when_above_threshold() {
        // Baseline: apply_edl is mostly accepted.
        let mut decisions = vec![];
        for _ in 0..6 {
            decisions.push(d("apply_edl", "kind=punchline score=0.8", "Allow"));
        }
        // Within apply_edl, kind=tangent is mostly denied — should
        // surface a snippet pattern.
        for _ in 0..6 {
            decisions.push(d("apply_edl", "kind=tangent score=0.3", "Deny"));
        }
        let patterns = extract_from_decisions(&decisions);
        assert!(
            patterns
                .iter()
                .any(|p| p.tool == "apply_edl" && p.snippet.as_deref() == Some("tangent")),
            "expected snippet pattern for tangent; got {patterns:#?}"
        );
    }

    #[test]
    fn editorial_tags_surface_cut_and_transition_preferences() {
        let mut decisions = vec![];
        for _ in 0..6 {
            decisions.push(tagged("apply_edl", "cut_type:jump_cut", "Deny"));
        }
        for _ in 0..6 {
            decisions.push(tagged(
                "apply_edl",
                "transition_family:motion_blur",
                "Allow",
            ));
        }

        let patterns = extract_from_decisions(&decisions);
        assert!(
            patterns.iter().any(|p| {
                p.tool == "apply_edl" && p.snippet.as_deref() == Some("cut_type:jump_cut")
            }),
            "expected cut_type tag preference; got {patterns:#?}"
        );
        assert!(
            patterns.iter().any(|p| {
                p.tool == "apply_edl"
                    && p.snippet.as_deref() == Some("transition_family:motion_blur")
            }),
            "expected transition_family tag preference; got {patterns:#?}"
        );
        let md = render_markdown(&patterns, decisions.len()).unwrap();
        assert!(md.contains("editorial tag `cut_type:jump_cut`"));
    }

    #[test]
    fn format_tags_render_as_output_format_guidance() {
        let mut decisions = vec![];
        for _ in 0..6 {
            decisions.push(tagged("apply_edl", "format_aspect:9x16", "Allow"));
        }
        for _ in 0..6 {
            decisions.push(tagged(
                "apply_edl",
                "format_platform:youtube_shorts",
                "Allow",
            ));
        }
        for _ in 0..6 {
            decisions.push(tagged("apply_edl", "cut_type:jump_cut", "Deny"));
        }

        let patterns = extract_from_decisions(&decisions);
        let md = render_markdown(&patterns, decisions.len()).unwrap();
        assert!(md.contains("Set Output Format"));
        assert!(md.contains("aspect_ratio=9:16"));
        assert!(md.contains("platform=youtube_shorts"));
    }

    #[test]
    fn learned_project_format_defaults_parse_from_accepted_guidance() {
        let markdown = "\
- When Set Output Format aspect_ratio=9:16, you accept 86% of the time (6 of 7).
- When Set Output Format platform=youtube_shorts, you accept 83% of the time (5 of 6).
- When Set Output Format safe_area=mobile, you accept 80% of the time (4 of 5).
- When Set Output Format aspect_ratio=1:1, you deny 80% of the time (4 of 5).
";

        let defaults = project_format_defaults_from_markdown(markdown);

        assert_eq!(defaults.aspect_ratio.as_deref(), Some("9:16"));
        assert_eq!(defaults.platform.as_deref(), Some("youtube_shorts"));
        assert_eq!(defaults.safe_area.as_deref(), Some("mobile"));
    }

    #[test]
    fn learned_project_format_defaults_stamp_fresh_project_without_overriding_existing_format() {
        let markdown = "\
- When Set Output Format aspect_ratio=9:16, you accept 86% of the time (6 of 7).
- When Set Output Format platform=youtube_shorts, you accept 83% of the time (5 of 6).
- When Set Output Format safe_area=mobile, you accept 80% of the time (4 of 5).
";
        let dir = tempfile::tempdir().unwrap();
        awidat_proto::project::Project::init(dir.path()).unwrap();

        let applied = apply_project_format_defaults_from_markdown(dir.path(), markdown).unwrap();

        assert_eq!(applied.aspect_ratio.as_deref(), Some("9:16"));
        let project = awidat_proto::project::Project::read(dir.path()).unwrap();
        let format = project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .unwrap()
            .extra
            .get("output_format")
            .unwrap();
        assert_eq!(
            format.get("aspect_ratio").and_then(|v| v.as_str()),
            Some("9:16")
        );
        assert_eq!(
            format.get("platform").and_then(|v| v.as_str()),
            Some("youtube_shorts")
        );
        assert_eq!(
            format.get("safe_area").and_then(|v| v.as_str()),
            Some("mobile")
        );

        let second = apply_project_format_defaults_from_markdown(
            dir.path(),
            "- When Set Output Format aspect_ratio=1:1, you accept 90% of the time (9 of 10).",
        )
        .unwrap();

        assert_eq!(second, LearnedProjectFormatDefaults::default());
        let project = awidat_proto::project::Project::read(dir.path()).unwrap();
        let format = project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .unwrap()
            .extra
            .get("output_format")
            .unwrap();
        assert_eq!(
            format.get("aspect_ratio").and_then(|v| v.as_str()),
            Some("9:16")
        );
    }

    #[test]
    fn render_markdown_returns_none_for_empty_patterns() {
        assert!(render_markdown(&[], 0).is_none());
    }

    #[test]
    fn render_markdown_includes_decision_count_header() {
        let patterns = vec![Pattern {
            tool: "bash".into(),
            snippet: None,
            allow_count: 1,
            deny_count: 9,
            rule: "test rule".into(),
        }];
        let md = render_markdown(&patterns, 42).unwrap();
        assert!(md.contains("42 captured"));
        assert!(md.contains("test rule"));
    }

    #[test]
    fn read_learned_style_handles_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("does-not-exist.md");
        assert!(read_learned_style(&p).is_none());
    }

    #[test]
    fn read_learned_style_returns_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("style.md");
        std::fs::write(&p, "hello world").unwrap();
        assert_eq!(read_learned_style(&p).as_deref(), Some("hello world"));
    }

    #[test]
    fn read_learned_style_treats_whitespace_only_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("style.md");
        std::fs::write(&p, "   \n\n  ").unwrap();
        assert!(read_learned_style(&p).is_none());
    }
}
