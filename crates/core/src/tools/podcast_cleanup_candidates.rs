//! `podcast_cleanup_candidates` tool — aggregate existing cleanup evidence.
//!
//! This does not invent a new audio analyzer. It packages the evidence
//! Montage already has — dead air, filler words, and false starts —
//! into safe/review/risky candidate buckets for the podcast workflow.

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Read-only cleanup candidate aggregator.
pub struct PodcastCleanupCandidatesTool;

#[derive(Debug, Deserialize)]
struct PodcastCleanupArgs {
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    dead_air_min_duration_s: Option<f64>,
    #[serde(default)]
    aggressive_fillers: bool,
}

#[derive(Debug, Serialize)]
struct CleanupReport {
    status: &'static str,
    summary_for_agent: String,
    safe_cuts: Vec<CleanupCandidate>,
    review_cuts: Vec<CleanupCandidate>,
    risky_cuts: Vec<CleanupCandidate>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CleanupCandidate {
    kind: &'static str,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    confidence: f64,
    risk: &'static str,
    suggested_action: &'static str,
    evidence: String,
    requires_approval: bool,
}

#[async_trait]
impl ToolHandler for PodcastCleanupCandidatesTool {
    fn name(&self) -> &'static str {
        "podcast_cleanup_candidates"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Maximum candidates per evidence family. Default 40."
                    },
                    "dead_air_min_duration_s": {
                        "type": "number",
                        "minimum": 0.6,
                        "description": "Minimum silence duration to consider. Default 1.2s."
                    },
                    "aggressive_fillers": {
                        "type": "boolean",
                        "description": "Include discourse markers like like / you know / i mean. Default false."
                    }
                }
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
        let args: PodcastCleanupArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_cleanup_candidates: invalid args ({e}). All fields are optional."
            ))
        })?;
        let max_results = args.max_results.unwrap_or(40).clamp(1, 200);
        let dead_air_min_duration_s = args.dead_air_min_duration_s.unwrap_or(1.2).max(0.6);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_cleanup_candidates: failed to read project: {e}"
            ))
        })?;

        let mut safe_cuts = Vec::new();
        let mut review_cuts = Vec::new();
        let risky_cuts = Vec::new();
        let mut missing_evidence = Vec::new();

        let dead_air = crate::tools::find_dead_air::scan_dead_air(
            &ctx.project_root,
            &project.timeline,
            dead_air_min_duration_s,
            max_results,
        );
        if dead_air.is_empty() {
            missing_evidence
                .push("no timeline-visible dead-air findings from silence sidecars".into());
        }
        for finding in dead_air {
            let candidate = CleanupCandidate {
                kind: "dead_air",
                asset_id: finding.asset_id,
                source_start_s: finding.source_start_s,
                source_end_s: finding.source_end_s,
                timeline_start_s: finding.timeline_start_s,
                timeline_end_s: finding.timeline_end_s,
                confidence: if finding.duration_s >= 2.0 { 0.9 } else { 0.72 },
                risk: if finding.duration_s >= 2.0 {
                    "low"
                } else {
                    "medium"
                },
                suggested_action: if finding.duration_s >= 2.0 {
                    "safe_cut_candidate"
                } else {
                    "review_cut_candidate"
                },
                evidence: format!(
                    "{:.2}s silence; before={:?}; after={:?}",
                    finding.duration_s, finding.transcript_before, finding.transcript_after
                ),
                requires_approval: finding.duration_s < 2.0,
            };
            if candidate.requires_approval {
                review_cuts.push(candidate);
            } else {
                safe_cuts.push(candidate);
            }
        }

        let fillers = crate::transcript_cleanup::default_filler_tokens(args.aggressive_fillers);
        let filler_words = crate::tools::find_filler_words::scan_filler_words(
            &ctx.project_root,
            &project.timeline,
            &fillers,
            max_results,
        );
        if filler_words.is_empty() {
            missing_evidence.push("no timeline-visible filler findings from whisper words".into());
        }
        for finding in filler_words {
            review_cuts.push(CleanupCandidate {
                kind: "filler_word",
                asset_id: finding.asset_id,
                source_start_s: finding.source_start_s,
                source_end_s: finding.source_end_s,
                timeline_start_s: finding.timeline_start_s,
                timeline_end_s: finding.timeline_end_s,
                confidence: if args.aggressive_fillers { 0.55 } else { 0.78 },
                risk: "medium",
                suggested_action: "review_cut_candidate",
                evidence: format!("matched filler token {:?}", finding.text),
                requires_approval: true,
            });
        }

        let false_starts = crate::tools::find_false_starts::scan_false_starts(
            &ctx.project_root,
            &project.timeline,
            max_results,
        );
        if false_starts.is_empty() {
            missing_evidence
                .push("no timeline-visible false-start findings from whisper words".into());
        }
        for finding in false_starts {
            review_cuts.push(CleanupCandidate {
                kind: "false_start",
                asset_id: finding.asset_id,
                source_start_s: finding.source_start_s,
                source_end_s: finding.source_end_s,
                timeline_start_s: finding.timeline_start_s,
                timeline_end_s: finding.timeline_end_s,
                confidence: 0.75,
                risk: "medium",
                suggested_action: "review_cut_candidate",
                evidence: format!(
                    "restart marker {:?}; snippet={:?}",
                    finding.marker, finding.snippet
                ),
                requires_approval: true,
            });
        }

        let status = if safe_cuts.is_empty() && review_cuts.is_empty() {
            "no_candidates"
        } else if missing_evidence.is_empty() {
            "ready"
        } else {
            "partial"
        };
        let summary_for_agent = format!(
            "Cleanup status: {status}. Found {} safe cut(s), {} review cut(s), and {} risky cut(s).",
            safe_cuts.len(),
            review_cuts.len(),
            risky_cuts.len()
        );
        let report = CleanupReport {
            status,
            summary_for_agent,
            safe_cuts,
            review_cuts,
            risky_cuts,
            missing_evidence,
        };
        serde_json::to_string(&report)
            .map(ToolOutput::text)
            .map_err(|e| {
                FunctionCallError::Fatal(format!("podcast_cleanup_candidates serialize: {e}"))
            })
    }
}

const DESCRIPTION: &str = "\
Aggregate existing podcast cleanup evidence into safe/review/risky \
candidate buckets. Uses current dead-air, filler-word, and false-start \
scanners; it does not mutate the timeline and does not require a new \
audio-analysis indexer.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange, Timeline, Track, TrackChild, TrackKind,
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
        let hash = fnv1a32_hex(&raw);
        let stem = raw.file_stem().unwrap().to_string_lossy();
        let path = root
            .join(".montage")
            .join("silences")
            .join(format!("{stem}-{hash}.json"));
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

    fn fnv1a32_hex(path: &std::path::Path) -> String {
        let mut hash: u32 = 0x811c9dc5;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        format!("{hash:08x}")
    }

    #[tokio::test]
    async fn reports_safe_dead_air_candidate_from_existing_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mov";
        make_project(dir.path(), asset);
        write_silences(dir.path(), asset);

        let out = PodcastCleanupCandidatesTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "podcast_cleanup_candidates".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "partial");
        assert_eq!(value["safe_cuts"][0]["kind"], "dead_air");
        assert_eq!(
            value["safe_cuts"][0]["suggested_action"],
            "safe_cut_candidate"
        );
    }
}
