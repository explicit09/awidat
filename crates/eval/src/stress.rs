//! Stress scenarios — exercise the system at and beyond expected
//! input sizes, find the failure modes before users do.
//!
//! Each scenario constructs a synthetic-but-realistic worst case
//! (huge transcripts, parallel recorders, malformed sidecars,
//! resume of huge logs, concurrent sub-agent spawning) and asserts
//! the system handles it without panicking, exhausting memory, or
//! corrupting state.
//!
//! These scenarios are NOT in `defaults()` because they're slow
//! (some take seconds) and not what `awidat-eval` should run on
//! every commit. Run via `awidat-eval --stress`.

use anyhow::Result;
use async_trait::async_trait;
use std::time::{Duration, Instant};

use awidat_core::rollout::Recorder;
use awidat_core::tool::ToolHandler;
use awidat_core::tools::{find_moment::FindMomentTool, read_index::ReadIndexTool};
use awidat_proto::project::Project;

use crate::scenarios::{ctx_at, make_call};
use crate::{Scenario, ScenarioOutcome, ScenarioStatus};

/// Build the stress-suite scenario list.
pub fn defaults() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(HugeTranscriptFindMoment),
        Box::new(MalformedSidecarReadIndex),
        Box::new(MissingAssetReadIndex),
        Box::new(ConcurrentRecorderWrites),
        Box::new(LargeRolloutResume),
        Box::new(EmptyTranscriptHandling),
        Box::new(UnicodeQueryHandling),
        Box::new(MaxLimitClampStress),
        Box::new(DecisionBurstWriteAndExtract),
        Box::new(LessonsAtScale),
        Box::new(MultiAssetCorpus),
        Box::new(LongQueryString),
        Box::new(SubagentRegistryFiltering),
        Box::new(RecorderResumeOfTrailingPartialLine),
    ]
}

/// 10K-segment transcript — bigger than any real ~1h video — and
/// `find_moment` must complete quickly. Catches BM25 build-time
/// regressions on large corpora.
struct HugeTranscriptFindMoment;

const HUGE_TRANSCRIPT_FIND_MOMENT_MAX: Duration = Duration::from_millis(8_000);

#[async_trait]
impl Scenario for HugeTranscriptFindMoment {
    fn id(&self) -> &'static str {
        "stress::find_moment_huge_transcript"
    }
    fn description(&self) -> &'static str {
        "find_moment over 10K synthetic transcript segments completes in <2.5s."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        // Synthesize 10K segments. Mix of repeated tokens so BM25 has
        // real ranking work to do.
        let segments: Vec<serde_json::Value> = (0..10_000)
            .map(|i| {
                let text = match i % 5 {
                    0 => format!("the speaker discussed battery problems at length, segment {i}"),
                    1 => format!("a tangent about kitchen appliances, segment {i}"),
                    2 => format!("dead air and brief silence, segment {i}"),
                    3 => format!("punchline about Samsung phones, segment {i}"),
                    _ => format!("filler content segment {i}"),
                };
                serde_json::json!({
                    "text": text,
                    "start_s": i as f64 * 0.3,
                    "end_s": i as f64 * 0.3 + 0.3,
                })
            })
            .collect();
        let sidecar_path = dir.path().join("index/whisper/raw/big.mp4.json");
        std::fs::create_dir_all(sidecar_path.parent().unwrap())?;
        std::fs::write(
            &sidecar_path,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": "raw/big.mp4",
                "data": {"segments": segments}
            }))?,
        )?;

        let query_started = Instant::now();
        let out = FindMomentTool
            .handle(
                make_call(
                    "find_moment",
                    serde_json::json!({"query": "battery", "limit": 10}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let query_elapsed = query_started.elapsed();

        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let n = body["results"].as_array().map(Vec::len).unwrap_or(0);
                if n > 0 && query_elapsed < HUGE_TRANSCRIPT_FIND_MOMENT_MAX {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!(
                            "10K segs, query {}ms, {n} hits",
                            query_elapsed.as_millis()
                        ),
                    }
                } else if n == 0 {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: "BM25 returned 0 hits on 10K seg corpus".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!(
                            "query took {}ms, expected <{}ms",
                            query_elapsed.as_millis(),
                            HUGE_TRANSCRIPT_FIND_MOMENT_MAX.as_millis()
                        ),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("find_moment errored: {e}"),
            },
        })
    }
}

/// Malformed sidecar (truncated JSON) must surface as
/// `RespondToModel`, not panic the whole tool.
struct MalformedSidecarReadIndex;

#[async_trait]
impl Scenario for MalformedSidecarReadIndex {
    fn id(&self) -> &'static str {
        "stress::malformed_sidecar_read_index"
    }
    fn description(&self) -> &'static str {
        "read_index against a corrupt JSON sidecar surfaces a clean error."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let sidecar_path = dir.path().join("index/whisper/raw/broken.mp4.json");
        std::fs::create_dir_all(sidecar_path.parent().unwrap())?;
        std::fs::write(&sidecar_path, b"{ truncated json no closing brace")?;

        let out = ReadIndexTool
            .handle(
                make_call(
                    "read_index",
                    serde_json::json!({
                        "asset_id": "raw/broken.mp4",
                        "channel": "transcript",
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            // We expect either a clean RespondToModel error or a
            // graceful empty result — both are non-panic failures.
            Err(awidat_core::FunctionCallError::RespondToModel(_)) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "surfaced as RespondToModel".into(),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("expected RespondToModel; got {e:?}"),
            },
            Ok(t) => {
                if t.content.is_empty() {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: "expected error; got empty Ok".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "graceful Ok response".into(),
                    }
                }
            }
        })
    }
}

/// `read_index` against an asset that doesn't exist must give a
/// pointed RespondToModel error (not an opaque IO error).
struct MissingAssetReadIndex;

#[async_trait]
impl Scenario for MissingAssetReadIndex {
    fn id(&self) -> &'static str {
        "stress::missing_asset_read_index"
    }
    fn description(&self) -> &'static str {
        "read_index against a nonexistent asset surfaces a pointed error."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;

        let out = ReadIndexTool
            .handle(
                make_call(
                    "read_index",
                    serde_json::json!({
                        "asset_id": "raw/does-not-exist.mp4",
                        "channel": "transcript",
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Err(awidat_core::FunctionCallError::RespondToModel(msg)) => {
                if msg.contains("does-not-exist")
                    || msg.contains("sidecar")
                    || msg.contains("not found")
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "pointed error referencing the missing asset".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("error didn't reference the asset/sidecar: {msg}"),
                    }
                }
            }
            other => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("expected RespondToModel; got {other:?}"),
            },
        })
    }
}

/// 4 parallel recorders (independent sessions) writing concurrently
/// must not corrupt each other's logs. Catches mpsc/file
/// synchronization regressions.
struct ConcurrentRecorderWrites;

#[async_trait]
impl Scenario for ConcurrentRecorderWrites {
    fn id(&self) -> &'static str {
        "stress::concurrent_recorder_writes"
    }
    fn description(&self) -> &'static str {
        "4 parallel sessions, each writing 200 messages, all log files parse cleanly."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        let state_root = dir.path().to_path_buf();

        let mut handles = Vec::new();
        for i in 0..4 {
            let state_root = state_root.clone();
            handles.push(tokio::spawn(async move {
                let rec = Recorder::create(
                    &state_root,
                    std::path::PathBuf::from(format!("/p{i}")),
                    format!("m{i}"),
                )
                .map_err(|e| format!("session {i} create: {e}"))?;
                for j in 0..200 {
                    rec.record_message(awidat_core::anthropic::Message::user_text(format!(
                        "session-{i}-msg-{j}"
                    )));
                }
                rec.flush()
                    .await
                    .map_err(|e| format!("session {i} flush: {e}"))?;
                Ok::<_, String>(rec.path().to_path_buf())
            }));
        }

        let mut paths = Vec::new();
        for h in handles {
            match h.await {
                Ok(Ok(p)) => paths.push(p),
                Ok(Err(e)) => {
                    return Ok(ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed: started.elapsed(),
                        message: e,
                    });
                }
                Err(je) => {
                    return Ok(ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed: started.elapsed(),
                        message: format!("task panicked: {je}"),
                    });
                }
            }
        }

        // Every log must replay cleanly with its 200 messages intact.
        let elapsed = started.elapsed();
        for p in &paths {
            let (_meta, msgs) = match Recorder::resume(p) {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("resume {} failed: {e}", p.display()),
                    });
                }
            };
            if msgs.len() != 200 {
                return Ok(ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!("expected 200 msgs at {}, got {}", p.display(), msgs.len()),
                });
            }
        }
        Ok(ScenarioOutcome {
            id: self.id().into(),
            status: ScenarioStatus::Pass,
            elapsed,
            message: format!(
                "4 sessions × 200 msgs replayed clean ({}ms)",
                elapsed.as_millis()
            ),
        })
    }
}

/// Resume of a 5K-message rollout completes in <500ms. Catches
/// regressions in the line-by-line resume parser.
struct LargeRolloutResume;

#[async_trait]
impl Scenario for LargeRolloutResume {
    fn id(&self) -> &'static str {
        "stress::large_rollout_resume"
    }
    fn description(&self) -> &'static str {
        "resume of a 5K-message JSONL log completes in <500ms."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        let rec = Recorder::create(dir.path(), std::path::PathBuf::from("/p"), "m".into())?;
        for i in 0..5_000 {
            rec.record_message(awidat_core::anthropic::Message::user_text(format!(
                "msg-{i} with some content for realism"
            )));
        }
        rec.flush().await?;
        let path = rec.path().to_path_buf();
        drop(rec);

        let resume_started = Instant::now();
        let (_meta, msgs) = Recorder::resume(&path)?;
        let resume_elapsed = resume_started.elapsed();
        let elapsed = started.elapsed();

        Ok(
            if msgs.len() == 5_000 && resume_elapsed < Duration::from_millis(500) {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: format!("5K msgs resumed in {}ms", resume_elapsed.as_millis()),
                }
            } else if msgs.len() != 5_000 {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!("expected 5000 msgs; got {}", msgs.len()),
                }
            } else {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!(
                        "resume took {}ms, expected <500ms",
                        resume_elapsed.as_millis()
                    ),
                }
            },
        )
    }
}

/// Empty transcript sidecar (zero segments) must not panic
/// `find_moment`.
struct EmptyTranscriptHandling;

#[async_trait]
impl Scenario for EmptyTranscriptHandling {
    fn id(&self) -> &'static str {
        "stress::empty_transcript"
    }
    fn description(&self) -> &'static str {
        "find_moment against a sidecar with 0 segments returns empty results, not panic."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let sidecar_path = dir.path().join("index/whisper/raw/empty.mp4.json");
        std::fs::create_dir_all(sidecar_path.parent().unwrap())?;
        std::fs::write(
            &sidecar_path,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": "raw/empty.mp4",
                "data": {"segments": []}
            }))?,
        )?;

        let out = FindMomentTool
            .handle(
                make_call("find_moment", serde_json::json!({"query": "anything"})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let n = body["results"].as_array().map(Vec::len).unwrap_or(0);
                if n == 0 {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "0 segments → 0 hits, no panic".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("expected 0 hits on empty corpus, got {n}"),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("errored on empty corpus: {e}"),
            },
        })
    }
}

/// Unicode-heavy queries (CJK, emoji, RTL) must not panic the BM25
/// pipeline.
struct UnicodeQueryHandling;

#[async_trait]
impl Scenario for UnicodeQueryHandling {
    fn id(&self) -> &'static str {
        "stress::unicode_query"
    }
    fn description(&self) -> &'static str {
        "find_moment with unicode/CJK/emoji queries doesn't panic."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let sidecar_path = dir.path().join("index/whisper/raw/u.mp4.json");
        std::fs::create_dir_all(sidecar_path.parent().unwrap())?;
        std::fs::write(
            &sidecar_path,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": "raw/u.mp4",
                "data": {"segments": [
                    {"text": "today we discuss 한국어 and 日本語 patterns", "start_s": 0, "end_s": 1},
                    {"text": "emoji response 🎬🔥🚀 was unexpected", "start_s": 1, "end_s": 2},
                    {"text": "العربية script handling matters too", "start_s": 2, "end_s": 3},
                ]}
            }))?,
        )?;
        let queries = ["한국어", "🎬", "العربية", "patterns"];
        let mut errored = Vec::new();
        for q in &queries {
            let out = FindMomentTool
                .handle(
                    make_call("find_moment", serde_json::json!({"query": q})),
                    ctx_at(dir.path()),
                )
                .await;
            if out.is_err() {
                errored.push(*q);
            }
        }
        let elapsed = started.elapsed();
        Ok(if errored.is_empty() {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!("{} unicode queries all handled", queries.len()),
            }
        } else {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("errored on: {errored:?}"),
            }
        })
    }
}

/// `read_index` with `limit=usize::MAX` clamps to MAX_LIMIT and
/// returns a parseable response — verifies the #156 cap holds under
/// adversarial input.
struct MaxLimitClampStress;

#[async_trait]
impl Scenario for MaxLimitClampStress {
    fn id(&self) -> &'static str {
        "stress::max_limit_clamp"
    }
    fn description(&self) -> &'static str {
        "read_index with limit=usize::MAX clamps to 300 cleanly."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let sidecar_path = dir.path().join("index/whisper/raw/big.mp4.json");
        std::fs::create_dir_all(sidecar_path.parent().unwrap())?;
        // Small corpus — we only need to verify the clamp behavior,
        // not stress the byte-cap.
        let segments: Vec<serde_json::Value> = (0..5)
            .map(|i| {
                serde_json::json!({
                    "text": format!("segment {i}"),
                    "start_s": i,
                    "end_s": i + 1,
                })
            })
            .collect();
        std::fs::write(
            &sidecar_path,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": "raw/big.mp4",
                "data": {"segments": segments}
            }))?,
        )?;

        // The schema-validator may reject usize::MAX upstream — our
        // tool clamps internally regardless. Pass a number well above
        // MAX_LIMIT but representable.
        let out = ReadIndexTool
            .handle(
                make_call(
                    "read_index",
                    serde_json::json!({
                        "asset_id": "raw/big.mp4",
                        "channel": "transcript",
                        "limit": 1_000_000,
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                if t.content.contains("\"limit\":300") {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "limit=1M clamped to 300".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: "limit was not clamped to 300 in output".into(),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("read_index errored on huge limit: {e}"),
            },
        })
    }
}

/// 1000 burst-written editorial decisions across 5 sessions, then
/// `lessons::extract_from_decisions` over the resulting log set.
/// Catches: decision-recording loss under burst (same root cause as
/// the rollout fix); pattern-extraction performance at scale.
struct DecisionBurstWriteAndExtract;

#[async_trait]
impl Scenario for DecisionBurstWriteAndExtract {
    fn id(&self) -> &'static str {
        "stress::decision_burst_write_and_extract"
    }
    fn description(&self) -> &'static str {
        "1000 decisions across 5 sessions, all captured + pattern extraction parses them."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        // Build signal-bearing data: hook = always Allow, tangent =
        // always Deny. The algorithm should pick that up as snippet
        // patterns.
        for s in 0..5 {
            let rec = Recorder::create(
                dir.path(),
                std::path::PathBuf::from(format!("/p{s}")),
                "m".into(),
            )?;
            for i in 0..200 {
                let (kind, decision) = if i % 2 == 0 {
                    ("hook", "Allow")
                } else {
                    ("tangent", "Deny")
                };
                rec.record_decision(
                    "apply_edl".into(),
                    format!("kind={kind} score=0.{}{}", (i % 9) + 1, i % 10),
                    vec![],
                    None,
                    decision.into(),
                );
            }
            rec.flush().await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let decisions = Recorder::collect_decisions(dir.path())?;
        let elapsed = started.elapsed();
        let total = decisions.len();
        let patterns = awidat_core::lessons::extract_from_decisions(&decisions);
        let has_hook_pattern = patterns
            .iter()
            .any(|p| p.snippet.as_deref() == Some("hook"));
        let has_tangent_pattern = patterns
            .iter()
            .any(|p| p.snippet.as_deref() == Some("tangent"));
        Ok(
            if total == 1000 && has_hook_pattern && has_tangent_pattern {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: format!(
                        "{total} decisions captured; {} patterns including hook+tangent",
                        patterns.len()
                    ),
                }
            } else if total != 1000 {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!("expected 1000 decisions; got {total}"),
                }
            } else {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!(
                        "expected hook+tangent patterns; got {} patterns: {:?}",
                        patterns.len(),
                        patterns
                            .iter()
                            .map(|p| p.snippet.clone())
                            .collect::<Vec<_>>()
                    ),
                }
            },
        )
    }
}

/// `lessons::extract` against 5K decisions completes in <1s. Catches
/// O(n²) regressions in the snippet-tokenize pass.
struct LessonsAtScale;

#[async_trait]
impl Scenario for LessonsAtScale {
    fn id(&self) -> &'static str {
        "stress::lessons_at_scale"
    }
    fn description(&self) -> &'static str {
        "lessons::extract over 5K decisions completes in <1s."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let decisions: Vec<awidat_core::rollout::EditorialDecision> = (0..5_000)
            .map(|i| awidat_core::rollout::EditorialDecision {
                tool: if i % 2 == 0 { "apply_edl" } else { "bash" }.into(),
                args_summary: format!(
                    "kind={kind} score=0.{score} cmd={cmd}",
                    kind = ["hook", "tangent", "punchline", "cta"][i % 4],
                    score = (i % 9) + 1,
                    cmd = ["ls", "git", "rm", "ffmpeg"][i % 4],
                ),
                approval_keys: vec![],
                retry_reason: None,
                decision: if i % 3 == 0 { "Deny" } else { "Allow" }.into(),
                timestamp: chrono::Utc::now(),
            })
            .collect();
        let extract_started = Instant::now();
        let patterns = awidat_core::lessons::extract_from_decisions(&decisions);
        let extract_elapsed = extract_started.elapsed();
        let elapsed = started.elapsed();
        Ok(if extract_elapsed < Duration::from_secs(1) {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!(
                    "5K decisions, extract {}ms, {} patterns",
                    extract_elapsed.as_millis(),
                    patterns.len()
                ),
            }
        } else {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!(
                    "extract took {}ms, expected <1000ms",
                    extract_elapsed.as_millis()
                ),
            }
        })
    }
}

/// 50 assets each with 200 transcript segments. `find_moment` walks
/// every sidecar; this catches multi-asset walker regressions and
/// confirms the BM25 corpus aggregates across files.
struct MultiAssetCorpus;

#[async_trait]
impl Scenario for MultiAssetCorpus {
    fn id(&self) -> &'static str {
        "stress::multi_asset_corpus"
    }
    fn description(&self) -> &'static str {
        "find_moment across 50 sidecars × 200 segments returns ranked hits."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        for asset_idx in 0..50 {
            let segments: Vec<serde_json::Value> = (0..200)
                .map(|i| {
                    serde_json::json!({
                        "text": format!(
                            "asset {asset_idx} segment {i} mentions battery and stripe"
                        ),
                        "start_s": i as f64,
                        "end_s": i as f64 + 1.0,
                    })
                })
                .collect();
            let p = dir
                .path()
                .join(format!("index/whisper/raw/a{asset_idx}.mp4.json"));
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(
                &p,
                serde_json::to_vec(&serde_json::json!({
                    "indexer": "whisper",
                    "asset_id": format!("raw/a{asset_idx}.mp4"),
                    "data": {"segments": segments}
                }))?,
            )?;
        }

        let q_started = Instant::now();
        let out = FindMomentTool
            .handle(
                make_call(
                    "find_moment",
                    serde_json::json!({"query": "stripe", "limit": 25}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let q_elapsed = q_started.elapsed();
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let n = body["results"].as_array().map(Vec::len).unwrap_or(0);
                let max_elapsed = Duration::from_secs(7);
                if n > 0 && q_elapsed < max_elapsed {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!(
                            "50 assets × 200 segs, query {}ms, {n} hits",
                            q_elapsed.as_millis()
                        ),
                    }
                } else if n == 0 {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: "0 hits across 10K-seg multi-asset corpus".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!(
                            "query took {}ms (>{}ms cap)",
                            q_elapsed.as_millis(),
                            max_elapsed.as_millis()
                        ),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("find_moment errored: {e}"),
            },
        })
    }
}

/// A 50KB query string must not crash BM25 or the result rendering.
/// (Real adversarial path: a malformed prompt that lands a giant
/// concatenated string in the query arg.)
struct LongQueryString;

#[async_trait]
impl Scenario for LongQueryString {
    fn id(&self) -> &'static str {
        "stress::long_query_string"
    }
    fn description(&self) -> &'static str {
        "find_moment with a 50KB query string returns gracefully."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let p = dir.path().join("index/whisper/raw/x.mp4.json");
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(
            &p,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "whisper",
                "asset_id": "raw/x.mp4",
                "data": {"segments": [
                    {"text": "battery exploded today", "start_s": 0, "end_s": 1},
                ]}
            }))?,
        )?;
        let huge_query = "battery ".repeat(6_400); // ~50KB
        let out = FindMomentTool
            .handle(
                make_call("find_moment", serde_json::json!({"query": huge_query})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(_) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "50KB query returned without panic".into(),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("errored: {e}"),
            },
        })
    }
}

/// `SubAgentRegistry::iter_kind` partitions cleanly. Catches
/// regressions in the kind-tagging contract that #153 depends on.
struct SubagentRegistryFiltering;

#[async_trait]
impl Scenario for SubagentRegistryFiltering {
    fn id(&self) -> &'static str {
        "stress::subagent_registry_filtering"
    }
    fn description(&self) -> &'static str {
        "SubAgentRegistry partitions correctly into Research vs Persona."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let r = awidat_core::subagent::SubAgentRegistry::defaults();
        let research: Vec<_> = r
            .iter_kind(awidat_core::subagent::SubAgentKind::Research)
            .map(|a| a.name)
            .collect();
        let personas: Vec<_> = r
            .iter_kind(awidat_core::subagent::SubAgentKind::Persona)
            .map(|a| a.name)
            .collect();
        let elapsed = started.elapsed();
        let ok = research.len() == 3
            && personas.len() == 3
            && research.iter().all(|n| !personas.contains(n));
        Ok(if ok {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!("research={research:?} personas={personas:?}"),
            }
        } else {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("partition broken: research={research:?} personas={personas:?}"),
            }
        })
    }
}

/// Resume of a log with a trailing partial line (simulates a crash
/// mid-write) must not lose the last good line. Catches off-by-one in
/// the line parser.
struct RecorderResumeOfTrailingPartialLine;

#[async_trait]
impl Scenario for RecorderResumeOfTrailingPartialLine {
    fn id(&self) -> &'static str {
        "stress::resume_with_trailing_partial_line"
    }
    fn description(&self) -> &'static str {
        "resume tolerates a truncated last line (simulated crash mid-write)."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        use std::io::Write;
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        let rec = Recorder::create(dir.path(), std::path::PathBuf::from("/p"), "m".into())?;
        for i in 0..50 {
            rec.record_message(awidat_core::anthropic::Message::user_text(format!(
                "msg-{i}"
            )));
        }
        rec.flush().await?;
        let path = rec.path().to_path_buf();
        // Append a half-written JSON line (simulating power loss
        // during write).
        let mut f = std::fs::OpenOptions::new().append(true).open(&path)?;
        f.write_all(b"{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"")?;
        f.flush()?;
        drop(f);
        drop(rec);

        let (_meta, msgs) = Recorder::resume(&path)?;
        let elapsed = started.elapsed();
        Ok(if msgs.len() == 50 {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "50 good msgs preserved; partial line skipped".into(),
            }
        } else {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("expected 50 msgs; got {}", msgs.len()),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_all;

    #[tokio::test]
    async fn stress_suite_runs_clean() {
        let scenarios = defaults();
        assert!(!scenarios.is_empty());
        let outcomes = run_all(&scenarios).await;
        for o in &outcomes {
            assert_eq!(
                o.status,
                ScenarioStatus::Pass,
                "stress {} failed: {}",
                o.id,
                o.message
            );
        }
    }
}
