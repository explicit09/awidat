//! Compact, prompt-time summary of the project — what Aider's
//! repomap is for code, ported for video.
//!
//! Aider's bet: "a function called by 20 other functions is more
//! valuable context than a private helper called once." — give the
//! model a *map of the territory* up front; let it ask for files
//! when it needs detail.
//!
//! For video the analogous unit isn't function-vs-private-helper;
//! it's **topics, beats, and editorial state**. The agent asks
//! questions like "find the funniest moment about startups" — and
//! before any tool call, the system prompt should already let it
//! reason about the *shape* of the episode (3 hooks, 1 CTA, dominant
//! topic = startup pitch) without re-fetching the whole transcript.
//!
//! Output is a ~200-300 token text block stuffed into the system
//! prompt at session start. Lives in the tier-1 prompt cache, so
//! cost is paid once per session.

use std::path::Path;

use montage_proto::otio::{Stack, StackChild, Track, TrackChild};
use montage_proto::project::Project;

/// Render the project into a compact textual map.
///
/// Layout:
/// ```text
/// Episode: <project name>
/// Speakers: <count>, dominant: <speaker>
/// Topics:
///   1. [<start>s..<end>s] <label>
///   2. ...
/// Beats: <hook count>, <punchline count>, ... (when editorial-moments index is present)
/// Editorial state: <clip count> on <track count> tracks · <total dur>s
///                  last edit: <description or 'untrimmed'>
/// ```
///
/// Fail-soft on every signal: if topic-mcp / editorial-moments / etc.
/// haven't run, we skip those rows. Producers see what's available
/// instead of a runtime error.
pub fn render(project: &Project, project_root: &Path) -> String {
    let mut out = String::new();

    // Episode title from the project name (whatever Project::read
    // gave us — typically the project directory name).
    out.push_str(&format!("Episode: {}\n", project.timeline.name));

    // Speakers from the first whisper sidecar we find. We only need
    // the count + dominant id; the agent can drill via inspect_clip.
    if let Some((count, dominant)) = speakers_summary(project_root) {
        out.push_str(&format!(
            "Speakers: {count}{}\n",
            dominant
                .map(|s| format!(", dominant: {s}"))
                .unwrap_or_default()
        ));
    }

    // Topic list from topic-mcp sidecars.
    let topics = topics_summary(project_root);
    if !topics.is_empty() {
        out.push_str("Topics:\n");
        for (i, t) in topics.iter().enumerate() {
            out.push_str(&format!(
                "  {}. [{:.1}s..{:.1}s] {}\n",
                i + 1,
                t.start_s,
                t.end_s,
                t.label,
            ));
        }
    }

    // Index coverage: lets the agent know which read_index channels
    // and specialized search tools have data before it plans a cut.
    // Directory existence is deliberately cheap; exact asset coverage
    // is still checked by read_index/tool calls.
    let coverage = index_coverage(project_root);
    if !coverage.available.is_empty() {
        out.push_str(&format!(
            "Index coverage: {}\n",
            coverage.available.join(", ")
        ));
    }
    if !coverage.missing.is_empty() {
        out.push_str(&format!(
            "Missing indexes: {} (call start_indexing only when the task needs them)\n",
            coverage.missing.join(", ")
        ));
    }

    // Editorial state from the OTIO timeline itself: clip count,
    // total duration, and a derived "untrimmed | trimmed" flag.
    let (clip_count, total_dur_s, trimmed_count) = timeline_summary(project);
    let trim_state = if trimmed_count == 0 {
        "untrimmed".to_string()
    } else {
        format!("{trimmed_count} clip(s) trimmed")
    };
    out.push_str(&format!(
        "Editorial state: {clip_count} clip(s) · {total_dur_s:.2}s total · {trim_state}\n"
    ));

    out.trim_end().to_string()
}

struct IndexCoverage {
    available: Vec<&'static str>,
    missing: Vec<&'static str>,
}

fn index_coverage(project_root: &Path) -> IndexCoverage {
    let all = [
        "whisper",
        "topic",
        "editorial-moments",
        "audio-energy",
        "beats",
        "scenedetect",
        "clip",
        "face",
        "gaze",
        "shot",
        "composition",
        "frame-quality",
        "color-analysis",
    ];
    let mut available = Vec::new();
    let mut missing = Vec::new();
    for name in all {
        if project_root.join("index").join(name).is_dir() {
            available.push(name);
        } else {
            missing.push(name);
        }
    }
    IndexCoverage { available, missing }
}

/// Walk index/whisper/* and collect speakers + segments. Returns
/// `(speaker_count, dominant_speaker_id)`. None if no whisper index
/// has run.
fn speakers_summary(project_root: &Path) -> Option<(usize, Option<String>)> {
    let dir = project_root.join("index").join("whisper");
    if !dir.exists() {
        return None;
    }
    let mut speakers: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    let walker = walkdir(&dir);
    for entry in walker {
        let Ok(bytes) = std::fs::read(&entry) else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(arr) = v.pointer("/data/speakers").and_then(|s| s.as_array()) {
            for spk in arr {
                if let Some(id) = spk.get("id").and_then(|i| i.as_str()) {
                    let dur = spk
                        .get("total_speech_s")
                        .and_then(|d| d.as_f64())
                        .unwrap_or(0.0);
                    *speakers.entry(id.to_string()).or_insert(0.0) += dur;
                }
            }
        }
    }
    if speakers.is_empty() {
        return Some((0, None));
    }
    let dominant = speakers
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(id, _)| id.clone());
    Some((speakers.len(), dominant))
}

/// One topic as the map renders it.
struct TopicRow {
    start_s: f64,
    end_s: f64,
    label: String,
}

fn topics_summary(project_root: &Path) -> Vec<TopicRow> {
    let dir = project_root.join("index").join("topic");
    if !dir.exists() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for entry in walkdir(&dir) {
        let Ok(bytes) = std::fs::read(&entry) else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(arr) = v.pointer("/data/topics").and_then(|t| t.as_array()) {
            for t in arr {
                let start_s = t.get("start_s").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let end_s = t.get("end_s").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let label = t
                    .get("label")
                    .and_then(|x| x.as_str())
                    .unwrap_or("untitled")
                    .to_string();
                rows.push(TopicRow {
                    start_s,
                    end_s,
                    label,
                });
            }
        }
    }
    rows.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows
}

/// Returns `(clip_count, total_duration_s, trimmed_count)` from the
/// OTIO timeline. A clip is "trimmed" if its source_range duration is
/// less than the media reference's available_range duration (when
/// declared).
fn timeline_summary(project: &Project) -> (usize, f64, usize) {
    let mut summary = TimelineSummary::default();
    for sc in &project.timeline.tracks.children {
        let StackChild::Track(track) = sc else {
            continue;
        };
        let track_summary = track_summary(track);
        summary.clip_count += track_summary.clip_count;
        summary.trimmed_count += track_summary.trimmed_count;
        summary.duration_s = summary.duration_s.max(track_summary.duration_s);
    }
    (
        summary.clip_count,
        summary.duration_s,
        summary.trimmed_count,
    )
}

#[derive(Clone, Copy, Default)]
struct TimelineSummary {
    clip_count: usize,
    duration_s: f64,
    trimmed_count: usize,
}

impl TimelineSummary {
    fn add_clip(&mut self, clip: &montage_proto::otio::Clip, timeline_end_s: f64) {
        use montage_proto::otio::{ExternalReference, MediaReference};

        self.clip_count += 1;
        self.duration_s = self.duration_s.max(timeline_end_s);
        if let Some(range) = &clip.source_range
            && let MediaReference::External(ExternalReference {
                available_range: Some(available),
                ..
            }) = &clip.media_reference
            && range.duration.to_seconds() + 1e-3 < available.duration.to_seconds()
        {
            self.trimmed_count += 1;
        }
    }

    fn merge_nested(&mut self, nested: TimelineSummary, timeline_start_s: f64) {
        self.clip_count += nested.clip_count;
        self.trimmed_count += nested.trimmed_count;
        self.duration_s = self.duration_s.max(timeline_start_s + nested.duration_s);
    }
}

fn stack_summary(stack: &Stack) -> TimelineSummary {
    let mut summary = TimelineSummary::default();
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                let track_summary = track_summary(track);
                summary.clip_count += track_summary.clip_count;
                summary.trimmed_count += track_summary.trimmed_count;
                summary.duration_s = summary.duration_s.max(track_summary.duration_s);
            }
            StackChild::Stack(stack) => {
                let stack_summary = stack_summary(stack);
                summary.clip_count += stack_summary.clip_count;
                summary.trimmed_count += stack_summary.trimmed_count;
                summary.duration_s = summary.duration_s.max(stack_summary.duration_s);
            }
            StackChild::Clip(clip) => {
                summary.add_clip(clip, clip_duration_s(clip));
            }
            StackChild::Gap(gap) => {
                summary.duration_s = summary
                    .duration_s
                    .max(gap.source_range.duration.to_seconds());
            }
        }
    }
    summary
}

fn track_summary(track: &Track) -> TimelineSummary {
    let mut summary = TimelineSummary::default();
    let mut cursor_s = 0.0;
    let sparse_overlay = is_sparse_overlay_track(track);

    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                let end_s = if sparse_overlay {
                    clip_sparse_timeline_end_s(clip)
                } else {
                    cursor_s + clip_duration_s(clip)
                };
                if !sparse_overlay {
                    cursor_s += clip_duration_s(clip);
                }
                summary.add_clip(clip, end_s);
            }
            TrackChild::Gap(gap) => {
                cursor_s += gap.source_range.duration.to_seconds();
            }
            TrackChild::Transition(_) => {}
            TrackChild::Stack(stack) => {
                let nested = stack_summary(stack);
                cursor_s += nested.duration_s;
                summary.merge_nested(nested, cursor_s - nested.duration_s);
            }
        }
    }

    summary
}

fn is_sparse_overlay_track(track: &Track) -> bool {
    track
        .metadata
        .get("montage_track_role")
        .and_then(|value| value.as_str())
        .is_some_and(|role| matches!(role, "titles" | "captions" | "annotations"))
}

fn clip_sparse_timeline_end_s(clip: &montage_proto::otio::Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.start_time.to_seconds() + range.duration.to_seconds())
        .unwrap_or(0.0)
}

fn clip_duration_s(clip: &montage_proto::otio::Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map(|range| range.duration.to_seconds())
        .unwrap_or(0.0)
}

/// Recursively walk `dir` collecting `*.json` files. Tiny — we don't
/// pull `walkdir` for this; std iterators are enough.
fn walkdir(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ClipMetadata, ExternalReference, Gap, MediaReference, RationalTime, Stack,
        StackChild, TimeRange, Timeline, Track, TrackChild, TrackKind,
    };
    use std::fs;

    fn dummy_project(dir: &Path, clip_dur_s: f64) -> Project {
        let mut tl = Timeline::empty("smoke-test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/clip-1.MOV"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(clip_dur_s * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata::default();
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));
        let mut p = Project::init(dir).unwrap();
        p.timeline = tl;
        p.write(dir).unwrap();
        p
    }

    #[test]
    fn render_with_no_sidecars_shows_minimal_map() {
        let dir = tempfile::tempdir().unwrap();
        let project = dummy_project(dir.path(), 56.47);
        let map = render(&project, dir.path());
        assert!(map.contains("Episode:"));
        assert!(map.contains("1 clip(s)"));
        assert!(map.contains("56.47s total"));
        // No whisper / topic sidecars — those rows must be absent.
        assert!(!map.contains("Speakers:"));
        assert!(!map.contains("Topics:"));
    }

    #[test]
    fn render_counts_episode_duration_as_longest_track_not_sum_of_linked_av() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut timeline = Timeline::empty("linked-av");

        let mut video = Track::empty("V1", TrackKind::Video);
        let mut video_clip = Clip::empty("video".to_string());
        video_clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/interview.mp4"));
        video_clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(180.0 * 24.0, 24.0),
        ));
        video.children.push(TrackChild::Clip(video_clip));

        let mut audio = Track::empty("A1", TrackKind::Audio);
        let mut audio_clip = Clip::empty("audio".to_string());
        audio_clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/interview.mp4"));
        audio_clip.source_range = Some(TimeRange::new(
            RationalTime::zero(48_000.0),
            RationalTime::new(120.0 * 48_000.0, 48_000.0),
        ));
        audio.children.push(TrackChild::Clip(audio_clip));

        timeline.tracks.children.push(StackChild::Track(video));
        timeline.tracks.children.push(StackChild::Track(audio));
        project.timeline = timeline;

        let map = render(&project, dir.path());
        assert!(map.contains("2 clip(s)"));
        assert!(map.contains("180.00s total"), "{map}");
        assert!(!map.contains("300.00s total"), "{map}");
    }

    #[test]
    fn render_ignores_trailing_gaps_when_reporting_episode_duration() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = dummy_project(dir.path(), 10.0);
        let StackChild::Track(track) = &mut project.timeline.tracks.children[0] else {
            panic!("expected video track");
        };
        track
            .children
            .push(TrackChild::Gap(Gap::of_duration(60.0, 24.0)));

        let map = render(&project, dir.path());

        assert!(map.contains("10.00s total"), "{map}");
        assert!(!map.contains("70.00s total"), "{map}");
    }

    #[test]
    fn render_honors_sparse_clip_timeline_starts() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = dummy_project(dir.path(), 10.0);
        let mut title_track = Track::empty("Titles", TrackKind::Video);
        title_track.metadata.insert(
            "montage_track_role".to_string(),
            serde_json::json!("titles"),
        );
        let mut title = Clip::empty("title".to_string());
        title.source_range = Some(TimeRange::new(
            RationalTime::new(50.0 * 24.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        title_track.children.push(TrackChild::Clip(title));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(title_track));

        let map = render(&project, dir.path());

        assert!(map.contains("2 clip(s)"));
        assert!(map.contains("55.00s total"), "{map}");
    }

    #[test]
    fn render_counts_clips_and_trimmed_state_inside_nested_stacks() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut timeline = Timeline::empty("nested");
        let mut nested_track = Track::empty("nested-video", TrackKind::Video);

        for index in 0..3 {
            let mut ext = ExternalReference::new(format!("raw/clip-{index}.mp4"));
            ext.available_range = Some(TimeRange::new(
                RationalTime::zero(24.0),
                RationalTime::new(10.0 * 24.0, 24.0),
            ));
            let mut clip = Clip::empty(format!("clip-{index}"));
            clip.media_reference = MediaReference::External(ext);
            clip.source_range = Some(TimeRange::new(
                RationalTime::zero(24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            nested_track.children.push(TrackChild::Clip(clip));
        }

        let mut outer_track = Track::empty("V1", TrackKind::Video);
        outer_track.children.push(TrackChild::Stack({
            let mut stack = Stack::empty("nested-stack");
            stack.children.push(StackChild::Track(nested_track));
            stack
        }));
        timeline
            .tracks
            .children
            .push(StackChild::Track(outer_track));
        project.timeline = timeline;

        let map = render(&project, dir.path());

        assert!(map.contains("3 clip(s)"));
        assert!(map.contains("15.00s total"), "{map}");
        assert!(map.contains("3 clip(s) trimmed"), "{map}");
    }

    #[test]
    fn render_keeps_overlapping_sparse_overlays_position_based() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = dummy_project(dir.path(), 10.0);
        let mut title_track = Track::empty("Titles", TrackKind::Video);
        title_track.metadata.insert(
            "montage_track_role".to_string(),
            serde_json::json!("titles"),
        );

        for (name, start_s) in [("title-a", 0.0), ("title-b", 5.0)] {
            let mut title = Clip::empty(name.to_string());
            title.source_range = Some(TimeRange::new(
                RationalTime::new(start_s * 24.0, 24.0),
                RationalTime::new(10.0 * 24.0, 24.0),
            ));
            title_track.children.push(TrackChild::Clip(title));
        }

        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(title_track));

        let map = render(&project, dir.path());

        assert!(map.contains("3 clip(s)"));
        assert!(map.contains("15.00s total"), "{map}");
        assert!(!map.contains("20.00s total"), "{map}");
    }

    #[test]
    fn render_preserves_direct_stack_gap_duration() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut timeline = Timeline::empty("direct-stack-gap");
        let mut stack = Stack::empty("nested-stack");
        stack
            .children
            .push(StackChild::Gap(Gap::of_duration(30.0, 24.0)));
        let mut outer_track = Track::empty("V1", TrackKind::Video);
        outer_track.children.push(TrackChild::Stack(stack));
        timeline
            .tracks
            .children
            .push(StackChild::Track(outer_track));
        project.timeline = timeline;

        let map = render(&project, dir.path());

        assert!(map.contains("0 clip(s)"));
        assert!(map.contains("30.00s total"), "{map}");
    }

    #[test]
    fn render_uses_duration_for_direct_stack_clip_source_inpoints() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut timeline = Timeline::empty("direct-stack-clip");
        let mut clip = Clip::empty("trimmed-stack-clip".to_string());
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(60.0 * 24.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        let mut stack = Stack::empty("nested-stack");
        stack.children.push(StackChild::Clip(clip));
        let mut outer_track = Track::empty("V1", TrackKind::Video);
        outer_track.children.push(TrackChild::Stack(stack));
        timeline
            .tracks
            .children
            .push(StackChild::Track(outer_track));
        project.timeline = timeline;

        let map = render(&project, dir.path());

        assert!(map.contains("1 clip(s)"));
        assert!(map.contains("5.00s total"), "{map}");
        assert!(!map.contains("65.00s total"), "{map}");
    }

    #[test]
    fn render_includes_topics_when_topic_sidecars_present() {
        let dir = tempfile::tempdir().unwrap();
        let project = dummy_project(dir.path(), 60.0);

        // Drop a fake topic sidecar.
        let topic_dir = dir.path().join("index/topic/raw");
        fs::create_dir_all(&topic_dir).unwrap();
        let body = serde_json::json!({
            "data": {
                "topics": [
                    {"start_s": 0.0,  "end_s": 17.5, "label": "pitch hook"},
                    {"start_s": 17.5, "end_s": 41.0, "label": "audience expansion"},
                ]
            }
        });
        fs::write(
            topic_dir.join("clip-1.MOV.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();

        let map = render(&project, dir.path());
        assert!(map.contains("Topics:"));
        assert!(map.contains("pitch hook"));
        assert!(map.contains("audience expansion"));
        assert!(map.contains("[0.0s..17.5s]"));
    }

    #[test]
    fn render_includes_speaker_count_when_whisper_present() {
        let dir = tempfile::tempdir().unwrap();
        let project = dummy_project(dir.path(), 60.0);

        let w_dir = dir.path().join("index/whisper/raw");
        fs::create_dir_all(&w_dir).unwrap();
        let body = serde_json::json!({
            "data": {
                "speakers": [
                    {"id": "host", "total_speech_s": 50.0},
                    {"id": "guest", "total_speech_s": 10.0},
                ]
            }
        });
        fs::write(
            w_dir.join("clip-1.MOV.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();

        let map = render(&project, dir.path());
        assert!(map.contains("Speakers: 2"));
        assert!(map.contains("dominant: host"));
    }

    #[test]
    fn render_lists_index_coverage_when_dirs_exist() {
        let dir = tempfile::tempdir().unwrap();
        let project = dummy_project(dir.path(), 60.0);
        // Just create the dirs; the indexer presence is what matters,
        // not the sidecar contents (other tools handle the read).
        for name in ["clip", "face", "shot"] {
            fs::create_dir_all(dir.path().join("index").join(name)).unwrap();
        }
        // gaze + frame-quality dirs deliberately absent.
        let map = render(&project, dir.path());
        assert!(map.contains("Index coverage:"));
        assert!(map.contains("clip"));
        assert!(map.contains("face"));
        assert!(map.contains("shot"));
        assert!(map.contains("Missing indexes:"));
        assert!(map.contains("gaze"));
    }

    #[test]
    fn render_omits_index_coverage_line_when_no_indexers_ran() {
        let dir = tempfile::tempdir().unwrap();
        let project = dummy_project(dir.path(), 60.0);
        let map = render(&project, dir.path());
        assert!(!map.contains("Index coverage:"));
        assert!(map.contains("Missing indexes:"));
    }

    #[test]
    fn render_marks_trimmed_clips_when_available_range_known() {
        let dir = tempfile::tempdir().unwrap();
        // Build a project where the clip's source_range is shorter
        // than the media reference's available_range.
        let mut tl = Timeline::empty("trimmed");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        let mut ext = ExternalReference::new("raw/x.mp4");
        ext.available_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(60.0 * 24.0, 24.0),
        ));
        clip.media_reference = MediaReference::External(ext);
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(30.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        let mut p = Project::init(dir.path()).unwrap();
        p.timeline = tl;
        p.write(dir.path()).unwrap();

        let map = render(&p, dir.path());
        assert!(map.contains("1 clip(s) trimmed"));
    }
}
