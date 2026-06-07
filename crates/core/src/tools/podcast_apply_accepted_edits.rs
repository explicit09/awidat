//! `podcast_apply_accepted_edits` tool — compile accepted proposal IDs to EDL.
//!
//! This tool is the deterministic bridge from proposal review to mutation. It
//! does not write the timeline itself; it validates accepted proposal IDs,
//! enforces quality evidence for review/risky items, orders cuts end-to-start,
//! and returns the exact `apply_edl` call the agent must run next.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use montage_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Compile accepted podcast proposal IDs into one `apply_edl` batch.
pub struct PodcastApplyAcceptedEditsTool;

#[derive(Debug, Deserialize)]
struct Args {
    accepted_ids: Vec<String>,
    #[serde(default)]
    quality_evidence: Vec<QualityEvidence>,
}

#[derive(Debug, Deserialize)]
struct QualityEvidence {
    item_id: String,
    tool: String,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    kind: &'static str,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    risk: &'static str,
    requires_quality: bool,
    evidence: String,
}

#[derive(Debug, Clone)]
struct ClipTarget {
    asset_id: String,
    anchor: String,
    source_start_s: f64,
    source_end_s: f64,
}

#[async_trait]
impl ToolHandler for PodcastApplyAcceptedEditsTool {
    fn name(&self) -> &'static str {
        "podcast_apply_accepted_edits"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description:
                "Compile accepted podcast proposal item IDs into one ordered apply_edl batch."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "accepted_ids": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Proposal item IDs accepted by the user/agent."
                    },
                    "quality_evidence": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "item_id": {"type": "string"},
                                "tool": {"type": "string"},
                                "summary": {"type": "string"}
                            },
                            "required": ["item_id", "tool"]
                        },
                        "description": "Required for review/risky items: proof that assess_edit_quality was run."
                    }
                },
                "required": ["accepted_ids"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_apply_accepted_edits: invalid args ({e}). Required: accepted_ids."
            ))
        })?;
        if args.accepted_ids.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "podcast_apply_accepted_edits: accepted_ids must contain at least one proposal item id.".into(),
            ));
        }

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_apply_accepted_edits: failed to read project: {e}"
            ))
        })?;
        let candidates = collect_candidates(&ctx.project_root, &project.timeline);
        let by_id: HashMap<_, _> = candidates
            .into_iter()
            .map(|candidate| (candidate.id.clone(), candidate))
            .collect();

        let accepted: Vec<_> = args
            .accepted_ids
            .iter()
            .map(|id| {
                by_id.get(id).cloned().ok_or_else(|| {
                    FunctionCallError::RespondToModel(format!(
                        "podcast_apply_accepted_edits: unknown proposal item id {id:?}. Re-run podcast_edit_proposal and pass an id it returned."
                    ))
                })
            })
            .collect::<Result<_, _>>()?;

        let quality_ids: HashSet<_> = args
            .quality_evidence
            .iter()
            .filter(|evidence| evidence.tool == "assess_edit_quality")
            .map(|evidence| evidence.item_id.as_str())
            .collect();
        let missing_quality: Vec<_> = accepted
            .iter()
            .filter(|candidate| {
                candidate.requires_quality && !quality_ids.contains(candidate.id.as_str())
            })
            .map(|candidate| candidate.id.as_str())
            .collect();
        if !missing_quality.is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "podcast_apply_accepted_edits: proposal item(s) {} require assess_edit_quality evidence before batching.",
                missing_quality.join(", ")
            )));
        }

        let clip_targets = collect_clip_targets(&project.timeline);
        let mut accepted = accepted;
        accepted.sort_by(|a, b| {
            b.timeline_start_s
                .partial_cmp(&a.timeline_start_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut edl = String::from("*** Begin EDL\n");
        let mut batched = Vec::new();
        for candidate in &accepted {
            let target = find_clip_target(&clip_targets, candidate).ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "podcast_apply_accepted_edits: could not map proposal item {} to a current timeline clip. Re-run view_timeline and podcast_edit_proposal.",
                    candidate.id
                ))
            })?;
            let right_anchor = format!("{}-b", target.anchor);
            edl.push_str("*** Split Clip\n");
            edl.push_str(&format!("@@ anchor: clip_uuid={}\n", target.anchor));
            edl.push_str(&format!("+ at_s: {:.3}\n", candidate.source_start_s));
            edl.push_str("*** Split Clip\n");
            edl.push_str(&format!("@@ anchor: clip_uuid={right_anchor}\n"));
            edl.push_str(&format!("+ at_s: {:.3}\n", candidate.source_end_s));
            edl.push_str("*** Ripple Delete\n");
            edl.push_str(&format!("@@ anchor: clip_uuid={right_anchor}\n"));
            batched.push(serde_json::json!({
                "id": candidate.id,
                "kind": candidate.kind,
                "risk": candidate.risk,
                "asset_id": candidate.asset_id,
                "source_start_s": candidate.source_start_s,
                "source_end_s": candidate.source_end_s,
                "timeline_start_s": candidate.timeline_start_s,
                "timeline_end_s": candidate.timeline_end_s,
                "anchor": target.anchor,
                "evidence": candidate.evidence,
            }));
        }
        edl.push_str("*** End EDL\n");

        let quality_evidence: Vec<_> = args
            .quality_evidence
            .iter()
            .map(|evidence| {
                serde_json::json!({
                    "item_id": evidence.item_id,
                    "tool": evidence.tool,
                    "summary": evidence.summary,
                })
            })
            .collect();
        let body = serde_json::json!({
            "status": "ready_to_apply",
            "summary_for_agent": format!(
                "Prepared {} accepted podcast edit(s) as one end-to-start apply_edl batch. Run apply_edl next, then view_timeline, vedit_diff, podcast_smooth_cut_boundaries, and podcast_post_draft_check.",
                accepted.len()
            ),
            "batched_edits": batched,
            "quality_evidence": quality_evidence,
            "apply_edl_tool_call": {
                "name": "apply_edl",
                "args": {
                    "edl": edl,
                    "dry_run": false,
                    "reasoning": "Applying user-approved podcast proposal items as a controlled batch; follow with view_timeline, vedit_diff, podcast_smooth_cut_boundaries, and podcast_post_draft_check."
                }
            },
            "smoothing_tool_call": {
                "name": "podcast_smooth_cut_boundaries",
                "args": {
                    "applied_edits": batched
                }
            },
            "required_follow_up_tools": [
                "apply_edl",
                "view_timeline",
                "vedit_diff",
                "podcast_smooth_cut_boundaries",
                "podcast_post_draft_check"
            ],
        });
        serde_json::to_string(&body)
            .map(ToolOutput::text)
            .map_err(|e| {
                FunctionCallError::Fatal(format!("podcast_apply_accepted_edits serialize: {e}"))
            })
    }
}

fn collect_candidates(project_root: &std::path::Path, timeline: &Timeline) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (index, finding) in
        crate::tools::find_dead_air::scan_dead_air(project_root, timeline, 1.2, 200)
            .into_iter()
            .enumerate()
    {
        let requires_quality = finding.duration_s < 2.0;
        candidates.push(Candidate {
            id: format!("dead-air-{}", index + 1),
            kind: "dead_air",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            risk: if finding.duration_s >= 2.0 {
                "low"
            } else {
                "medium"
            },
            requires_quality,
            evidence: format!(
                "{:.2}s silence; before={:?}; after={:?}",
                finding.duration_s, finding.transcript_before, finding.transcript_after
            ),
        });
    }

    let fillers = crate::transcript_cleanup::default_filler_tokens(false);
    for (index, finding) in
        crate::tools::find_filler_words::scan_filler_words(project_root, timeline, &fillers, 200)
            .into_iter()
            .enumerate()
    {
        candidates.push(Candidate {
            id: format!("filler-{}", index + 1),
            kind: "filler_word",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            risk: "medium",
            requires_quality: true,
            evidence: format!("matched filler token {:?}", finding.text),
        });
    }

    for (index, finding) in
        crate::tools::find_false_starts::scan_false_starts(project_root, timeline, 200)
            .into_iter()
            .enumerate()
    {
        candidates.push(Candidate {
            id: format!("false-start-{}", index + 1),
            kind: "false_start",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            risk: "medium",
            requires_quality: true,
            evidence: format!(
                "restart marker {:?}; snippet={:?}",
                finding.marker, finding.snippet
            ),
        });
    }
    candidates
}

fn collect_clip_targets(timeline: &Timeline) -> Vec<ClipTarget> {
    let mut targets = Vec::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        for child in &track.children {
            let TrackChild::Clip(clip) = child else {
                continue;
            };
            let MediaReference::External(ext) = &clip.media_reference else {
                continue;
            };
            let Some(range) = clip.source_range.as_ref() else {
                continue;
            };
            let source_start_s = range.start_time.to_seconds();
            let source_end_s = source_start_s + range.duration.to_seconds();
            let anchor = clip
                .metadata
                .montage
                .as_ref()
                .and_then(|meta| meta.extra.get("clip_uuid"))
                .and_then(|value| value.as_str())
                .unwrap_or(clip.name.as_str())
                .to_string();
            targets.push(ClipTarget {
                asset_id: ext.target_url.clone(),
                anchor,
                source_start_s,
                source_end_s,
            });
        }
    }
    targets
}

fn find_clip_target<'a>(
    targets: &'a [ClipTarget],
    candidate: &Candidate,
) -> Option<&'a ClipTarget> {
    targets.iter().find(|target| {
        candidate.asset_id == target.asset_id
            && candidate.source_start_s > target.source_start_s
            && candidate.source_end_s < target.source_end_s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange, Timeline, Track, TrackChild, TrackKind,
    };
    use montage_proto::project::Project;
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

    fn make_project(root: &std::path::Path, asset: &str) {
        let mut project = Project::init(root).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(20.0 * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata::default();
        track.children.push(TrackChild::Clip(clip));
        let mut timeline = Timeline::empty("episode");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        project.timeline = timeline;
        project.write(root).unwrap();
    }

    fn write_silences(root: &std::path::Path, asset: &str) {
        let raw = root.join(asset);
        std::fs::create_dir_all(raw.parent().unwrap()).unwrap();
        std::fs::write(&raw, b"fake").unwrap();
        let mut hash: u32 = 0x811c9dc5;
        for byte in raw.to_string_lossy().as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        let stem = raw.file_stem().unwrap().to_string_lossy();
        let path = root
            .join(".montage")
            .join("silences")
            .join(format!("{stem}-{hash:08x}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "ranges": [{"start_s": 3.0, "end_s": 5.5, "db_floor": -45.0}],
                "threshold_db": -40.0,
                "min_duration_s": 0.6
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_whisper(root: &std::path::Path, asset: &str) {
        let path = root
            .join("index")
            .join("whisper")
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "data": {
                    "words": [
                        {"text": "So", "start_s": 1.0, "end_s": 1.2},
                        {"text": "um", "start_s": 1.2, "end_s": 1.4}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn safe_acceptance_returns_ordered_apply_edl_batch_and_followups() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mov";
        make_project(dir.path(), asset);
        write_silences(dir.path(), asset);

        let out = PodcastApplyAcceptedEditsTool
            .handle(
                ToolInvocation {
                    call_id: "a1".into(),
                    name: "podcast_apply_accepted_edits".into(),
                    args: serde_json::json!({"accepted_ids": ["dead-air-1"]}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "ready_to_apply");
        let edl = value["apply_edl_tool_call"]["args"]["edl"]
            .as_str()
            .unwrap();
        assert!(edl.contains("*** Split Clip"));
        assert!(edl.contains("*** Ripple Delete"));
        assert!(edl.contains("@@ anchor: clip_uuid=clip-1-b"));
        assert_eq!(value["required_follow_up_tools"][0], "apply_edl");
        assert_eq!(value["required_follow_up_tools"][1], "view_timeline");
        assert_eq!(value["required_follow_up_tools"][2], "vedit_diff");
        assert_eq!(
            value["required_follow_up_tools"][3],
            "podcast_smooth_cut_boundaries"
        );
        assert_eq!(
            value["required_follow_up_tools"][4],
            "podcast_post_draft_check"
        );
        assert_eq!(
            value["smoothing_tool_call"]["name"],
            "podcast_smooth_cut_boundaries"
        );
    }

    #[tokio::test]
    async fn review_acceptance_requires_quality_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mov";
        make_project(dir.path(), asset);
        write_whisper(dir.path(), asset);

        let err = PodcastApplyAcceptedEditsTool
            .handle(
                ToolInvocation {
                    call_id: "a2".into(),
                    name: "podcast_apply_accepted_edits".into(),
                    args: serde_json::json!({"accepted_ids": ["filler-1"]}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(message.contains("assess_edit_quality"));
                assert!(message.contains("filler-1"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
