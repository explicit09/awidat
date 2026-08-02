//! Generated-media disclosure inspection.
//!
//! This module walks the current timeline against its project-local
//! generated-media registry, collects provenance credits, and exposes a
//! serializable [`AiDisclosure`] for the desktop warning surface.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use montage_core::generated_media::registry::{
    GeneratedMediaRecord, REGISTRY_RELATIVE_PATH, Registry,
};
use montage_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use serde::{Deserialize, Serialize};

/// One generated-media credit pulled out of the cut.
///
/// Mirrors the fields the platforms' disclosure surfaces care about
/// (provider, model, prompt) plus the timestamps + asset id we need
/// to address the record from the frontend.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GeneratedMediaCredit {
    /// Provider key — `"runway"`, `"sora"`, `"mock"`, etc.
    pub provider: String,
    /// Provider model id — `None` when the registry record didn't pin
    /// one. Optional because mock / placeholder generators may not
    /// carry a model field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Full prompt the agent / user authored. Stored project-locally
    /// for auditability; surfaced (truncated) in the disclosure banner.
    pub prompt: String,
    /// Unix epoch seconds the asset finished generating. `None` when
    /// the registry record didn't pin a completed_at (job is in flight
    /// or got dropped before the timeline reference resolved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<i64>,
    /// Asset id (registry `job_id`) — addressable from the frontend
    /// for deep-links into the Brief / Media tab if we want that later.
    pub asset_id: String,
}

/// Full AI-disclosure payload for one render job.
///
/// `has_synthetic_content` is the warning gate. `credits` is the
/// human-facing provider/prompt/time audit trail.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AiDisclosure {
    /// `true` iff the cut contains at least one generated-media clip.
    /// Kept explicit rather than inferred from credits so callers can
    /// represent synthetic content whose provenance is unavailable.
    pub has_synthetic_content: bool,
    /// Credits for every generated clip that landed in the cut.
    /// Sorted by `generated_at` descending so the most recent appears
    /// first in the UI.
    #[serde(default)]
    pub credits: Vec<GeneratedMediaCredit>,
}

impl AiDisclosure {
    /// Empty disclosure — no synthetic content, no credits. Used as
    /// the default for clean cuts and missing projects.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a disclosure from a credits list. Sets
    /// `has_synthetic_content` iff the list is non-empty.
    pub fn from_credits(credits: Vec<GeneratedMediaCredit>) -> Self {
        Self {
            has_synthetic_content: !credits.is_empty(),
            credits,
        }
    }
}

/// Walk a [`Timeline`] and collect the [`GeneratedMediaCredit`] for
/// every clip whose `media_reference.target_url` points at a generated
/// asset in the project's generated-media registry.
///
/// Two sources of truth are joined here:
///
/// 1. The OTIO timeline's clips — each `ExternalReference` carries a
///    project-relative path. Generated assets always live under
///    `raw/generated/`, so a fast prefix check rejects irrelevant
///    clips before we touch the registry.
/// 2. The registry at `<project>/.montage/generated-media/registry.json`
///    — each record carries the provider / model / prompt the
///    disclosure surface needs.
///
/// We dedupe by asset id so a clip used twice (the user pasted the
/// same generated cutaway over two beats) only contributes one credit
/// — the platforms care about "is *any* generated content present",
/// not the per-use count.
///
/// Missing / unreadable registry collapses to an empty list rather
/// than an error: the disclosure surface is non-load-bearing chrome
/// at the moment, and a missing registry should never block an upload.
pub fn cut_contains_generated_media(
    timeline: &Timeline,
    project_path: &Path,
) -> Vec<GeneratedMediaCredit> {
    let registry = match Registry::load_or_default(project_path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // Empty registry → no possible matches; skip the timeline walk.
    if registry.records.is_empty() {
        return Vec::new();
    }

    let mut credits = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in iter_external_clip_paths(timeline) {
        if !is_generated_path(&path) {
            continue;
        }
        if let Some(record) = find_record_by_output_path(&registry, &path) {
            if seen.insert(record.job_id.clone()) {
                credits.push(credit_from_record(record));
            }
        }
    }

    // Stable order: most-recently-generated first so the UI banner
    // surfaces the newest provenance at the top.
    credits.sort_by(|a, b| {
        b.generated_at
            .unwrap_or(i64::MIN)
            .cmp(&a.generated_at.unwrap_or(i64::MIN))
    });
    credits
}

/// Iterate every `ExternalReference.target_url` on every active clip
/// of the timeline. Inactive clips (`active: false`) are skipped — the
/// playback engine doesn't render them, so they shouldn't pull an AI
/// disclosure either.
fn iter_external_clip_paths(timeline: &Timeline) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for child in &timeline.tracks.children {
        visit_stack_child(child, &mut out);
    }
    out
}

fn visit_stack_child(child: &StackChild, out: &mut Vec<PathBuf>) {
    match child {
        StackChild::Track(track) => {
            for tc in &track.children {
                visit_track_child(tc, out);
            }
        }
        StackChild::Stack(stack) => {
            for c in &stack.children {
                visit_stack_child(c, out);
            }
        }
        StackChild::Clip(clip) => collect_clip(clip, out),
        StackChild::Gap(_) => {}
    }
}

fn visit_track_child(child: &TrackChild, out: &mut Vec<PathBuf>) {
    match child {
        TrackChild::Clip(clip) => collect_clip(clip, out),
        TrackChild::Stack(stack) => {
            for c in &stack.children {
                visit_stack_child(c, out);
            }
        }
        TrackChild::Gap(_) | TrackChild::Transition(_) => {}
    }
}

fn collect_clip(clip: &montage_proto::otio::Clip, out: &mut Vec<PathBuf>) {
    if !clip.active {
        return;
    }
    if let MediaReference::External(ext) = &clip.media_reference {
        out.push(PathBuf::from(&ext.target_url));
    }
}

/// `true` for project-relative paths under `raw/generated/`. The
/// generated-media registry rejects output paths outside this prefix
/// (`crates/core/src/generated_media/registry.rs`'s
/// `validate_generated_output_path`), so this matches the inverse
/// invariant.
///
/// Accepts both forward-slash (`raw/generated/...`) and backslash
/// (`raw\generated\...`) prefixes — Windows-authored OTIO files store
/// paths with backslashes that `Path::components` on Unix treats as
/// a single component, so we sniff the string form alongside the
/// component walk.
fn is_generated_path(path: &Path) -> bool {
    let mut comps = path.components();
    let by_components = matches!(
        (comps.next(), comps.next()),
        (Some(Component::Normal(raw)), Some(Component::Normal(generated)))
            if raw == "raw" && generated == "generated"
    );
    if by_components {
        return true;
    }
    // Fallback: a Windows-style backslash path on a Unix host reads
    // as one Component; check the normalised string form too.
    let normalised = normalize_path(path);
    normalised.starts_with("raw/generated/")
}

/// Find the registry record whose `output_paths["video"]` matches the
/// clip's path. Both sides are normalised to forward-slash strings so
/// a Windows-authored project file with backslash paths still matches.
fn find_record_by_output_path<'a>(
    registry: &'a Registry,
    path: &Path,
) -> Option<&'a GeneratedMediaRecord> {
    let needle = normalize_path(path);
    registry
        .records
        .iter()
        .find(|r| r.output_video_path().map(normalize_str) == Some(needle.clone()))
}

fn normalize_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn normalize_str(s: &str) -> String {
    s.replace('\\', "/")
}

fn credit_from_record(record: &GeneratedMediaRecord) -> GeneratedMediaCredit {
    GeneratedMediaCredit {
        provider: record.provider.clone(),
        model: if record.model.is_empty() {
            None
        } else {
            Some(record.model.clone())
        },
        prompt: record.prompt.clone(),
        generated_at: record.completed_at.map(|t| t.timestamp()),
        asset_id: record.job_id.clone(),
    }
}

/// Convenience helper for callers that have a project root path and
/// want the disclosure in one shot. Reads the project's OTIO timeline,
/// walks it, and joins against the generated-media registry. Missing
/// project (no `project.otio.json` on disk) collapses to an empty
/// disclosure — same "non-load-bearing" rule used downstream.
pub fn disclosure_for_project_root(project_root: &Path) -> AiDisclosure {
    let otio_path = project_root.join("project.otio.json");
    if !otio_path.is_file() {
        return AiDisclosure::empty();
    }
    // We don't go through `Project::read` because that validates +
    // reads the edit plan + manifest — heavier than necessary for
    // disclosure. A direct timeline load is enough.
    let raw = match std::fs::read_to_string(&otio_path) {
        Ok(s) => s,
        Err(_) => return AiDisclosure::empty(),
    };
    let timeline: Timeline = match montage_proto::serde_robust::from_json_str(&raw) {
        Ok(t) => t,
        Err(_) => return AiDisclosure::empty(),
    };
    AiDisclosure::from_credits(cut_contains_generated_media(&timeline, project_root))
}

/// Same as [`disclosure_for_project_root`] but borrowing the in-memory
/// [`Timeline`] — the callers that already have one parsed (e.g. the
/// dispatcher mid-render) skip the disk round-trip.
#[allow(dead_code)]
pub fn disclosure_for_timeline(timeline: &Timeline, project_root: &Path) -> AiDisclosure {
    AiDisclosure::from_credits(cut_contains_generated_media(timeline, project_root))
}

/// Relative path under the project root where the registry lives.
/// Re-exported so external callers can compose the absolute path
/// without depending on `montage-core` directly.
#[allow(dead_code)]
pub fn registry_relative_path() -> &'static str {
    REGISTRY_RELATIVE_PATH
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, Stack, StackChild, Timeline, TimelineMetadata,
        Track, TrackChild, TrackKind,
    };
    use std::collections::HashMap;

    fn clip_with_path(name: &str, path: &str) -> Clip {
        Clip {
            name: name.into(),
            source_range: None,
            media_reference: MediaReference::External(ExternalReference::new(path.to_string())),
            effects: Vec::new(),
            markers: Vec::new(),
            active: true,
            metadata: Default::default(),
        }
    }

    fn timeline_with_paths(paths: &[&str]) -> Timeline {
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, p) in paths.iter().enumerate() {
            track
                .children
                .push(TrackChild::Clip(clip_with_path(&format!("c{i}"), p)));
        }
        let mut stack = Stack::empty("tracks");
        stack.children.push(StackChild::Track(track));
        Timeline {
            otio_schema: "Timeline.1".into(),
            name: "t".into(),
            global_start_time: None,
            tracks: stack,
            metadata: TimelineMetadata::default(),
        }
    }

    fn seed_registry(root: &Path, records: Vec<GeneratedMediaRecord>) {
        let registry = Registry { records };
        registry.save(root).expect("seed registry");
    }

    fn record(job_id: &str, output_path: &str, prompt: &str) -> GeneratedMediaRecord {
        GeneratedMediaRecord::new_mock_succeeded(job_id, output_path, prompt).expect("mock record")
    }

    #[test]
    fn empty_timeline_has_empty_disclosure() {
        let tmp = tempfile::tempdir().unwrap();
        let timeline = timeline_with_paths(&[]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert!(credits.is_empty());
        assert!(!AiDisclosure::from_credits(credits).has_synthetic_content);
    }

    #[test]
    fn timeline_with_only_raw_clips_has_empty_disclosure() {
        let tmp = tempfile::tempdir().unwrap();
        // Seed a registry entry but the timeline doesn't reference it
        // → no disclosure.
        seed_registry(
            tmp.path(),
            vec![record(
                "job-1",
                "raw/generated/runway/abc.mp4",
                "a quiet street",
            )],
        );
        let timeline = timeline_with_paths(&["raw/ep-014-cam-a.mp4"]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert!(credits.is_empty());
    }

    #[test]
    fn timeline_with_generated_clip_emits_credit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = "raw/generated/runway/abc.mp4";
        seed_registry(
            tmp.path(),
            vec![record("job-1", path, "a quiet street at dusk")],
        );
        let timeline = timeline_with_paths(&["raw/ep-014-cam-a.mp4", path]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert_eq!(credits.len(), 1);
        assert_eq!(credits[0].asset_id, "job-1");
        assert_eq!(credits[0].provider, "mock");
        assert_eq!(credits[0].prompt, "a quiet street at dusk");
        assert!(credits[0].generated_at.is_some());
        let disclosure = AiDisclosure::from_credits(credits);
        assert!(disclosure.has_synthetic_content);
    }

    #[test]
    fn duplicate_clip_usage_dedupes_to_one_credit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = "raw/generated/runway/abc.mp4";
        seed_registry(tmp.path(), vec![record("job-1", path, "x")]);
        // Same generated clip referenced twice — disclosure carries one credit.
        let timeline = timeline_with_paths(&[path, path]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert_eq!(credits.len(), 1);
    }

    #[test]
    fn inactive_clip_is_not_disclosed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = "raw/generated/runway/abc.mp4";
        seed_registry(tmp.path(), vec![record("job-1", path, "x")]);
        let mut clip = clip_with_path("c", path);
        clip.active = false;
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut stack = Stack::empty("tracks");
        stack.children.push(StackChild::Track(track));
        let timeline = Timeline {
            otio_schema: "Timeline.1".into(),
            name: "t".into(),
            global_start_time: None,
            tracks: stack,
            metadata: TimelineMetadata::default(),
        };
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert!(
            credits.is_empty(),
            "inactive generated clip must not disclose"
        );
    }

    #[test]
    fn missing_registry_collapses_to_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let timeline = timeline_with_paths(&["raw/generated/runway/abc.mp4"]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        // Registry doesn't exist → empty result.
        assert!(credits.is_empty());
    }

    #[test]
    fn credits_sorted_by_generated_at_desc() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r1 = record("job-old", "raw/generated/a/1.mp4", "old prompt");
        let mut r2 = record("job-new", "raw/generated/a/2.mp4", "new prompt");
        r1.completed_at = Some(chrono::DateTime::from_timestamp(1_000, 0).unwrap());
        r2.completed_at = Some(chrono::DateTime::from_timestamp(2_000, 0).unwrap());
        seed_registry(tmp.path(), vec![r1, r2]);
        let timeline = timeline_with_paths(&["raw/generated/a/1.mp4", "raw/generated/a/2.mp4"]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert_eq!(credits.len(), 2);
        assert_eq!(credits[0].asset_id, "job-new", "newest first");
        assert_eq!(credits[1].asset_id, "job-old");
    }

    #[test]
    fn nested_stack_clips_walked() {
        let tmp = tempfile::tempdir().unwrap();
        let path = "raw/generated/runway/nested.mp4";
        seed_registry(tmp.path(), vec![record("job-nested", path, "nested")]);
        // Nested stack containing a clip.
        let mut inner_track = Track::empty("V2", TrackKind::Video);
        inner_track
            .children
            .push(TrackChild::Clip(clip_with_path("c", path)));
        let mut nested_stack = Stack::empty("nested");
        nested_stack.children.push(StackChild::Track(inner_track));
        let mut outer = Stack::empty("tracks");
        outer.children.push(StackChild::Stack(nested_stack));
        let timeline = Timeline {
            otio_schema: "Timeline.1".into(),
            name: "t".into(),
            global_start_time: None,
            tracks: outer,
            metadata: TimelineMetadata::default(),
        };
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert_eq!(credits.len(), 1);
        assert_eq!(credits[0].asset_id, "job-nested");
    }

    #[test]
    fn windows_style_path_in_clip_matches_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let unix_path = "raw/generated/runway/win.mp4";
        seed_registry(tmp.path(), vec![record("job-win", unix_path, "x")]);
        // OTIO authored on Windows uses backslashes — confirm we still match.
        let timeline = timeline_with_paths(&[r"raw\generated\runway\win.mp4"]);
        let credits = cut_contains_generated_media(&timeline, tmp.path());
        assert_eq!(credits.len(), 1, "windows path mismatch");
        let _ = HashMap::<String, String>::new(); // silence unused import
    }

    #[test]
    fn ai_disclosure_empty_is_clean() {
        let disclosure = AiDisclosure::empty();
        assert!(!disclosure.has_synthetic_content);
        assert!(disclosure.credits.is_empty());
    }
}
