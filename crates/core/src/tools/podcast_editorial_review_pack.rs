//! `podcast_editorial_review_pack` tool — compact evidence packets
//! for AI editorial classification.
//!
//! Existing scanners are good recall mechanisms, but they should not
//! be treated as final editorial judgment. This tool packages their
//! findings with transcript context so the active agent can classify
//! meaning before proposing any timeline mutation.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

const DEFAULT_MAX_RESULTS: usize = 30;
const HARD_MAX_RESULTS: usize = 100;
const DEFAULT_WINDOW_PADDING_S: f64 = 5.0;
const MAX_WINDOW_PADDING_S: f64 = 20.0;

/// Read-only tool that prepares editorial evidence for AI review.
pub struct PodcastEditorialReviewPackTool;

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    window_padding_s: Option<f64>,
    #[serde(default)]
    include_dead_air: Option<bool>,
}

#[derive(Debug, Serialize)]
struct EditorialReviewPack {
    status: &'static str,
    summary_for_agent: String,
    classification_schema: serde_json::Value,
    agent_instructions: Vec<&'static str>,
    packets: Vec<EditorialReviewPacket>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct EditorialReviewPacket {
    id: String,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    signals: Vec<String>,
    transcript_before: String,
    transcript_during: String,
    transcript_after: String,
    review_question: String,
}

#[async_trait]
impl ToolHandler for PodcastEditorialReviewPackTool {
    fn name(&self) -> &'static str {
        "podcast_editorial_review_pack"
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
                        "maximum": HARD_MAX_RESULTS,
                        "description": "Maximum review packets to return. Default 30."
                    },
                    "window_padding_s": {
                        "type": "number",
                        "minimum": 1,
                        "maximum": MAX_WINDOW_PADDING_S,
                        "description": "Transcript context to include before and after each evidence range. Default 5s."
                    },
                    "include_dead_air": {
                        "type": "boolean",
                        "description": "Include silence ranges as review evidence. Default true."
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
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_editorial_review_pack: invalid args ({e}). All fields are optional."
            ))
        })?;
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, HARD_MAX_RESULTS);
        let window_padding_s = args
            .window_padding_s
            .unwrap_or(DEFAULT_WINDOW_PADDING_S)
            .clamp(1.0, MAX_WINDOW_PADDING_S);
        let include_dead_air = args.include_dead_air.unwrap_or(true);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_editorial_review_pack: failed to read project: {e}"
            ))
        })?;

        let report = build_review_pack(
            &ctx.project_root,
            &project.timeline,
            max_results,
            window_padding_s,
            include_dead_air,
        );
        serde_json::to_string(&report)
            .map(ToolOutput::text)
            .map_err(|e| FunctionCallError::Fatal(format!("serialize editorial review pack: {e}")))
    }
}

fn build_review_pack(
    project_root: &Path,
    timeline: &montage_proto::otio::Timeline,
    max_results: usize,
    window_padding_s: f64,
    include_dead_air: bool,
) -> EditorialReviewPack {
    let mut packets = Vec::new();
    let mut missing_evidence = Vec::new();

    let false_starts =
        crate::podcast_cleanup_scan::scan_false_starts(project_root, timeline, max_results);
    if false_starts.is_empty() {
        missing_evidence.push("no false-start or production-aside recall signals".into());
    }
    for finding in false_starts {
        let signals = vec![format!(
            "false_start_or_production_aside:{}",
            finding.marker
        )];
        packets.push(packet_from_range(
            project_root,
            packets.len(),
            finding.asset_id,
            finding.source_start_s,
            finding.source_end_s,
            finding.timeline_start_s,
            finding.timeline_end_s,
            signals,
            window_padding_s,
            "Does this range represent a false start, production/coaching aside, natural speech, or useful content?",
        ));
        if packets.len() >= max_results {
            break;
        }
    }

    if include_dead_air && packets.len() < max_results {
        let dead_air = crate::podcast_cleanup_scan::scan_dead_air(
            project_root,
            timeline,
            0.8,
            max_results - packets.len(),
        );
        if dead_air.is_empty() {
            missing_evidence.push("no timeline-visible silence recall signals".into());
        }
        for finding in dead_air {
            let signals = vec![format!("silence:{:.2}s", finding.duration_s)];
            packets.push(packet_from_range(
                project_root,
                packets.len(),
                finding.asset_id,
                finding.source_start_s,
                finding.source_end_s,
                finding.timeline_start_s,
                finding.timeline_end_s,
                signals,
                window_padding_s,
                "Is this silence dead air to tighten, intentional pacing, a thought boundary, or a risky cut?",
            ));
            if packets.len() >= max_results {
                break;
            }
        }
    }

    let status = if packets.is_empty() {
        "no_packets"
    } else if missing_evidence.is_empty() {
        "ready"
    } else {
        "partial"
    };
    EditorialReviewPack {
        status,
        summary_for_agent: format!(
            "Editorial review pack: {status}. Classify {} packet(s) before proposing cleanup or episode-boundary edits.",
            packets.len()
        ),
        classification_schema: serde_json::json!({
            "decision": ["cut", "keep", "review"],
            "editorial_label": [
                "false_start",
                "production_aside",
                "coaching",
                "setup_or_not_recording",
                "dead_air",
                "natural_pause",
                "publishable_content",
                "episode_boundary",
                "cold_open_candidate",
                "needs_human_review"
            ],
            "confidence": "0.0 to 1.0",
            "reason": "Brief editorial explanation grounded in transcript_before/during/after.",
            "proposed_source_range_s": "Only include when decision is cut or review."
        }),
        agent_instructions: vec![
            "Treat signals as recall evidence, not truth.",
            "Use transcript_before, transcript_during, and transcript_after to decide editorial meaning.",
            "Never apply a cut directly from this pack; first state the classification and why.",
            "Silence alone is not a cut. Classify whether the pause is dead air, pacing, a thought boundary, or risky.",
            "For podcast starts, prefer publishable conversational intent over literal phrases like welcome.",
        ],
        packets,
        missing_evidence,
    }
}

#[allow(clippy::too_many_arguments)]
fn packet_from_range(
    project_root: &Path,
    index: usize,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    signals: Vec<String>,
    window_padding_s: f64,
    review_question: &'static str,
) -> EditorialReviewPacket {
    let transcript = read_transcript_window(
        project_root,
        &asset_id,
        source_start_s,
        source_end_s,
        window_padding_s,
    );
    EditorialReviewPacket {
        id: format!("editorial-review-{index:03}"),
        asset_id,
        source_start_s,
        source_end_s,
        timeline_start_s,
        timeline_end_s,
        signals,
        transcript_before: transcript.before,
        transcript_during: transcript.during,
        transcript_after: transcript.after,
        review_question: review_question.into(),
    }
}

#[derive(Debug, Default)]
struct TranscriptWindow {
    before: String,
    during: String,
    after: String,
}

fn read_transcript_window(
    project_root: &Path,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
    padding_s: f64,
) -> TranscriptWindow {
    let path = whisper_sidecar_path(project_root, asset_id);
    let Ok(bytes) = std::fs::read(&path) else {
        return TranscriptWindow::default();
    };
    let Ok(value): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return TranscriptWindow::default();
    };
    let Some(items) = value
        .pointer("/data/words")
        .or_else(|| value.pointer("/data/segments"))
        .and_then(|v| v.as_array())
    else {
        return TranscriptWindow::default();
    };

    let before_start = start_s - padding_s;
    let after_end = end_s + padding_s;
    let mut before = Vec::new();
    let mut during = Vec::new();
    let mut after = Vec::new();

    for item in items {
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let item_start = item_start_s(item);
        let item_end = item_end_s(item);
        if !item_start.is_finite() || !item_end.is_finite() {
            continue;
        }
        if item_end < before_start || item_start > after_end {
            continue;
        }
        if item_end <= start_s {
            before.push(text);
        } else if item_start >= end_s {
            after.push(text);
        } else {
            during.push(text);
        }
    }

    TranscriptWindow {
        before: before.join(" "),
        during: during.join(" "),
        after: after.join(" "),
    }
}

fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

fn item_start_s(v: &serde_json::Value) -> f64 {
    v.get("start_s")
        .or_else(|| v.get("start"))
        .and_then(|s| s.as_f64())
        .unwrap_or(f64::INFINITY)
}

fn item_end_s(v: &serde_json::Value) -> f64 {
    v.get("end_s")
        .or_else(|| v.get("end"))
        .and_then(|s| s.as_f64())
        .unwrap_or(f64::NEG_INFINITY)
}

const DESCRIPTION: &str = "\
Build a compact AI editorial review pack for podcast cleanup and \
episode-shape decisions. The tool collects timeline-visible recall \
signals such as false starts, production/coaching asides, and optional \
silence ranges, then adds before/during/after transcript context and \
an explicit classification schema. It is read-only and does not label \
anything as a final cut. The active agent must classify each packet \
as cut/keep/review before calling proposal or mutation tools.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
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

    fn invocation(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "podcast_editorial_review_pack".into(),
            args,
        }
    }

    fn timeline_with_clip(
        asset_id: &str,
        source_start_s: f64,
        duration_s: f64,
    ) -> montage_proto::otio::Timeline {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_id));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut timeline = montage_proto::otio::Timeline::empty("test");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        timeline
    }

    fn write_whisper_sidecar(root: &std::path::Path, asset_id: &str, body: serde_json::Value) {
        let path = root
            .join("index")
            .join("whisper")
            .join(format!("{asset_id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
    }

    fn write_silences_sidecar(
        project_root: &std::path::Path,
        asset_id: &str,
        ranges: Vec<(f64, f64)>,
    ) {
        let abs = project_root.join(asset_id);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, b"stub").unwrap();
        let sidecar_path = silences_path_for_test(project_root, &abs);
        std::fs::create_dir_all(sidecar_path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({
            "ranges": ranges
                .into_iter()
                .map(|(s, e)| serde_json::json!({"start_s": s, "end_s": e, "db_floor": -45.0}))
                .collect::<Vec<_>>(),
            "threshold_db": -40.0,
            "min_duration_s": 0.6,
        });
        std::fs::write(sidecar_path, serde_json::to_vec(&payload).unwrap()).unwrap();
    }

    fn silences_path_for_test(
        project_root: &std::path::Path,
        asset_abs_path: &std::path::Path,
    ) -> PathBuf {
        let stem = asset_abs_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("asset");
        project_root.join(".montage").join("silences").join(format!(
            "{stem}-{:08x}.json",
            stable_path_hash_for_test(asset_abs_path)
        ))
    }

    fn stable_path_hash_for_test(path: &std::path::Path) -> u32 {
        let mut hash = 0x811c9dc5_u32;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        hash
    }

    #[tokio::test]
    async fn surfaces_yusuff_style_coaching_as_ai_review_packet() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        let mut project = montage_proto::project::Project::init(dir.path()).unwrap();
        project.timeline = timeline_with_clip(asset, 1520.0, 70.0);
        project.write(dir.path()).unwrap();
        write_whisper_sidecar(
            dir.path(),
            asset,
            serde_json::json!({
                "data": {
                    "segments": [
                        {"start_s": 1527.495, "end_s": 1529.175, "text": "You're"},
                        {"start_s": 1529.175, "end_s": 1530.375, "text": "doing good. Mhmm."},
                        {"start_s": 1531.255, "end_s": 1533.390, "text": "Mhmm. I"},
                        {"start_s": 1535.310, "end_s": 1535.870, "text": "think"},
                        {"start_s": 1542.590, "end_s": 1543.870, "text": "think you can just say, like,"},
                        {"start_s": 1549.155, "end_s": 1554.995, "text": "when do you when do you decide to become a builder?"},
                        {"start_s": 1556.115, "end_s": 1561.350, "text": "We don't have to go into a long one on one in the beginning."},
                        {"start_s": 1568.550, "end_s": 1572.000, "text": "I've actually been building underwater robotics."}
                    ]
                }
            }),
        );

        let output = PodcastEditorialReviewPackTool
            .handle(
                invocation(serde_json::json!({
                    "max_results": 10,
                    "include_dead_air": false,
                    "window_padding_s": 8.0
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let packets = body["packets"].as_array().unwrap();
        assert!(!packets.is_empty());
        let packet_text = serde_json::to_string(&packets[0]).unwrap();
        assert!(packet_text.contains("you can just say"));
        assert!(packet_text.contains("We don't have to"));
        assert_eq!(
            body["classification_schema"]["decision"][0]
                .as_str()
                .unwrap(),
            "cut"
        );
        assert!(
            body["agent_instructions"][0]
                .as_str()
                .unwrap()
                .contains("recall evidence")
        );
    }

    #[tokio::test]
    async fn dead_air_is_evidence_not_final_cut() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        let mut project = montage_proto::project::Project::init(dir.path()).unwrap();
        project.timeline = timeline_with_clip(asset, 0.0, 20.0);
        project.write(dir.path()).unwrap();
        write_whisper_sidecar(
            dir.path(),
            asset,
            serde_json::json!({
                "data": {
                    "words": [
                        {"start_s": 1.0, "end_s": 1.2, "text": "before"},
                        {"start_s": 6.0, "end_s": 6.2, "text": "after"}
                    ]
                }
            }),
        );
        write_silences_sidecar(dir.path(), asset, vec![(2.0, 5.0)]);

        let output = PodcastEditorialReviewPackTool
            .handle(
                invocation(serde_json::json!({
                    "max_results": 10,
                    "include_dead_air": true
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let packet = &body["packets"][0];

        assert!(
            packet["signals"][0]
                .as_str()
                .unwrap()
                .starts_with("silence:")
        );
        assert!(packet.get("suggested_action").is_none());
        assert!(
            body["agent_instructions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|instruction| instruction
                    .as_str()
                    .unwrap()
                    .contains("Silence alone is not a cut"))
        );
    }
}
