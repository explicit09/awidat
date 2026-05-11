//! V1 scenario suite.
//!
//! Each scenario is a self-contained struct implementing [`Scenario`].
//! Scenarios live in this single file (not a folder-per-scenario) for
//! V1 because the count is small (~3) and tight coupling between the
//! shared fixture helpers and each scenario keeps reading easy. When
//! the count crosses ~10 we split.
//!
//! V1 scenarios are **structural**: they exercise tool dispatch and
//! state mutation against a fresh project, without an LLM in the loop.
//! Editorial-quality scenarios (with the model deciding the cuts) need
//! API budget + golden cuts and land alongside #149.

use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

use awidat_core::tool::{ApprovalCache, ApprovalDecision, ApprovalKey, SandboxMode, ToolHandler};
use awidat_core::tools::{
    find_beat::FindBeatTool, find_moment::FindMomentTool, list_assets::ListAssetsTool,
    read_index::ReadIndexTool, view_episode::ViewEpisodeTool, view_timeline::ViewTimelineTool,
};
use awidat_proto::project::Project;

use crate::{Scenario, ScenarioOutcome, ScenarioStatus};

/// Build the default V1 scenario list. Add new scenarios here. The
/// CLI runner takes a slice of `Box<dyn Scenario>` so wiring a new
/// one in is a one-liner.
///
/// Scenarios opportunistically exercising `$AWIDAT_REAL_PROJECT`
/// (a fully-indexed project on disk) self-skip when the env is
/// unset. Used in Week 8 demo bringup against
/// `/tmp/awidat-real/yt-test` and similar.
pub fn defaults() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(FreshProjectViewEpisode),
        Box::new(EmptyProjectListAssets),
        Box::new(EmptyTimelineViewTimeline),
        Box::new(OrchestratorApprovalCacheIsOperationKeyed),
        Box::new(OrchestratorSandboxDenialEscalates),
        Box::new(OrchestratorNoSilentUnsandboxedRetry),
        Box::new(RolloutApprovalDecisionMetadata),
        Box::new(RealProjectViewEpisode),
        Box::new(RealProjectFindMoment),
        Box::new(RealProjectFindBeat),
        Box::new(RealProjectReadIndexTranscript),
    ]
}

// ---------- helpers ----------

pub(crate) fn ctx_at(root: &std::path::Path) -> awidat_core::tool::ToolContext {
    let (tx, _) = tokio::sync::broadcast::channel(8);
    awidat_core::tool::ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: awidat_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: awidat_core::tool::SandboxMode::Default,
        mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
            name: "eval".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }),
        skills: Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

pub(crate) fn make_call(name: &str, args: serde_json::Value) -> awidat_core::tool::ToolInvocation {
    awidat_core::tool::ToolInvocation {
        call_id: "eval-1".into(),
        name: name.into(),
        args,
    }
}

// ---------- scenarios ----------

/// Fresh project + `view_episode` returns a shape the agent can render.
/// Catches: tool schema regressions; `Project::init` regressions;
/// episode-map empty-state handling.
struct FreshProjectViewEpisode;

#[async_trait]
impl Scenario for FreshProjectViewEpisode {
    fn id(&self) -> &'static str {
        "structural::view_episode_on_fresh_project"
    }
    fn description(&self) -> &'static str {
        "Fresh `awidat init` + view_episode returns non-empty text without panicking."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let out = ViewEpisodeTool
            .handle(
                make_call("view_episode", serde_json::json!({})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let outcome = match out {
            Ok(t) if !t.content.is_empty() => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!("returned {} chars", t.content.len()),
            },
            Ok(_) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: "view_episode returned empty content".into(),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("view_episode errored: {e}"),
            },
        };
        Ok(outcome)
    }
}

/// `list_assets` on an empty project returns the no-assets pagination
/// header (not an error). Catches: list_assets crashing on no-`raw/`.
struct EmptyProjectListAssets;

#[async_trait]
impl Scenario for EmptyProjectListAssets {
    fn id(&self) -> &'static str {
        "structural::list_assets_on_empty"
    }
    fn description(&self) -> &'static str {
        "list_assets handles empty project gracefully."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let out = ListAssetsTool
            .handle(
                make_call("list_assets", serde_json::json!({})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let outcome = match out {
            Ok(t) => {
                // Either includes "total=0" or matches our pagination
                // header. Both are valid empty-state outputs.
                if t.content.contains("total=0") || t.content.contains("scope=") {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "empty-state header rendered".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("list_assets errored: {e}"),
            },
        };
        Ok(outcome)
    }
}

/// `view_timeline` on a fresh project (empty timeline) returns the
/// no-clips message rather than panicking. Catches: timeline empty-
/// window edge case.
struct EmptyTimelineViewTimeline;

#[async_trait]
impl Scenario for EmptyTimelineViewTimeline {
    fn id(&self) -> &'static str {
        "structural::view_timeline_on_empty"
    }
    fn description(&self) -> &'static str {
        "view_timeline handles empty timeline without panicking."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let out = ViewTimelineTool
            .handle(
                make_call("view_timeline", serde_json::json!({})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let outcome = match out {
            Ok(t) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!("returned {} chars", t.content.len()),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("view_timeline errored: {e}"),
            },
        };
        Ok(outcome)
    }
}

/// Operation-keyed approval caching: "allow for session" on one
/// operation must not approve a different operation for the same tool.
struct OrchestratorApprovalCacheIsOperationKeyed;

struct EvalMutatingTool;

#[async_trait]
impl ToolHandler for EvalMutatingTool {
    fn name(&self) -> &'static str {
        "eval_mutating"
    }

    fn schema(&self) -> awidat_core::anthropic::Tool {
        awidat_core::anthropic::Tool {
            name: "eval_mutating".into(),
            description: "eval-only mutating tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        }
    }

    fn approval_keys(&self, invocation: &awidat_core::tool::ToolInvocation) -> Vec<ApprovalKey> {
        vec![ApprovalKey::new(
            invocation.name.clone(),
            invocation.args["operation"].as_str().unwrap_or("missing"),
        )]
    }

    async fn handle(
        &self,
        invocation: awidat_core::tool::ToolInvocation,
        _ctx: awidat_core::tool::ToolContext,
    ) -> Result<awidat_core::tool::ToolOutput, awidat_core::FunctionCallError> {
        Ok(awidat_core::tool::ToolOutput::text(format!(
            "ran {}",
            invocation.args["operation"]
        )))
    }
}

#[async_trait]
impl Scenario for OrchestratorApprovalCacheIsOperationKeyed {
    fn id(&self) -> &'static str {
        "runtime::orchestrator_operation_keyed_approval_cache"
    }
    fn description(&self) -> &'static str {
        "Allow-for-session approval is cached by operation key, not just tool name."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::channel(8);
        let orchestrator = awidat_core::orchestrator::ToolOrchestrator::new(
            Some(approval_tx),
            Arc::new(tokio::sync::Mutex::new(ApprovalCache::default())),
            None,
        );
        let handler: Arc<dyn ToolHandler> = Arc::new(EvalMutatingTool);
        let cancel = tokio_util::sync::CancellationToken::new();

        let approvals = tokio::spawn(async move {
            let mut seen = Vec::new();
            if let Some(req) = approval_rx.recv().await {
                seen.push(req.args_summary.clone());
                let _ = req.reply.send(ApprovalDecision::AllowForSession);
            }
            if let Some(req) = approval_rx.recv().await {
                seen.push(req.args_summary.clone());
                let _ = req.reply.send(ApprovalDecision::Allow);
            }
            seen
        });

        for op in ["alpha", "alpha", "beta"] {
            let out = orchestrator
                .run(
                    handler.clone(),
                    make_call("eval_mutating", serde_json::json!({"operation": op})),
                    ctx_at(dir.path()),
                    &cancel,
                )
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if !out.content.contains(op) {
                return Ok(ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed: started.elapsed(),
                    message: format!("unexpected output for {op}: {}", out.content),
                });
            }
        }

        let seen = approvals.await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let elapsed = started.elapsed();
        Ok(
            if seen.len() == 2 && seen[0].contains("alpha") && seen[1].contains("beta") {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: "prompted for alpha once and beta once".into(),
                }
            } else {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!("expected two operation-keyed prompts, got {seen:?}"),
                }
            },
        )
    }
}

/// Sandbox-capable tools can report a denial; the orchestrator asks for
/// explicit escalation and retries with sandbox bypassed.
struct OrchestratorSandboxDenialEscalates;
struct OrchestratorNoSilentUnsandboxedRetry;
struct RolloutApprovalDecisionMetadata;

struct EvalSandboxedTool;

#[async_trait]
impl ToolHandler for EvalSandboxedTool {
    fn name(&self) -> &'static str {
        "eval_sandboxed"
    }

    fn schema(&self) -> awidat_core::anthropic::Tool {
        awidat_core::anthropic::Tool {
            name: "eval_sandboxed".into(),
            description: "eval-only sandboxed tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        }
    }

    fn approval_keys(&self, _invocation: &awidat_core::tool::ToolInvocation) -> Vec<ApprovalKey> {
        vec![ApprovalKey::new("eval_sandboxed", "write:/outside-project")]
    }

    fn sandbox_policy(
        &self,
        _invocation: &awidat_core::tool::ToolInvocation,
    ) -> awidat_core::tool::ToolSandboxPolicy {
        awidat_core::tool::ToolSandboxPolicy::SandboxFirst {
            escalate_on_denial: true,
        }
    }

    async fn handle(
        &self,
        _invocation: awidat_core::tool::ToolInvocation,
        ctx: awidat_core::tool::ToolContext,
    ) -> Result<awidat_core::tool::ToolOutput, awidat_core::FunctionCallError> {
        match ctx.sandbox_mode {
            SandboxMode::Default => Err(awidat_core::FunctionCallError::SandboxDenied {
                reason: "write outside project root".into(),
            }),
            SandboxMode::Bypass => Ok(awidat_core::tool::ToolOutput::text(
                "retried without sandbox",
            )),
        }
    }
}

#[async_trait]
impl Scenario for OrchestratorSandboxDenialEscalates {
    fn id(&self) -> &'static str {
        "runtime::orchestrator_sandbox_denial_escalates"
    }
    fn description(&self) -> &'static str {
        "Sandbox denial reports cleanly and can retry unsandboxed after approval."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let (approval_tx, mut approval_rx) = tokio::sync::mpsc::channel(8);
        let orchestrator = awidat_core::orchestrator::ToolOrchestrator::new(
            Some(approval_tx),
            Arc::new(tokio::sync::Mutex::new(ApprovalCache::default())),
            None,
        );
        let handler: Arc<dyn ToolHandler> = Arc::new(EvalSandboxedTool);
        let cancel = tokio_util::sync::CancellationToken::new();
        let approvals = tokio::spawn(async move {
            let mut summaries = Vec::new();
            while let Some(req) = approval_rx.recv().await {
                summaries.push(req.args_summary.clone());
                let _ = req.reply.send(ApprovalDecision::Allow);
                if summaries.len() == 2 {
                    break;
                }
            }
            summaries
        });

        let out = orchestrator
            .run(
                handler,
                make_call(
                    "eval_sandboxed",
                    serde_json::json!({"path": "/etc/blocked"}),
                ),
                ctx_at(dir.path()),
                &cancel,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let summaries = approvals.await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let elapsed = started.elapsed();
        Ok(
            if out.content.contains("retried without sandbox")
                && summaries.len() == 2
                && summaries[1].contains("sandbox denied")
            {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: "denial prompted for unsandboxed retry".into(),
                }
            } else {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!(
                        "unexpected output/summaries: {} / {summaries:?}",
                        out.content
                    ),
                }
            },
        )
    }
}

#[async_trait]
impl Scenario for OrchestratorNoSilentUnsandboxedRetry {
    fn id(&self) -> &'static str {
        "runtime::orchestrator_no_silent_unsandboxed_retry"
    }
    fn description(&self) -> &'static str {
        "Sandbox denial is returned to the model when no explicit approval channel exists."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        let orchestrator = awidat_core::orchestrator::ToolOrchestrator::new(
            None,
            Arc::new(tokio::sync::Mutex::new(ApprovalCache::default())),
            None,
        );
        let handler: Arc<dyn ToolHandler> = Arc::new(EvalSandboxedTool);
        let cancel = tokio_util::sync::CancellationToken::new();
        let out = orchestrator
            .run(
                handler,
                make_call(
                    "eval_sandboxed",
                    serde_json::json!({"path": "/etc/blocked"}),
                ),
                ctx_at(dir.path()),
                &cancel,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let elapsed = started.elapsed();
        Ok(match out {
            Err(awidat_core::FunctionCallError::SandboxDenied { reason })
                if reason.contains("outside project") =>
            {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: "denial returned without bypass retry".into(),
                }
            }
            other => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("expected SandboxDenied without retry, got {other:?}"),
            },
        })
    }
}

#[async_trait]
impl Scenario for RolloutApprovalDecisionMetadata {
    fn id(&self) -> &'static str {
        "runtime::rollout_approval_decision_metadata"
    }
    fn description(&self) -> &'static str {
        "Rollout logs approval operation keys and retry reasons for replay/debugging."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        let rec = awidat_core::rollout::Recorder::create(
            dir.path(),
            std::path::PathBuf::from("/project"),
            "m".into(),
        )?;
        rec.record_decision(
            "bash".into(),
            "touch /tmp/outside".into(),
            vec![ApprovalKey::new(
                "bash",
                "command:/project:touch /tmp/outside:unsandboxed",
            )],
            Some("sandbox denied 'bash': write outside project. Retry unsandboxed?".into()),
            "Deny".into(),
        );
        rec.flush().await?;
        let decisions = awidat_core::rollout::Recorder::collect_decisions(dir.path())?;
        let elapsed = started.elapsed();
        let ok = decisions.len() == 1
            && decisions[0]
                .retry_reason
                .as_deref()
                .is_some_and(|s| s.contains("sandbox denied"))
            && decisions[0]
                .approval_keys
                .iter()
                .any(|k| k.operation.contains("unsandboxed"));
        Ok(if ok {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "approval metadata round-tripped".into(),
            }
        } else {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("unexpected decisions: {decisions:?}"),
            }
        })
    }
}

// ---------- real-project scenarios ----------
//
// These exercise the read-tool surface against an actually-indexed
// project on disk (`$AWIDAT_REAL_PROJECT`). They self-skip when the
// env is unset so the suite stays green on a fresh checkout.

fn real_project_root() -> Option<std::path::PathBuf> {
    std::env::var("AWIDAT_REAL_PROJECT")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
}

fn skip_no_real_project(id: &str, started: Instant) -> ScenarioOutcome {
    ScenarioOutcome {
        id: id.into(),
        status: ScenarioStatus::Skipped,
        elapsed: started.elapsed(),
        message: "AWIDAT_REAL_PROJECT not set or not a dir; skipping".into(),
    }
}

/// `view_episode` against a real, fully-indexed project. Catches
/// regressions in episode-map rendering on real timelines (vs. the
/// fresh-init empty case).
struct RealProjectViewEpisode;

#[async_trait]
impl Scenario for RealProjectViewEpisode {
    fn id(&self) -> &'static str {
        "real::view_episode"
    }
    fn description(&self) -> &'static str {
        "view_episode against $AWIDAT_REAL_PROJECT renders a non-trivial map."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        let out = ViewEpisodeTool
            .handle(
                make_call("view_episode", serde_json::json!({})),
                ctx_at(&root),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) if t.content.len() > 100 => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!("rendered {} chars of map", t.content.len()),
            },
            Ok(t) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!(
                    "expected substantial map (>100 chars); got {} chars",
                    t.content.len()
                ),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("view_episode errored: {e}"),
            },
        })
    }
}

/// BM25 `find_moment` against the real whisper transcript with a
/// generic stop-word query that should match many segments. Catches
/// regressions in BM25 wiring against real corpus shape.
struct RealProjectFindMoment;

#[async_trait]
impl Scenario for RealProjectFindMoment {
    fn id(&self) -> &'static str {
        "real::find_moment_returns_hits"
    }
    fn description(&self) -> &'static str {
        "find_moment against the real transcript returns ≥1 hit for a content query."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        // Try several content-bearing queries. BM25's `Language::English`
        // strips stopwords (`the`, `and`, `is`, …) — that's correct
        // behavior, not a bug, but it means the test query has to be
        // a real content word. At least one of these should match any
        // non-trivial transcript; if all five miss, that's a wiring
        // regression worth flagging.
        let mut hits = 0usize;
        let mut best_query = String::new();
        for q in ["video", "people", "thing", "really", "one"] {
            let out = FindMomentTool
                .handle(
                    make_call("find_moment", serde_json::json!({"query": q, "limit": 5})),
                    ctx_at(&root),
                )
                .await;
            if let Ok(t) = out {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let n = body["results"].as_array().map(Vec::len).unwrap_or(0);
                if n > hits {
                    hits = n;
                    best_query = q.into();
                }
                if n > 0 {
                    break;
                }
            }
        }
        let elapsed = started.elapsed();
        Ok(if hits > 0 {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: format!("query='{best_query}' → {hits} hits"),
            }
        } else {
            ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: "no content query matched any transcript segment".into(),
            }
        })
    }
}

/// `find_beat` against the real editorial-moments index. Catches
/// regressions in the editorial-moments sidecar shape contract.
struct RealProjectFindBeat;

#[async_trait]
impl Scenario for RealProjectFindBeat {
    fn id(&self) -> &'static str {
        "real::find_beat_runs"
    }
    fn description(&self) -> &'static str {
        "find_beat against the real editorial-moments index returns a parseable response."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        // No `kind` filter — just verify the tool runs end-to-end.
        let out = FindBeatTool
            .handle(make_call("find_beat", serde_json::json!({})), ctx_at(&root))
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let parsed: Result<serde_json::Value, _> = serde_json::from_str(&t.content);
                if parsed.is_ok() {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!("returned {} chars of valid JSON", t.content.len()),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: "find_beat returned non-JSON content".into(),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("find_beat errored: {e}"),
            },
        })
    }
}

/// `read_index(channel="transcript")` end-to-end. Verifies the
/// MAX_LIMIT cap (#156) doesn't break real-corpus reads.
struct RealProjectReadIndexTranscript;

#[async_trait]
impl Scenario for RealProjectReadIndexTranscript {
    fn id(&self) -> &'static str {
        "real::read_index_transcript"
    }
    fn description(&self) -> &'static str {
        "read_index transcript channel against real corpus returns segments."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        // Find an asset id from the project's manifest by scanning raw/.
        let raw_dir = root.join("raw");
        let asset_id = match std::fs::read_dir(&raw_dir) {
            Ok(entries) => entries.filter_map(|e| e.ok()).find_map(|e| {
                e.path()
                    .file_name()
                    .map(|n| format!("raw/{}", n.to_string_lossy()))
            }),
            Err(_) => None,
        };
        let Some(asset_id) = asset_id else {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: "no raw/ assets found in real project".into(),
            });
        };
        let out = ReadIndexTool
            .handle(
                make_call(
                    "read_index",
                    serde_json::json!({
                        "asset_id": asset_id,
                        "channel": "transcript",
                        "limit": 10,
                    }),
                ),
                ctx_at(&root),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let n = body["segments"].as_array().map(Vec::len).unwrap_or(0);
                if n > 0 {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!("read {n} transcript segments for {asset_id}"),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("0 segments for {asset_id}"),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("read_index errored: {e}"),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_all;

    #[tokio::test]
    async fn defaults_runs_clean_on_fresh_machine() {
        // Real-project scenarios self-skip without `AWIDAT_REAL_PROJECT`,
        // so on a fresh checkout the structural ones pass and the
        // real:: ones skip — neither should fail.
        let scenarios = defaults();
        assert!(!scenarios.is_empty());
        let outcomes = run_all(&scenarios).await;
        for o in &outcomes {
            assert!(
                matches!(o.status, ScenarioStatus::Pass | ScenarioStatus::Skipped),
                "scenario {} must Pass or Skip; got Fail: {}",
                o.id,
                o.message,
            );
        }
        let pass = outcomes
            .iter()
            .filter(|o| o.status == ScenarioStatus::Pass)
            .count();
        assert!(pass >= 3, "the 3 structural scenarios must always pass");
    }

    #[tokio::test]
    async fn fresh_project_view_episode_passes() {
        let outcome = FreshProjectViewEpisode.run().await.unwrap();
        assert_eq!(outcome.status, ScenarioStatus::Pass);
    }
}
