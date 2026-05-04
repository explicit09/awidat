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
        let editorial_indexers: Vec<&'static str> =
            ["whisper", "topic", "editorial-moments"]
                .into_iter()
                .filter(|n| exists(n))
                .collect();

        // Editorial-moments roll-up. Walk every sidecar under
        // index/editorial-moments/ and tally moments + kinds.
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
        // Compact form: just the names joined by `·`. Order is
        // canonical-chain order across both lists. Mostly a status
        // glance, not a deep summary.
        let mut parts: Vec<&str> = Vec::new();
        parts.extend(self.editorial_indexers.iter().copied());
        parts.extend(self.vision_indexers.iter().copied());
        Some(parts.join(" · "))
    }

    /// One-line moments summary like "12 moments · 5 hooks · 3
    /// punchlines · 2 ctas · 2 other". Returns `None` when the
    /// editorial-moments index hasn't run.
    pub fn welcome_moments_line(&self) -> Option<String> {
        let total = self.moment_count?;
        if total == 0 {
            return Some("0 moments".to_string());
        }
        let mut parts = vec![format!("{total} moments")];
        for (kind, n) in self.moment_kinds.iter().take(3) {
            parts.push(format!("{n} {kind}{}", if *n == 1 { "" } else { "s" }));
        }
        // Roll the rest into "other" so the card never grows.
        let listed: usize = self.moment_kinds.iter().take(3).map(|(_, n)| n).sum();
        let remainder = total.saturating_sub(listed);
        if remainder > 0 {
            parts.push(format!("{remainder} other"));
        }
        Some(parts.join(" · "))
    }
}

/// Walk every sidecar under `dir`, sum moment counts, tally by kind.
/// Errors silently — malformed sidecars are surfaced separately by
/// `awidat validate`; the welcome card just shows what it could.
fn count_moments(dir: &Path) -> (Option<usize>, Vec<(String, usize)>) {
    let mut total = 0usize;
    let mut by_kind: std::collections::HashMap<String, usize> = Default::default();
    walk(dir, &mut |path| {
        let Ok(bytes) = std::fs::read(path) else { return };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
        let Some(arr) = v.pointer("/data/moments").and_then(|x| x.as_array()) else { return };
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
    let Ok(entries) = std::fs::read_dir(dir) else { return };
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
        assert_eq!(i.vision_indexers, vec!["clip", "face", "shot", "gaze", "frame-quality"]);
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
            vec![("hook".into(), 2), ("punchline".into(), 2), ("cta".into(), 1)]
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
}
