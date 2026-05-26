//! Project-insights snapshot for the Awidat side panel.
//!
//! Ported in step 6 from `crates/tui/src/project_insights.rs`.
//! Pure directory walk — no awidat-core dependency, no event channel.

use std::path::Path;

/// Snapshot of the project's indexer state, gathered at session
/// start. Cheap to construct; cheap to render.
pub struct ProjectInsights {
    pub vision_indexers: Vec<&'static str>,
    pub editorial_indexers: Vec<&'static str>,
    pub moment_count: Option<usize>,
    pub moment_kinds: Vec<(String, usize)>,
}

impl ProjectInsights {
    pub fn gather(project_root: &Path) -> Self {
        let index_dir = project_root.join("index");
        let exists = |name: &str| index_dir.join(name).is_dir();

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

    pub fn welcome_indexers_line(&self) -> Option<String> {
        if self.vision_indexers.is_empty() && self.editorial_indexers.is_empty() {
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        parts.extend(self.editorial_indexers.iter().copied());
        parts.extend(self.vision_indexers.iter().copied());
        Some(parts.join(" · "))
    }

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
        let verbose = parts.join(" · ");
        if verbose.chars().count() <= max {
            return Some(verbose);
        }
        if remainder > 0 {
            parts.pop();
        }
        let trimmed = parts.join(" · ");
        if trimmed.chars().count() <= max {
            return Some(trimmed);
        }
        while parts.len() > 1 {
            parts.pop();
            let visible_kinds = parts.len() - 1;
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
        Some(total_label)
    }
}

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
