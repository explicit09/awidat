//! `podcast_qc_report` tool — pre-render timeline QC gate.

use async_trait::async_trait;
use montage_proto::montage_meta::{
    AudioRelation, CutType, MontageTimelineMetadata, SemanticCutSpec,
};
use montage_proto::otio::{MediaReference, Stack, StackChild, Track, TrackChild};
use montage_proto::project::Project;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Build a podcast timeline QC report before render.
pub struct PodcastQcReportTool;

#[async_trait]
impl ToolHandler for PodcastQcReportTool {
    fn name(&self) -> &'static str {
        "podcast_qc_report"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: "Run pre-render podcast QC for gaps, missing media, captions, audio readiness, and suspicious timeline structure.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_qc_report: failed to read project: {e}"
            ))
        })?;
        let body = build_podcast_qc_report(&ctx.project_root, &project);
        serde_json::to_string(&body)
            .map(ToolOutput::text)
            .map_err(|e| FunctionCallError::Fatal(format!("podcast_qc_report serialize: {e}")))
    }
}

pub(crate) fn is_podcast_project(project: &Project) -> bool {
    project
        .timeline
        .metadata
        .montage
        .as_ref()
        .and_then(|meta| meta.extra.get("montage_project_type"))
        .and_then(|value| value.get("kind"))
        .and_then(|value| value.as_str())
        .is_some_and(|kind| kind == "podcast")
        || project.timeline.name.eq_ignore_ascii_case("podcast")
}

pub(crate) fn build_podcast_qc_report(
    project_root: &std::path::Path,
    project: &Project,
) -> serde_json::Value {
    let mut issues = Vec::new();
    collect_timeline_issues(project_root, &project.timeline.tracks, &mut issues);
    let caption_summary = crate::captions::summarize_captions(project);
    for warning in &caption_summary.warnings {
        issues.push(serde_json::json!({
            "kind": "caption_warning",
            "severity": "warning",
            "message": warning,
        }));
    }
    let audio_finishing = crate::professional::derive_audio_finishing_state(&project.timeline);
    if audio_finishing.meters.is_empty() {
        issues.push(serde_json::json!({
            "kind": "audio_metering_missing",
            "severity": "warning",
            "message": "No audio meter readings found; run podcast_audio_polish before final render."
        }));
    }
    if project
        .timeline
        .metadata
        .montage
        .as_ref()
        .is_none_or(|meta| meta.cut_boundaries.is_empty())
    {
        issues.push(serde_json::json!({
            "kind": "cut_intent_missing",
            "severity": "info",
            "message": "No cut-boundary intent metadata found; suspicious cuts may be harder to audit."
        }));
    }
    let transition_keys = transition_boundary_keys(&project.timeline.tracks);
    let workflow_sections =
        workflow_section_receipt(project.timeline.metadata.montage.as_ref(), &transition_keys);
    collect_workflow_guardrail_issues(
        project.timeline.metadata.montage.as_ref(),
        &transition_keys,
        &mut issues,
    );
    let status = if issues.iter().any(|issue| issue["severity"] == "error") {
        "blocked"
    } else if issues.iter().any(|issue| issue["severity"] == "warning") {
        "needs_review"
    } else {
        "ready"
    };
    serde_json::json!({
        "status": status,
        "summary_for_agent": format!("Podcast QC status: {status}. {} issue(s) before render.", issues.len()),
        "issues": issues,
        "caption_summary": caption_summary,
        "audio_meter_count": audio_finishing.meters.len(),
        "workflow_sections": workflow_sections,
        "required_before_render": true,
    })
}

fn workflow_section_receipt(
    meta: Option<&MontageTimelineMetadata>,
    transition_keys: &std::collections::HashSet<String>,
) -> serde_json::Value {
    let Some(meta) = meta else {
        return serde_json::json!([]);
    };
    let cleanup_cut_count = meta
        .cut_boundaries
        .values()
        .filter(|spec| is_cleanup_cut(spec))
        .count();
    let unresolved_cleanup_cut_count = meta
        .cut_boundaries
        .iter()
        .filter(|(boundary, spec)| {
            is_unverified_cleanup_hard_cut(boundary, spec, transition_keys)
        })
        .count();
    serde_json::json!([
        {
            "id": "brand_overlay",
            "label": "Brand overlay / lower thirds",
            "status": if meta.broadcast_overlay.is_some() { "done" } else { "missing" },
            "evidence_count": usize::from(meta.broadcast_overlay.is_some())
        },
        {
            "id": "reviewable_proposals",
            "label": "Reviewable proposal package",
            "status": if meta.proposal_packages.is_empty() { "missing" } else { "done" },
            "evidence_count": meta.proposal_packages.len()
        },
        {
            "id": "cleanup_cut_smoothing",
            "label": "Cleanup cut smoothing / quality gate",
            "status": if cleanup_cut_count == 0 {
                "not_applicable"
            } else if unresolved_cleanup_cut_count == 0 {
                "done"
            } else {
                "blocked"
            },
            "evidence_count": cleanup_cut_count.saturating_sub(unresolved_cleanup_cut_count),
            "unresolved_count": unresolved_cleanup_cut_count
        },
        {
            "id": "visual_polish",
            "label": "Visual support / polish pass",
            "status": if has_visual_polish_evidence(meta) { "done" } else { "missing" },
            "evidence_count": visual_polish_evidence_count(meta)
        },
        {
            "id": "audio_polish",
            "label": "Audio polish / loudness pass",
            "status": if meta.audio_finishing.is_some() { "done" } else { "missing" },
            "evidence_count": usize::from(meta.audio_finishing.is_some())
        },
        {
            "id": "preflight",
            "label": "Pre-render preflight",
            "status": if meta.preflight_reports.is_empty() { "missing" } else { "done" },
            "evidence_count": meta.preflight_reports.len()
        }
    ])
}

fn collect_workflow_guardrail_issues(
    meta: Option<&MontageTimelineMetadata>,
    transition_keys: &std::collections::HashSet<String>,
    issues: &mut Vec<serde_json::Value>,
) {
    let Some(meta) = meta else {
        return;
    };
    for (boundary, spec) in &meta.cut_boundaries {
        if is_unverified_cleanup_hard_cut(boundary, spec, transition_keys) {
            issues.push(serde_json::json!({
                "kind": "cleanup_hard_cut_smoothing_missing",
                "severity": "error",
                "boundary": boundary,
                "cut_type": "hard_cut",
                "intent": spec.intent,
                "reason": spec.reason,
                "message": "Cleanup/transcript-removal hard cut has no smoothing or assess_edit_quality evidence. Run podcast_smooth_cut_boundaries, assess_edit_quality for this boundary, then recut, Set Audio Lead/Trail, cover with b-roll/cutaway, insert a transition at the boundary, or stamp verified hard-cut evidence."
            }));
        }
    }
}

fn is_unverified_cleanup_hard_cut(
    boundary: &str,
    spec: &SemanticCutSpec,
    transition_keys: &std::collections::HashSet<String>,
) -> bool {
    is_cleanup_cut(spec)
        && spec.cut_type == CutType::HardCut
        && spec.audio_relation == AudioRelation::Sync
        && !has_cut_quality_evidence(spec)
        // A transition physically spanning this clip pair IS the
        // smoothing — the playbook sanctions plan_transition as a
        // repair, so QC must not keep flagging a covered boundary.
        && !transition_keys.contains(boundary)
}

/// Collect `{from}::{to}` boundary keys for every transition that sits
/// between two clips, matching `montage_proto::montage_meta::cut_boundary_key`.
fn transition_boundary_keys(stack: &Stack) -> std::collections::HashSet<String> {
    let mut keys = std::collections::HashSet::new();
    collect_transition_keys_from_stack(stack, &mut keys);
    keys
}

fn collect_transition_keys_from_stack(
    stack: &Stack,
    keys: &mut std::collections::HashSet<String>,
) {
    for stack_child in &stack.children {
        match stack_child {
            StackChild::Track(track) => {
                for (index, child) in track.children.iter().enumerate() {
                    if !matches!(child, TrackChild::Transition(_)) {
                        continue;
                    }
                    let from = track.children[..index].iter().rev().find_map(qc_clip_id);
                    let to = track.children[index + 1..].iter().find_map(qc_clip_id);
                    if let (Some(from), Some(to)) = (from, to) {
                        keys.insert(montage_proto::montage_meta::cut_boundary_key(from, to));
                    }
                }
            }
            StackChild::Stack(inner) => collect_transition_keys_from_stack(inner, keys),
            _ => {}
        }
    }
}

fn qc_clip_id(child: &TrackChild) -> Option<&str> {
    let TrackChild::Clip(clip) = child else {
        return None;
    };
    Some(
        clip.metadata
            .montage
            .as_ref()
            .and_then(|meta| meta.extra.get("clip_uuid"))
            .and_then(|value| value.as_str())
            .unwrap_or(clip.name.as_str()),
    )
}

fn is_cleanup_cut(spec: &SemanticCutSpec) -> bool {
    let haystack = format!(
        "{} {}",
        spec.intent,
        spec.reason.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    [
        "transcript",
        "removal",
        "removed",
        "cleanup",
        "dead air",
        "silence",
        "filler",
        "false start",
        "planning",
        "chatter",
        "aside",
        "production",
        "pause",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn has_cut_quality_evidence(spec: &SemanticCutSpec) -> bool {
    if spec
        .reason
        .as_deref()
        .is_some_and(|reason| text_mentions_quality_evidence(reason))
    {
        return true;
    }
    [
        "assess_edit_quality",
        "quality_evidence",
        "smoothing_checked",
        "smoothing_verified",
        "post_cut_smoothing",
        "podcast_smooth_cut_boundaries",
    ]
    .iter()
    .any(|key| spec.extra.contains_key(*key))
}

fn text_mentions_quality_evidence(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "assess_edit_quality",
        "smoothing checked",
        "smoothing verified",
        "verified hard cut",
        "reviewed hard cut",
        "post-cut smoothing",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn has_visual_polish_evidence(meta: &MontageTimelineMetadata) -> bool {
    visual_polish_evidence_count(meta) > 0
}

fn visual_polish_evidence_count(meta: &MontageTimelineMetadata) -> usize {
    meta.motion_packages.len()
        + meta.motion_scenes.len()
        + meta.composition_graphs.len()
        + meta.parameter_animations.len()
        + meta.timeline_markers.len()
        + usize::from(meta.broll_recommendations.is_some())
}

fn collect_timeline_issues(
    project_root: &std::path::Path,
    stack: &Stack,
    issues: &mut Vec<serde_json::Value>,
) {
    collect_primary_av_duration_issue(stack, issues);
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_issues(project_root, track, issues),
            StackChild::Stack(stack) => collect_timeline_issues(project_root, stack, issues),
            StackChild::Clip(_) | StackChild::Gap(_) => {}
        }
    }
}

fn collect_primary_av_duration_issue(stack: &Stack, issues: &mut Vec<serde_json::Value>) {
    let video = first_track_duration(stack, montage_proto::otio::TrackKind::Video);
    let audio = first_track_duration(stack, montage_proto::otio::TrackKind::Audio);
    let (Some((video_name, video_duration_s)), Some((audio_name, audio_duration_s))) =
        (video, audio)
    else {
        return;
    };
    let drift_s = (video_duration_s - audio_duration_s).abs();
    if drift_s > 0.25 {
        issues.push(serde_json::json!({
            "kind": "primary_av_duration_mismatch",
            "severity": "error",
            "video_track": video_name,
            "audio_track": audio_name,
            "video_duration_s": video_duration_s,
            "audio_duration_s": audio_duration_s,
            "drift_s": drift_s,
            "message": "Primary video and audio track durations differ; linked cleanup likely changed only one side of A/V."
        }));
    }
}

fn first_track_duration(
    stack: &Stack,
    kind: montage_proto::otio::TrackKind,
) -> Option<(String, f64)> {
    for child in &stack.children {
        match child {
            StackChild::Track(track) if track.kind == kind => {
                let duration_s = track.children.iter().map(track_child_duration).sum();
                return Some((track.name.clone(), duration_s));
            }
            StackChild::Stack(stack) => {
                if let Some(found) = first_track_duration(stack, kind) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn track_child_duration(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(clip) => clip
            .source_range
            .as_ref()
            .map(|range| range.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        TrackChild::Transition(transition) => {
            transition.in_offset.to_seconds() + transition.out_offset.to_seconds()
        }
        TrackChild::Stack(stack) => stack
            .children
            .iter()
            .map(|child| match child {
                StackChild::Clip(clip) => clip
                    .source_range
                    .as_ref()
                    .map(|range| range.duration.to_seconds())
                    .unwrap_or(0.0),
                StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
                StackChild::Track(track) => track.children.iter().map(track_child_duration).sum(),
                StackChild::Stack(stack) => {
                    first_track_duration(stack, montage_proto::otio::TrackKind::Video)
                        .map(|(_, duration)| duration)
                        .unwrap_or(0.0)
                }
            })
            .sum(),
    }
}

fn collect_track_issues(
    project_root: &std::path::Path,
    track: &Track,
    issues: &mut Vec<serde_json::Value>,
) {
    let mut cursor_s = 0.0_f64;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                if let MediaReference::External(reference) = &clip.media_reference {
                    let asset_path = project_root.join(&reference.target_url);
                    if !asset_path.exists() {
                        issues.push(serde_json::json!({
                            "kind": "missing_media",
                            "severity": "error",
                            "track": track.name,
                            "clip": clip.name,
                            "asset": reference.target_url,
                            "message": "Timeline references media that does not exist on disk."
                        }));
                    }
                }
                if !clip.active {
                    issues.push(serde_json::json!({
                        "kind": "inactive_clip",
                        "severity": "warning",
                        "track": track.name,
                        "clip": clip.name,
                        "message": "Inactive clip remains on timeline before render."
                    }));
                }
                if let Some(range) = &clip.source_range {
                    cursor_s += range.duration.to_seconds();
                }
            }
            TrackChild::Gap(gap) => {
                let duration_s = gap.source_range.duration.to_seconds();
                if duration_s > 0.5 {
                    issues.push(serde_json::json!({
                        "kind": "timeline_gap",
                        "severity": "warning",
                        "track": track.name,
                        "timeline_start_s": cursor_s,
                        "duration_s": duration_s,
                        "message": "Timeline contains a visible gap longer than 0.5s."
                    }));
                }
                cursor_s += duration_s;
            }
            TrackChild::Transition(transition) => {
                cursor_s += transition.in_offset.to_seconds() + transition.out_offset.to_seconds();
            }
            TrackChild::Stack(stack) => collect_timeline_issues(project_root, stack, issues),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::montage_meta::{AudioRelation, SemanticCutSpec, cut_boundary_key};
    use montage_proto::otio::{
        Clip, ExternalReference, Gap, MediaReference, RationalTime, Stack, StackChild, TimeRange,
        Timeline, Track, TrackChild, TrackKind,
    };
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_project(root: &std::path::Path) {
        let mut project = Project::init(root).unwrap();
        let mut clip = Clip::empty("clip-1");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/missing.mov"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        let mut gap = Gap::of_duration(2.0, 24.0);
        gap.name = "gap".into();
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        track.children.push(TrackChild::Gap(gap));
        let mut timeline = Timeline::empty("podcast");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        project.timeline = timeline;
        project.write(root).unwrap();
    }

    fn write_cleanup_cut_project(root: &std::path::Path, verified: bool) {
        let mut project = Project::init(root).unwrap();
        let asset = root.join("raw/episode.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"fake").unwrap();
        let mut track = Track::empty("Video 1", TrackKind::Video);
        for (id, start_s, duration_s) in [("from", 0.0, 10.0), ("to", 15.0, 10.0)] {
            let mut clip = Clip::empty(id);
            clip.media_reference =
                MediaReference::External(ExternalReference::new("raw/episode.mov"));
            clip.source_range = Some(TimeRange::new(
                RationalTime::new(start_s * 24.0, 24.0),
                RationalTime::new(duration_s * 24.0, 24.0),
            ));
            clip.metadata
                .montage
                .get_or_insert_with(Default::default)
                .extra
                .insert("clip_uuid".into(), serde_json::Value::String(id.into()));
            track.children.push(TrackChild::Clip(clip));
        }
        let mut timeline = Timeline::empty("podcast");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        let meta = timeline
            .metadata
            .montage
            .get_or_insert_with(Default::default);
        let mut spec = SemanticCutSpec {
            cut_type: CutType::HardCut,
            intent: "clean_story_rhythm".into(),
            energy: None,
            audio_relation: AudioRelation::Sync,
            confidence: Some(0.95),
            reason: Some("planning aside removed at a clean boundary".into()),
            extra: Default::default(),
        };
        if verified {
            spec.extra.insert(
                "assess_edit_quality".into(),
                serde_json::json!({"action": "hard_cut"}),
            );
        }
        meta.cut_boundaries
            .insert(cut_boundary_key("from", "to"), spec);
        project.timeline = timeline;
        project.write(root).unwrap();
    }

    fn write_av_mismatch_project(root: &std::path::Path) {
        let mut project = Project::init(root).unwrap();
        let asset = root.join("raw/episode.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"fake").unwrap();
        let mut stack = montage_proto::otio::Stack::empty("root");
        for (name, kind, duration_s) in [
            ("Video 1", TrackKind::Video, 30.0),
            ("A1", TrackKind::Audio, 26.0),
        ] {
            let mut clip = Clip::empty(name);
            clip.media_reference =
                MediaReference::External(ExternalReference::new("raw/episode.mov"));
            clip.source_range = Some(TimeRange::new(
                RationalTime::zero(24.0),
                RationalTime::new(duration_s * 24.0, 24.0),
            ));
            let mut track = Track::empty(name, kind);
            track.children.push(TrackChild::Clip(clip));
            stack.children.push(StackChild::Track(track));
        }
        let mut timeline = Timeline::empty("podcast");
        timeline.tracks = stack;
        project.timeline = timeline;
        project.write(root).unwrap();
    }

    #[tokio::test]
    async fn blocks_missing_media_and_reports_gaps() {
        let dir = tempfile::tempdir().unwrap();
        write_project(dir.path());
        let out = PodcastQcReportTool
            .handle(
                ToolInvocation {
                    call_id: "q1".into(),
                    name: "podcast_qc_report".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "blocked");
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "missing_media")
        );
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "timeline_gap")
        );
    }

    #[tokio::test]
    async fn blocks_primary_av_duration_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        write_av_mismatch_project(dir.path());
        let out = PodcastQcReportTool
            .handle(
                ToolInvocation {
                    call_id: "q2".into(),
                    name: "podcast_qc_report".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "blocked");
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "primary_av_duration_mismatch")
        );
    }

    #[tokio::test]
    async fn blocks_unverified_cleanup_hard_cut() {
        let dir = tempfile::tempdir().unwrap();
        write_cleanup_cut_project(dir.path(), false);
        let out = PodcastQcReportTool
            .handle(
                ToolInvocation {
                    call_id: "q3".into(),
                    name: "podcast_qc_report".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "blocked");
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "cleanup_hard_cut_smoothing_missing")
        );
        assert!(
            value["workflow_sections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|section| {
                    section["id"] == "cleanup_cut_smoothing" && section["status"] == "blocked"
                })
        );
    }

    #[tokio::test]
    async fn transition_at_boundary_satisfies_smoothing_gate() {
        // The agent covered the cleanup boundary with a transition
        // (e.g. montage.motion_blur via Insert Transition) — QC must
        // count that as smoothing instead of staying blocked.
        let dir = tempfile::tempdir().unwrap();
        write_cleanup_cut_project(dir.path(), false);
        let mut project = Project::read(dir.path()).unwrap();
        let StackChild::Track(track) = &mut project.timeline.tracks.children[0] else {
            panic!()
        };
        let transition = montage_proto::otio::Transition {
            name: "motion blur".into(),
            transition_type: "montage.motion_blur".into(),
            in_offset: RationalTime::new(0.09 * 24.0, 24.0),
            out_offset: RationalTime::new(0.09 * 24.0, 24.0),
            metadata: Default::default(),
        };
        track
            .children
            .insert(1, TrackChild::Transition(transition));
        project.write(dir.path()).unwrap();

        let out = PodcastQcReportTool
            .handle(
                ToolInvocation {
                    call_id: "q5".into(),
                    name: "podcast_qc_report".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(
            !value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "cleanup_hard_cut_smoothing_missing"),
            "transition spanning the boundary must satisfy the smoothing gate; got {}",
            value["issues"]
        );
        assert!(
            value["workflow_sections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|section| {
                    section["id"] == "cleanup_cut_smoothing" && section["status"] == "done"
                })
        );
    }

    #[tokio::test]
    async fn verified_cleanup_hard_cut_satisfies_smoothing_gate() {
        let dir = tempfile::tempdir().unwrap();
        write_cleanup_cut_project(dir.path(), true);
        let out = PodcastQcReportTool
            .handle(
                ToolInvocation {
                    call_id: "q4".into(),
                    name: "podcast_qc_report".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert!(
            !value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "cleanup_hard_cut_smoothing_missing")
        );
        assert!(
            value["workflow_sections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|section| {
                    section["id"] == "cleanup_cut_smoothing" && section["status"] == "done"
                })
        );
    }
}
