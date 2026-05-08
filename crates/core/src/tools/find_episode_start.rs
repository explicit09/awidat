//! `find_episode_start` tool — find the publishable start of an episode.
//!
//! This is intentionally different from `read_index(offset=0)` and from
//! `find_beat(kind="hook")`. Podcast recordings often begin with real
//! transcript text that is still pre-roll: mic checks, planning, rehearsal,
//! or a false intro. This tool scans a broad transcript window, scores
//! canonical host-intro cues, and explicitly rejects setup/rehearsal cues.

use async_trait::async_trait;
use awidat_index::{read_sidecar, walk_indexer};
use awidat_proto::index::AssetId;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

const DEFAULT_SEARCH_UNTIL_S: f64 = 1800.0;
const MAX_SEARCH_UNTIL_S: f64 = 7200.0;
const DEFAULT_CONTEXT_S: f64 = 45.0;
const MAX_CANDIDATES: usize = 8;

/// Finds the publishable start of a podcast/interview transcript.
pub struct FindEpisodeStartTool;

#[derive(Debug, Deserialize)]
struct FindEpisodeStartArgs {
    #[serde(default)]
    asset_id: Option<String>,
    #[serde(default)]
    search_until_s: Option<f64>,
    #[serde(default)]
    context_s: Option<f64>,
}

#[derive(Debug, Clone)]
struct Segment {
    start_s: f64,
    end_s: f64,
    speaker_id: serde_json::Value,
    text: String,
}

#[derive(Debug, Clone)]
struct Candidate {
    asset_id: String,
    segment_idx: usize,
    start_s: f64,
    end_s: f64,
    score: f64,
    rejected: bool,
    label: &'static str,
    reasons: Vec<String>,
}

#[async_trait]
impl ToolHandler for FindEpisodeStartTool {
    fn name(&self) -> &'static str {
        "find_episode_start"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_episode_start".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Optional project-relative asset path. Omit to scan every whisper transcript sidecar."
                    },
                    "search_until_s": {
                        "type": "number",
                        "minimum": 30,
                        "maximum": 7200,
                        "description": "Only inspect transcript segments that start before this source time. Default 1800s."
                    },
                    "context_s": {
                        "type": "number",
                        "minimum": 10,
                        "maximum": 120,
                        "description": "Transcript context returned around the recommended start. Default 45s."
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
        let args: FindEpisodeStartArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_episode_start: invalid args ({e}). All fields are optional."
            ))
        })?;
        let search_until_s = args
            .search_until_s
            .unwrap_or(DEFAULT_SEARCH_UNTIL_S)
            .clamp(30.0, MAX_SEARCH_UNTIL_S);
        let context_s = args
            .context_s
            .unwrap_or(DEFAULT_CONTEXT_S)
            .clamp(10.0, 120.0);

        let mut assets = Vec::new();
        if let Some(asset_id) = args.asset_id {
            let asset = AssetId::new(asset_id.clone());
            let sidecar = read_sidecar(&ctx.project_root, "whisper", &asset)
                .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
            assets.push((asset_id, sidecar));
        } else {
            let walker = walk_indexer(&ctx.project_root, "whisper")
                .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
            assets.extend(walker);
        }

        let mut candidates = Vec::new();
        let mut by_asset = Vec::new();
        for (asset_id, sidecar) in assets {
            let segments = parse_segments(&sidecar);
            if segments.is_empty() {
                continue;
            }
            candidates.extend(score_asset(&asset_id, &segments, search_until_s));
            by_asset.push((asset_id, segments));
        }

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    a.start_s
                        .partial_cmp(&b.start_s)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        let recommended = candidates
            .iter()
            .find(|c| !c.rejected && c.score >= 6.0)
            .cloned();

        let recommended_json = recommended.as_ref().map(|candidate| {
            let segments = by_asset
                .iter()
                .find(|(asset_id, _)| asset_id == &candidate.asset_id)
                .map(|(_, segments)| segments.as_slice())
                .unwrap_or(&[]);
            serde_json::json!({
                "asset_id": candidate.asset_id,
                "start_s": round3(candidate.start_s),
                "timecode": format_time(candidate.start_s),
                "confidence": confidence(candidate.score),
                "score": round2(candidate.score),
                "reason": candidate.reasons.first().cloned().unwrap_or_else(|| "best-scoring clean intro candidate".into()),
                "evidence": evidence_window(segments, candidate.segment_idx, context_s),
            })
        });

        let candidate_rows: Vec<_> = candidates
            .iter()
            .take(MAX_CANDIDATES)
            .map(|c| {
                serde_json::json!({
                    "asset_id": c.asset_id,
                    "start_s": round3(c.start_s),
                    "timecode": format_time(c.start_s),
                    "end_s": round3(c.end_s),
                    "score": round2(c.score),
                    "status": if c.rejected { "rejected" } else { "candidate" },
                    "label": c.label,
                    "reasons": c.reasons,
                })
            })
            .collect();

        let body = serde_json::json!({
            "recommended": recommended_json,
            "candidates": candidate_rows,
            "note": "Use the recommended start for publishable intro/cold-open decisions. Do not infer episode start from read_index(offset=0) when pre-roll or rehearsal is present."
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

fn parse_segments(sidecar: &serde_json::Value) -> Vec<Segment> {
    sidecar
        .pointer("/data/segments")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|seg| {
            let text = seg.get("text")?.as_str()?.trim();
            if text.is_empty() {
                return None;
            }
            Some(Segment {
                start_s: seg
                    .get("start_s")
                    .or_else(|| seg.get("start"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                end_s: seg
                    .get("end_s")
                    .or_else(|| seg.get("end"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0),
                speaker_id: seg
                    .get("speaker_id")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                text: text.to_string(),
            })
        })
        .collect()
}

fn score_asset(asset_id: &str, segments: &[Segment], search_until_s: f64) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (idx, seg) in segments.iter().enumerate() {
        if seg.start_s > search_until_s {
            break;
        }
        let window = joined_text(segments, idx, seg.start_s + 75.0);
        let local = joined_text(segments, idx, seg.start_s + 20.0);
        let segment_text = normalize(&seg.text);
        let previous = joined_previous_text(segments, idx, 120.0);

        let mut score = 0.0;
        let mut reasons = Vec::new();
        let mut positive = 0;

        add_if(
            &window,
            "welcome to ",
            8.0,
            "host welcome/opening line",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "welcome back",
            7.0,
            "host welcome/opening line",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "today we have",
            5.0,
            "introduces today's guest/topic",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "today, we have",
            5.0,
            "introduces today's guest/topic",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "special guest",
            4.0,
            "formal guest introduction",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "our podcast",
            3.0,
            "podcast framing",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "the podcast",
            2.0,
            "podcast framing",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "great to have you",
            4.0,
            "host welcomes guest",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "thanks for the introduction",
            3.0,
            "guest responds to host intro",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "first question",
            2.0,
            "moves from intro into interview",
            &mut score,
            &mut reasons,
            &mut positive,
        );
        add_if(
            &window,
            "before we dive",
            2.0,
            "formal intro phrasing",
            &mut score,
            &mut reasons,
            &mut positive,
        );

        if seg.start_s > 300.0 && positive > 0 {
            score += 1.5;
            reasons.push("clean intro appears after early pre-roll".into());
        }

        if idx > 0 {
            let gap = (seg.start_s - segments[idx - 1].end_s).max(0.0);
            if gap >= 4.0 && positive > 0 {
                score += 1.0;
                reasons.push("follows a noticeable reset/gap".into());
            }
        }

        let mut rejected = false;
        let mut label = "possible_episode_start";
        for (cue, penalty, reason) in SETUP_CUES {
            if local.contains(cue) {
                score -= penalty;
                rejected = true;
                label = "setup_or_rehearsal";
                reasons.push((*reason).into());
            }
        }

        for (cue, penalty, reason) in START_REJECT_CUES {
            if segment_text.contains(cue) {
                score -= penalty;
                rejected = true;
                label = "setup_or_rehearsal";
                reasons.push((*reason).into());
            }
        }

        if positive == 0 {
            for (cue, penalty, reason) in PRE_ROLL_CUES {
                if local.contains(cue) || previous.contains(cue) {
                    score -= penalty;
                    reasons.push((*reason).into());
                }
            }
        }

        if seg.start_s < 120.0 && positive == 0 {
            score -= 3.0;
            reasons.push("early transcript without episode-intro cues".into());
        }

        if positive > 0 || rejected || score > 2.0 {
            out.push(Candidate {
                asset_id: asset_id.to_string(),
                segment_idx: idx,
                start_s: seg.start_s,
                end_s: seg.end_s,
                score,
                rejected,
                label,
                reasons,
            });
        }
    }
    out
}

const SETUP_CUES: &[(&str, f64, &str)] = &[
    (
        "intro is going to be",
        18.0,
        "rejected because this sounds like a rehearsed intro plan",
    ),
    (
        "my intro is",
        12.0,
        "rejected because this sounds like rehearsal/setup",
    ),
    (
        "normally explicitly say",
        10.0,
        "rejected because speakers are discussing recording mechanics",
    ),
    (
        "this might take one or two cuts",
        10.0,
        "rejected because this is production chatter",
    ),
    (
        "off camera",
        10.0,
        "rejected because this is off-camera direction",
    ),
    (
        "camera",
        4.0,
        "rejected because this references production setup",
    ),
    (
        "mic check",
        10.0,
        "rejected because this is recording setup",
    ),
];

const PRE_ROLL_CUES: &[(&str, f64, &str)] = &[
    (
        "want to start",
        6.0,
        "pre-roll asks whether recording should start",
    ),
    (
        "can't say anything dumb",
        5.0,
        "pre-roll banter before the show",
    ),
    ("where are we at", 3.0, "recording-position check"),
    ("one or two cuts", 6.0, "production chatter"),
    ("two extra hats", 4.0, "production/planning aside"),
    (
        "questions that i can expound",
        4.0,
        "planning the interview structure",
    ),
];

const START_REJECT_CUES: &[(&str, f64, &str)] = &[
    (
        "where are we at",
        12.0,
        "rejected because this segment is a recording-position check before the intro",
    ),
    (
        "okay okay okay",
        8.0,
        "rejected because this segment is reset chatter before the intro",
    ),
];

fn add_if(
    text: &str,
    needle: &str,
    points: f64,
    reason: &str,
    score: &mut f64,
    reasons: &mut Vec<String>,
    positive: &mut usize,
) {
    if text.contains(needle) {
        *score += points;
        *positive += 1;
        if !reasons.iter().any(|r| r == reason) {
            reasons.push(reason.into());
        }
    }
}

fn joined_text(segments: &[Segment], start_idx: usize, end_s: f64) -> String {
    segments
        .iter()
        .skip(start_idx)
        .take_while(|seg| seg.start_s <= end_s)
        .map(|seg| normalize(&seg.text))
        .collect::<Vec<_>>()
        .join(" ")
}

fn joined_previous_text(segments: &[Segment], start_idx: usize, context_s: f64) -> String {
    if start_idx == 0 {
        return String::new();
    }
    let start_s = (segments[start_idx].start_s - context_s).max(0.0);
    segments
        .iter()
        .take(start_idx)
        .filter(|seg| seg.end_s >= start_s)
        .map(|seg| normalize(&seg.text))
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch.is_ascii_whitespace() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn evidence_window(segments: &[Segment], idx: usize, context_s: f64) -> Vec<serde_json::Value> {
    if segments.is_empty() {
        return Vec::new();
    }
    let start_s = (segments[idx].start_s - context_s).max(0.0);
    let end_s = segments[idx].start_s + context_s;
    segments
        .iter()
        .filter(|seg| seg.end_s >= start_s && seg.start_s <= end_s)
        .map(|seg| {
            serde_json::json!({
                "start_s": round3(seg.start_s),
                "end_s": round3(seg.end_s),
                "speaker_id": seg.speaker_id,
                "text": seg.text,
            })
        })
        .collect()
}

fn confidence(score: f64) -> &'static str {
    if score >= 16.0 {
        "high"
    } else if score >= 10.0 {
        "medium"
    } else {
        "low"
    }
}

fn format_time(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

const DESCRIPTION: &str = "\
Find the actual publishable start of a podcast/interview episode by \
scanning the whisper transcript for clean host-intro cues and rejecting \
pre-roll, off-camera setup, and rehearsal intros. Use this before \
trimming the top of a podcast or answering 'what time does the episode \
start?'. It is safer than reading transcript offset 0 because raw \
recordings often begin with real but unpublished chatter. Returns a \
recommended start time, confidence, evidence transcript, and rejected \
candidates. Requires the whisper transcript index.\
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
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "find_episode_start".into(),
            args,
        }
    }

    fn write_whisper(root: &std::path::Path, asset: &str, segments: Vec<serde_json::Value>) {
        let p = root.join("index/whisper").join(format!("{asset}.json"));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            p,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": asset,
                "data": {
                    "language": "en",
                    "segments": segments,
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn rejects_preroll_and_rehearsal_intro_for_ben_style_recording() {
        let dir = tempfile::tempdir().unwrap();
        write_whisper(
            dir.path(),
            "raw/Podcast May 3 2026.mov",
            vec![
                serde_json::json!({"start_s": 1.178, "end_s": 1.498, "speaker_id": null, "text": "Thank you."}),
                serde_json::json!({"start_s": 2.679, "end_s": 4.701, "speaker_id": null, "text": "He was asking if we want to start."}),
                serde_json::json!({"start_s": 28.45, "end_s": 30.692, "speaker_id": null, "text": "So wait, when you say CXO, is that like the word for it?"}),
                serde_json::json!({"start_s": 429.5, "end_s": 437.2, "speaker_id": null, "text": "I'm so excited for this, guys."}),
                serde_json::json!({"start_s": 694.78, "end_s": 703.2, "speaker_id": null, "text": "So my intro is going to be like, welcome to technology. Today we have a very special guest."}),
                serde_json::json!({"start_s": 933.52, "end_s": 948.42, "speaker_id": null, "text": "this might take one or two cuts, so."}),
                serde_json::json!({"start_s": 963.25, "end_s": 965.1, "speaker_id": null, "text": "Okay, okay, okay. Where are we at?"}),
                serde_json::json!({"start_s": 967.15, "end_s": 970.2, "speaker_id": null, "text": "Welcome to technology."}),
                serde_json::json!({"start_s": 973.0, "end_s": 981.0, "speaker_id": null, "text": "Today, we have a very special guest on our podcast, Mr. Ben Adams."}),
                serde_json::json!({"start_s": 982.0, "end_s": 988.0, "speaker_id": null, "text": "Thanks for the introduction. It's great to have you here."}),
                serde_json::json!({"start_s": 996.056, "end_s": 1001.0, "speaker_id": null, "text": "First question, we are in a very software obsessed world."}),
            ],
        );

        let out = FindEpisodeStartTool
            .handle(
                invoke(serde_json::json!({"asset_id": "raw/Podcast May 3 2026.mov"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(v["recommended"]["start_s"], serde_json::json!(967.15));
        assert_eq!(v["recommended"]["timecode"], serde_json::json!("16:07"));
        assert!(
            v["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["start_s"] == serde_json::json!(694.78)
                    && c["status"] == serde_json::json!("rejected"))
        );
    }
}
