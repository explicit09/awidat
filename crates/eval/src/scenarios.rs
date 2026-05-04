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

use awidat_core::tool::ToolHandler;
use awidat_core::tools::{
    list_assets::ListAssetsTool, view_episode::ViewEpisodeTool,
    view_timeline::ViewTimelineTool,
};
use awidat_proto::project::Project;

use crate::{Scenario, ScenarioOutcome, ScenarioStatus};

/// Build the default V1 scenario list. Add new scenarios here. The
/// CLI runner takes a slice of `Box<dyn Scenario>` so wiring a new
/// one in is a one-liner.
pub fn defaults() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(FreshProjectViewEpisode),
        Box::new(EmptyProjectListAssets),
        Box::new(EmptyTimelineViewTimeline),
    ]
}

// ---------- helpers ----------

fn ctx_at(root: &std::path::Path) -> awidat_core::tool::ToolContext {
    let (tx, _) = tokio::sync::broadcast::channel(8);
    awidat_core::tool::ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: awidat_render::JobManager::new(),
        approval_tx: None,
        mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
            name: "eval".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }),
        skills: Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn make_call(name: &str, args: serde_json::Value) -> awidat_core::tool::ToolInvocation {
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
            .handle(make_call("view_episode", serde_json::json!({})), ctx_at(dir.path()))
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
            .handle(make_call("list_assets", serde_json::json!({})), ctx_at(dir.path()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_all;

    #[tokio::test]
    async fn defaults_runs_clean_on_fresh_machine() {
        let scenarios = defaults();
        assert!(!scenarios.is_empty());
        let outcomes = run_all(&scenarios).await;
        for o in &outcomes {
            assert_eq!(
                o.status,
                ScenarioStatus::Pass,
                "scenario {} expected to pass; failed: {}",
                o.id,
                o.message
            );
        }
    }

    #[tokio::test]
    async fn fresh_project_view_episode_passes() {
        let outcome = FreshProjectViewEpisode.run().await.unwrap();
        assert_eq!(outcome.status, ScenarioStatus::Pass);
    }
}
