//! What we know about the project at session-start.
//!
//! Populated once when the App is built; consumed by the welcome card
//! to show the user "here's what's already indexed and ready" — the
//! Cursor moment of "your codebase is mapped, ask anything."
//!
//! All scans here are cheap: directory listings + a small JSON read
//! per indexer that has a sidecar. Total cost on the 44-min Samsung
//! project: a few ms.

use std::path::Path;

/// Snapshot of the project's indexer state, gathered at session
/// start. Cheap to construct; cheap to render.
pub struct ProjectInsights {
    /// Names of vision indexers whose sidecar dirs exist (clip, face,
    /// shot, gaze, frame-quality). Order matches the canonical
    /// chain so the welcome card line reads naturally.
    pub vision_indexers: Vec<&'static str>,
    /// Names of editorial indexers whose sidecar dirs exist
    /// (whisper, topic, editorial-moments). Same canonical order.
    pub editorial_indexers: Vec<&'static str>,
    /// Editorial-moments count, summed across assets, when that
    /// indexer ran. None when the indexer hasn't run yet.
    pub moment_count: Option<usize>,
    /// Editorial-moments breakdown by kind (e.g.
    /// `[("hook", 5), ("punchline", 3)]`) when present. Sorted by
    /// count descending. Capped at 4 entries to fit the card.
    pub moment_kinds: Vec<(String, usize)>,
}

impl ProjectInsights {
    /// Read the project's index/ directory and sidecars. Always
    /// returns something — fresh projects produce an "empty"
    /// snapshot that the welcome card renders as a coachmark.
    pub fn gather(project_root: &Path) -> Self {
        let index_dir = project_root.join("index");
        let exists = |name: &str| index_dir.join(name).is_dir();

        // Order matches the canonical chain for both lists.
        let vision_indexers: Vec<&'static str> = ["clip", "face", "shot", "gaze", "frame-quality"]
            .into_iter()
            .filter(|n| exists(n))
            .collect();
        let editorial_indexers: Vec<&'static str> = ["whisper", "topic", "editorial-moments"]
            .into_iter()
            .filter(|n| exists(n))
            .collect();

        let (moment_count, moment_kinds) = if exists("editorial-moments") {
            count_moments(&index_dir.join("editorial-moments"))
        } else {
            (None, Vec::new())
        };

        Self {
            vision_indexers,
            editorial_indexers,
            moment_count,
            moment_kinds,
        }
    }

    /// Compact human-readable rendering of the indexed-state for
    /// the welcome card. Returns `None` when nothing is indexed —
    /// the card uses that as a signal to render a coachmark
    /// instead.
    pub fn welcome_indexers_line(&self) -> Option<String> {
        if self.vision_indexers.is_empty() && self.editorial_indexers.is_empty() {
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        parts.extend(self.editorial_indexers.iter().copied());
        parts.extend(self.vision_indexers.iter().copied());
        Some(parts.join(" · "))
    }

    /// One-line moments summary like "12 moments · 5 hooks · 3
    /// punchlines · 2 ctas · 2 other". Returns `None` when the
    /// editorial-moments index hasn't run OR has run but produced
    /// zero moments — both states want the coachmark, not a misleading
    /// "0 moments" line that suggests the indexer succeeded with empty
    /// output. Real-world session bug: stale empty `raw/` dir from a
    /// mid-investigation rm of a sidecar produced "0 moments" in the
    /// welcome card even though the live data was 159 moments after
    /// re-indexing.
    pub fn welcome_moments_line(&self) -> Option<String> {
        self.welcome_moments_line_for_width(u16::MAX)
    }

    /// Width-aware moments line. Builds the verbose form first, then
    /// progressively trims if it exceeds `max_width`:
    ///   1. Drop the "N other" suffix.
    ///   2. Drop kind labels one at a time (rightmost first) but
    ///      keep the total count + "more" suffix so the user still
    ///      sees how many moments exist.
    ///   3. Last resort: just "<total> moments".
    /// Each fallback is still informative — the caller never gets
    /// a clipped mid-word string.
    pub fn welcome_moments_line_for_width(&self, max_width: u16) -> Option<String> {
        let total = self.moment_count?;
        if total == 0 {
            return None;
        }
        let max = max_width as usize;
        let total_label = format!("{total} moments");

        let mut parts: Vec<String> = vec![total_label.clone()];
        for (kind, n) in self.moment_kinds.iter().take(3) {
            parts.push(format!("{n} {kind}{}", if *n == 1 { "" } else { "s" }));
        }
        let listed: usize = self.moment_kinds.iter().take(3).map(|(_, n)| n).sum();
        let remainder = total.saturating_sub(listed);
        if remainder > 0 {
            parts.push(format!("{remainder} other"));
        }
        // Try the verbose form; if it fits, ship it.
        let verbose = parts.join(" · ");
        if verbose.chars().count() <= max {
            return Some(verbose);
        }
        // Drop the "N other" suffix first — it's the lowest-info
        // part. Recompute parts without it.
        if remainder > 0 {
            parts.pop();
        }
        let trimmed = parts.join(" · ");
        if trimmed.chars().count() <= max {
            return Some(trimmed);
        }
        // Drop kinds rightmost-first; replace with "+N more" tag so
        // the user still knows there were more kinds. Walk down to
        // the totals-only form if needed.
        while parts.len() > 1 {
            parts.pop();
            let visible_kinds = parts.len() - 1; // -1 for the "N moments" total
            let remaining_kinds = self.moment_kinds.len().saturating_sub(visible_kinds);
            let candidate = if remaining_kinds > 0 {
                format!("{} · +{remaining_kinds} more", parts.join(" · "))
            } else {
                parts.join(" · ")
            };
            if candidate.chars().count() <= max {
                return Some(candidate);
            }
        }
        // Last-resort: just the total.
        Some(total_label)
    }
}

/// Walk every sidecar under `dir`, sum moment counts, tally by kind.
/// Errors silently — malformed sidecars are surfaced separately by
/// `awidat validate`; the welcome card just shows what it could.
fn count_moments(dir: &Path) -> (Option<usize>, Vec<(String, usize)>) {
    let mut total = 0usize;
    let mut by_kind: std::collections::HashMap<String, usize> = Default::default();
    walk(dir, &mut |path| {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return;
        };
        let Some(arr) = v.pointer("/data/moments").and_then(|x| x.as_array()) else {
            return;
        };
        total += arr.len();
        for m in arr {
            if let Some(kind) = m.get("kind").and_then(|x| x.as_str()) {
                *by_kind.entry(kind.to_string()).or_insert(0) += 1;
            }
        }
    });
    let mut kinds: Vec<(String, usize)> = by_kind.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    (Some(total), kinds)
}

fn walk(dir: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, visit);
        } else if path.extension().is_some_and(|e| e == "json") {
            visit(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_json(path: &Path, body: serde_json::Value) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&body).unwrap()).unwrap();
    }

    #[test]
    fn empty_project_produces_empty_insights() {
        let dir = tempfile::tempdir().unwrap();
        let i = ProjectInsights::gather(dir.path());
        assert!(i.vision_indexers.is_empty());
        assert!(i.editorial_indexers.is_empty());
        assert!(i.welcome_indexers_line().is_none());
        assert!(i.welcome_moments_line().is_none());
    }

    #[test]
    fn vision_dirs_listed_in_canonical_order() {
        let dir = tempfile::tempdir().unwrap();
        // Create in REVERSE order to verify the output sorts canonically.
        for name in ["frame-quality", "gaze", "shot", "face", "clip"] {
            std::fs::create_dir_all(dir.path().join("index").join(name)).unwrap();
        }
        let i = ProjectInsights::gather(dir.path());
        assert_eq!(
            i.vision_indexers,
            vec!["clip", "face", "shot", "gaze", "frame-quality"]
        );
    }

    #[test]
    fn welcome_line_combines_editorial_and_vision() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["whisper", "topic", "clip", "face"] {
            std::fs::create_dir_all(dir.path().join("index").join(name)).unwrap();
        }
        let i = ProjectInsights::gather(dir.path());
        let line = i.welcome_indexers_line().unwrap();
        // Editorial first, then vision — both in canonical order.
        assert_eq!(line, "whisper · topic · clip · face");
    }

    #[test]
    fn empty_editorial_moments_dir_returns_none_not_zero_moments() {
        // Real-world session bug: the editorial-moments dir exists
        // (because the indexer was run earlier in the session) but
        // contains no sidecars (because the user rm'd the file
        // mid-investigation, or because the indexer crashed before
        // writing). welcome_moments_line should return None and let
        // the welcome card render the coachmark, NOT "0 moments"
        // which falsely suggests the indexer ran successfully with
        // empty output.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("index/editorial-moments/raw")).unwrap();
        let i = ProjectInsights::gather(dir.path());
        assert_eq!(i.moment_count, Some(0));
        assert!(
            i.welcome_moments_line().is_none(),
            "empty dir should hide the line"
        );
    }

    #[test]
    fn moments_summary_aggregates_across_assets() {
        let dir = tempfile::tempdir().unwrap();
        let em = dir.path().join("index/editorial-moments/raw");
        write_json(
            &em.join("a.json"),
            serde_json::json!({"data": {"moments": [
                {"kind": "hook"}, {"kind": "hook"}, {"kind": "punchline"},
            ]}}),
        );
        write_json(
            &em.join("b.json"),
            serde_json::json!({"data": {"moments": [
                {"kind": "punchline"}, {"kind": "cta"},
            ]}}),
        );
        let i = ProjectInsights::gather(dir.path());
        assert_eq!(i.moment_count, Some(5));
        // Sorted by count desc, then alpha-asc on ties.
        assert_eq!(
            i.moment_kinds,
            vec![
                ("hook".into(), 2),
                ("punchline".into(), 2),
                ("cta".into(), 1)
            ]
        );
        let line = i.welcome_moments_line().unwrap();
        assert!(line.starts_with("5 moments"));
        assert!(line.contains("2 hooks"));
        assert!(line.contains("2 punchlines"));
        assert!(line.contains("1 cta"));
    }

    #[test]
    fn moments_line_caps_at_3_kinds_with_other_remainder() {
        let dir = tempfile::tempdir().unwrap();
        let em = dir.path().join("index/editorial-moments/raw");
        write_json(
            &em.join("a.json"),
            serde_json::json!({"data": {"moments": [
                {"kind": "hook"}, {"kind": "hook"}, {"kind": "hook"},
                {"kind": "punchline"}, {"kind": "punchline"},
                {"kind": "story"},
                {"kind": "tangent"},
                {"kind": "explanation"},
            ]}}),
        );
        let i = ProjectInsights::gather(dir.path());
        let line = i.welcome_moments_line().unwrap();
        // 8 total, top 3 = hook(3) + punchline(2) + ?(1) = 6 listed;
        // remainder 2 = "2 other".
        // The 3rd slot is alphabetically-first of the three 1-counts
        // (explanation, story, tangent) — tiebreak is by name asc.
        assert!(line.contains("8 moments"));
        assert!(line.contains("3 hooks"));
        assert!(line.contains("2 punchlines"));
        assert!(line.contains("1 explanation"));
        assert!(line.contains("2 other"));
        // Story + tangent NOT broken out separately (rolled into "other").
        assert!(!line.contains("story"));
        assert!(!line.contains("tangent"));
    }

    #[test]
    fn width_aware_drops_other_first_then_kinds_rightmost() {
        // Real-world from the 44-min Samsung session: 159 moments
        // distributed across 11 kinds. Verbose form is way too
        // long for a narrow column. Width-aware degrades cleanly.
        let dir = tempfile::tempdir().unwrap();
        let em = dir.path().join("index/editorial-moments/raw");
        // Build distribution mimicking the real session.
        let counts = [
            ("explanation", 41),
            ("hook", 29),
            ("setup", 22),
            ("punchline", 21),
            ("story", 19),
            ("answer", 14),
            ("cta", 4),
            ("question", 4),
            ("emotional_peak", 2),
            ("tangent", 2),
            ("dead_air", 1),
        ];
        let moments: Vec<serde_json::Value> = counts
            .iter()
            .flat_map(|(k, n)| {
                let k = *k;
                (0..*n).map(move |_| serde_json::json!({"kind": k}))
            })
            .collect();
        write_json(
            &em.join("a.json"),
            serde_json::json!({"data": {"moments": moments}}),
        );
        let i = ProjectInsights::gather(dir.path());

        // Plenty of width: full verbose form.
        let big = i.welcome_moments_line_for_width(200).unwrap();
        assert!(big.contains("159 moments"));
        assert!(big.contains("41 explanations"));
        assert!(big.contains("29 hooks"));
        assert!(big.contains("22 setups"));
        assert!(big.contains("other")); // remainder

        // Medium width: should drop "other" suffix first.
        // Verbose form length on this data is ~70 chars; force
        // mid-width that's just below verbose.
        let medium_width = (big.chars().count() - 5) as u16;
        let medium = i.welcome_moments_line_for_width(medium_width).unwrap();
        assert!(medium.contains("41 explanations"));
        assert!(
            !medium.contains("other"),
            "should drop the 'N other' suffix first"
        );

        // Tight width: should drop kinds rightmost-first, replace
        // with "+N more".
        let tight = i.welcome_moments_line_for_width(40).unwrap();
        assert!(tight.contains("159 moments"), "always keep the total");
        assert!(
            tight.contains("more"),
            "should append '+N more' tag when truncating: {tight:?}"
        );
        assert!(
            tight.chars().count() <= 40,
            "tight form should fit within budget; got {} chars",
            tight.chars().count()
        );

        // Last-resort: just the total.
        let minimal = i.welcome_moments_line_for_width(15).unwrap();
        assert_eq!(minimal, "159 moments");
    }

    #[test]
    fn width_aware_with_zero_kinds_still_returns_total() {
        // Edge case: moments without `kind` fields (malformed
        // sidecar, or future schema change). welcome_moments_line
        // should still report the total; degradation never panics.
        let dir = tempfile::tempdir().unwrap();
        let em = dir.path().join("index/editorial-moments/raw");
        write_json(
            &em.join("a.json"),
            serde_json::json!({"data": {"moments": [{}, {}, {}]}}),
        );
        let i = ProjectInsights::gather(dir.path());
        let line = i.welcome_moments_line_for_width(20).unwrap();
        assert!(line.contains("3 moments"));
    }
}
