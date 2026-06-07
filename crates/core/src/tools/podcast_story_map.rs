//! `podcast_story_map` tool — tolerant first-watch story pass.
//!
//! This gathers the evidence an editor would collect before making
//! cuts: strong beats, low-value tangents/dead air, and transcript
//! spans that look like production/meta-direction chatter.

use async_trait::async_trait;
use montage_index::walk_indexer;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Read-only podcast story map.
pub struct PodcastStoryMapTool;

#[derive(Debug, Deserialize)]
struct PodcastStoryMapArgs {
    #[serde(default)]
    limit_per_kind: Option<usize>,
    #[serde(default)]
    min_score: Option<f64>,
}

#[derive(Debug, Serialize)]
struct PodcastStoryMap {
    status: &'static str,
    summary_for_agent: String,
    beat_kinds: Vec<BeatKindSummary>,
    production_chatter_candidates: Vec<TranscriptCandidate>,
    missing_evidence: Vec<String>,
    next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BeatKindSummary {
    kind: &'static str,
    candidates: Vec<BeatCandidate>,
}

#[derive(Debug, Serialize)]
struct BeatCandidate {
    asset_id: String,
    moment_id: Option<String>,
    start_s: Option<f64>,
    end_s: Option<f64>,
    score: f64,
    speaker: Option<String>,
    note: Option<String>,
    dependencies: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct TranscriptCandidate {
    asset_id: String,
    start_s: f64,
    end_s: f64,
    suggested_cut_start_s: f64,
    suggested_cut_end_s: f64,
    speaker: Option<String>,
    text: String,
    context_before: Option<String>,
    context_after: Option<String>,
    reason: &'static str,
    confidence: f64,
    risk: &'static str,
    suggested_action: &'static str,
    matched_phrase: &'static str,
}

#[async_trait]
impl ToolHandler for PodcastStoryMapTool {
    fn name(&self) -> &'static str {
        "podcast_story_map"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit_per_kind": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "description": "Maximum candidates to return for each beat kind. Default 6."
                    },
                    "min_score": {
                        "type": "number",
                        "minimum": 0.0,
                        "maximum": 1.0,
                        "description": "Minimum editorial moment score. Default 0.45."
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
        let args: PodcastStoryMapArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_story_map: invalid args ({e}). All fields are optional."
            ))
        })?;
        let limit_per_kind = args.limit_per_kind.unwrap_or(6).clamp(1, 20);
        let min_score = args.min_score.unwrap_or(0.45).clamp(0.0, 1.0);

        let mut missing_evidence = Vec::new();
        let moments = match read_editorial_moments(&ctx.project_root) {
            Ok(moments) => moments,
            Err(message) => {
                missing_evidence.push(message);
                Vec::new()
            }
        };
        let transcript_candidates = match read_production_chatter(&ctx.project_root) {
            Ok(candidates) => candidates,
            Err(message) => {
                missing_evidence.push(message);
                Vec::new()
            }
        };

        let beat_kinds = [
            "hook",
            "story",
            "emotional_peak",
            "punchline",
            "cta",
            "tangent",
            "dead_air",
        ]
        .into_iter()
        .map(|kind| BeatKindSummary {
            kind,
            candidates: top_candidates(&moments, kind, min_score, limit_per_kind),
        })
        .collect::<Vec<_>>();

        let strong_count: usize = beat_kinds
            .iter()
            .filter(|group| {
                matches!(
                    group.kind,
                    "hook" | "story" | "emotional_peak" | "punchline"
                )
            })
            .map(|group| group.candidates.len())
            .sum();
        let removal_count = beat_kinds
            .iter()
            .filter(|group| matches!(group.kind, "tangent" | "dead_air"))
            .map(|group| group.candidates.len())
            .sum::<usize>()
            + transcript_candidates.len();
        let status = if missing_evidence.is_empty() {
            "ready"
        } else if strong_count > 0 || removal_count > 0 {
            "partial"
        } else {
            "missing_indexes"
        };
        let summary_for_agent = format!(
            "Story map status: {status}. Found {strong_count} strong beat candidate(s) and {removal_count} removal/tightening candidate(s)."
        );
        let next_steps = vec![
            "Use hook/story/emotional candidates to choose the opening and chapter spine.".into(),
            "Treat tangent, dead-air, and production-chatter candidates as proposed removals, not automatic destructive edits.".into(),
            "Before applying risky cuts, check continuity with assess_edit_quality at the candidate boundary.".into(),
            "Ask for pre-render confirmation, then hand off the rendered draft for review.".into(),
        ];

        let response = PodcastStoryMap {
            status,
            summary_for_agent,
            beat_kinds,
            production_chatter_candidates: transcript_candidates,
            missing_evidence,
            next_steps,
        };
        serde_json::to_string(&response)
            .map(ToolOutput::text)
            .map_err(|e| FunctionCallError::Fatal(format!("podcast_story_map serialize: {e}")))
    }
}

#[derive(Debug, Clone)]
struct MomentRow {
    asset_id: String,
    value: serde_json::Value,
}

fn read_editorial_moments(project_root: &std::path::Path) -> Result<Vec<MomentRow>, String> {
    let walker = walk_indexer(project_root, "editorial-moments")
        .map_err(|e| format!("editorial-moments index missing or unreadable: {e}"))?;
    let mut rows = Vec::new();
    for (asset_id, sidecar) in walker {
        let Some(moments) = sidecar.pointer("/data/moments").and_then(|v| v.as_array()) else {
            continue;
        };
        for value in moments {
            rows.push(MomentRow {
                asset_id: asset_id.clone(),
                value: value.clone(),
            });
        }
    }
    if rows.is_empty() {
        return Err("editorial-moments index had no moments".into());
    }
    Ok(rows)
}

fn top_candidates(
    moments: &[MomentRow],
    kind: &'static str,
    min_score: f64,
    limit: usize,
) -> Vec<BeatCandidate> {
    let mut rows = moments
        .iter()
        .filter(|row| row.value.get("kind").and_then(|v| v.as_str()) == Some(kind))
        .filter_map(|row| {
            let score = row
                .value
                .get("score")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            if score < min_score {
                return None;
            }
            Some(BeatCandidate {
                asset_id: row.asset_id.clone(),
                moment_id: row
                    .value
                    .get("moment_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                start_s: row.value.get("start_s").and_then(|value| value.as_f64()),
                end_s: row.value.get("end_s").and_then(|value| value.as_f64()),
                score,
                speaker: row
                    .value
                    .get("speaker")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                note: row
                    .value
                    .get("note")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                dependencies: row
                    .value
                    .get("dependencies")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows.truncate(limit);
    rows
}

fn read_production_chatter(
    project_root: &std::path::Path,
) -> Result<Vec<TranscriptCandidate>, String> {
    let walker = walk_indexer(project_root, "whisper")
        .map_err(|e| format!("whisper transcript index missing or unreadable: {e}"))?;
    let mut candidates = Vec::new();
    for (asset_id, sidecar) in walker {
        let Some(segments) = sidecar.pointer("/data/segments").and_then(|v| v.as_array()) else {
            continue;
        };
        for (index, segment) in segments.iter().enumerate() {
            let text = segment_text(segment);
            let Some(chatter) = score_production_chatter(text) else {
                continue;
            };
            let Some(start_s) = segment.get("start_s").and_then(|value| value.as_f64()) else {
                continue;
            };
            let Some(end_s) = segment.get("end_s").and_then(|value| value.as_f64()) else {
                continue;
            };
            let context_before = index
                .checked_sub(1)
                .and_then(|previous| segments.get(previous))
                .map(segment_text)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
            let context_after = segments
                .get(index + 1)
                .map(segment_text)
                .filter(|text| !text.is_empty())
                .map(str::to_string);
            candidates.push(TranscriptCandidate {
                asset_id: asset_id.clone(),
                start_s,
                end_s,
                suggested_cut_start_s: (start_s - chatter.handle_before_s).max(0.0),
                suggested_cut_end_s: end_s + chatter.handle_after_s,
                speaker: segment
                    .get("speaker")
                    .or_else(|| segment.get("speaker_id"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                text: text.to_string(),
                context_before,
                context_after,
                reason: chatter.reason,
                confidence: chatter.confidence,
                risk: chatter.risk,
                suggested_action: chatter.suggested_action,
                matched_phrase: chatter.matched_phrase,
            });
        }
    }
    candidates.sort_by(|a, b| {
        a.start_s
            .partial_cmp(&b.start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(50);
    Ok(candidates)
}

fn segment_text(segment: &serde_json::Value) -> &str {
    segment
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
}

#[derive(Debug, Clone, Copy)]
struct ProductionChatterMatch {
    reason: &'static str,
    confidence: f64,
    risk: &'static str,
    suggested_action: &'static str,
    matched_phrase: &'static str,
    handle_before_s: f64,
    handle_after_s: f64,
}

fn score_production_chatter(text: &str) -> Option<ProductionChatterMatch> {
    let lower = text.to_ascii_lowercase();
    let high_confidence = [
        ("cut that", "cut_request"),
        ("can we cut", "cut_request"),
        ("say it again", "restart_or_retake"),
        ("let me say that again", "restart_or_retake"),
        ("start again", "restart_or_retake"),
        ("let's restart", "restart_or_retake"),
        ("lets restart", "restart_or_retake"),
    ];
    for (needle, reason) in high_confidence {
        if lower.contains(needle) {
            return Some(ProductionChatterMatch {
                reason,
                confidence: 0.95,
                risk: "low",
                suggested_action: "safe_cut_candidate",
                matched_phrase: needle,
                handle_before_s: 1.5,
                handle_after_s: 0.3,
            });
        }
    }

    let medium_confidence = [
        ("you can just say", "direction_to_speaker"),
        ("i think you can", "direction_to_speaker"),
        ("maybe we can ask", "question_planning"),
        ("we don't have to", "production_planning"),
        ("we do not have to", "production_planning"),
        ("ask you about", "question_planning"),
        ("go into a long", "production_planning"),
    ];
    for (needle, reason) in medium_confidence {
        if lower.contains(needle) {
            return Some(ProductionChatterMatch {
                reason,
                confidence: 0.72,
                risk: "medium",
                suggested_action: "review_cut_candidate",
                matched_phrase: needle,
                handle_before_s: 1.0,
                handle_after_s: 0.3,
            });
        }
    }

    let uncertain = [
        ("on the podcast", "meta_podcast_reference"),
        ("for the podcast", "meta_podcast_reference"),
    ];
    for (needle, reason) in uncertain {
        if lower.contains(needle) {
            return Some(ProductionChatterMatch {
                reason,
                confidence: 0.45,
                risk: "high",
                suggested_action: "review_only",
                matched_phrase: needle,
                handle_before_s: 0.0,
                handle_after_s: 0.0,
            });
        }
    }

    None
}

const DESCRIPTION: &str = "\
Build a first-watch podcast story map from indexed editorial moments and \
transcript sidecars. Returns hook/story/emotional/punchline/CTA candidates, \
tangent/dead-air candidates, and transcript spans that look like production \
or meta-direction chatter inside the interview. This tool is tolerant of \
missing indexes: it reports missing evidence instead of failing the workflow.\
";

#[cfg(test)]
mod tests {
    use super::*;
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

    fn write_sidecar(root: &std::path::Path, indexer: &str, asset: &str, data: serde_json::Value) {
        let path = root
            .join("index")
            .join(indexer)
            .join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({
                "indexer": indexer,
                "asset_id": asset,
                "data": data
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn returns_story_and_production_chatter_candidates() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "editorial-moments",
            "raw_ep",
            serde_json::json!({
                "moments": [
                    {"moment_id": "h1", "kind": "hook", "start_s": 10.0, "end_s": 18.0, "score": 0.9, "note": "strong opening"},
                    {"moment_id": "t1", "kind": "tangent", "start_s": 90.0, "end_s": 100.0, "score": 0.7, "note": "low value"}
                ]
            }),
        );
        write_sidecar(
            dir.path(),
            "whisper",
            "raw_ep",
            serde_json::json!({
                "segments": [
                    {"start_s": 95.0, "end_s": 99.0, "speaker": "S1", "text": "I think you can just say it in the beginning."}
                ]
            }),
        );

        let out = PodcastStoryMapTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "podcast_story_map".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "ready");
        assert_eq!(value["production_chatter_candidates"][0]["start_s"], 95.0);
        assert_eq!(
            value["production_chatter_candidates"][0]["suggested_action"],
            "review_cut_candidate"
        );
        assert_eq!(
            value["production_chatter_candidates"][0]["reason"],
            "direction_to_speaker"
        );
        assert_eq!(
            value["production_chatter_candidates"][0]["matched_phrase"],
            "you can just say"
        );
        assert_eq!(value["beat_kinds"][0]["candidates"][0]["moment_id"], "h1");
    }

    #[tokio::test]
    async fn production_chatter_scoring_avoids_generic_beginning_false_positive() {
        let dir = tempfile::tempdir().unwrap();
        write_sidecar(
            dir.path(),
            "whisper",
            "raw_ep",
            serde_json::json!({
                "segments": [
                    {"start_s": 10.0, "end_s": 14.0, "text": "In the beginning, startups usually do not know their customer."},
                    {"start_s": 20.0, "end_s": 22.0, "text": "Let me say that again."}
                ]
            }),
        );

        let out = PodcastStoryMapTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "podcast_story_map".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        let candidates = value["production_chatter_candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0]["text"], "Let me say that again.");
        assert_eq!(candidates[0]["suggested_action"], "safe_cut_candidate");
        assert_eq!(candidates[0]["risk"], "low");
    }

    #[tokio::test]
    async fn missing_indexes_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let out = PodcastStoryMapTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "podcast_story_map".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "missing_indexes");
        assert!(
            value["missing_evidence"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }
}
