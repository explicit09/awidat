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
    apply_edl::ApplyEdlTool, find_beat::FindBeatTool,
    find_broll_opportunities::FindBrollOpportunitiesTool, find_dead_air::FindDeadAirTool,
    find_moment::FindMomentTool, list_assets::ListAssetsTool, read_index::ReadIndexTool,
    use_broll::UseBrollTool, vedit_diff::VeditDiffTool, vedit_revert::VeditRevertTool,
    view_episode::ViewEpisodeTool, view_timeline::ViewTimelineTool,
};
use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Timeline, Track,
    TrackChild, TrackKind,
};
use awidat_proto::project::Project;

use crate::fixtures::{
    ClipSpec, clip_count, clip_range_by_uuid, write_project_with_clips, write_silence_ranges,
    write_whisper_words,
};
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
    let mut out = fast();
    out.extend(product());
    out.extend(real_corpus());
    out
}

/// Fast deterministic offline evals for every PR.
pub fn fast() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(FreshProjectViewEpisode),
        Box::new(EmptyProjectListAssets),
        Box::new(EmptyTimelineViewTimeline),
        Box::new(EdlCoreOpsProposalDiffSurface),
        Box::new(SamePositionMovePreservesTimeline),
        Box::new(OrchestratorApprovalCacheIsOperationKeyed),
        Box::new(OrchestratorSandboxDenialEscalates),
        Box::new(OrchestratorNoSilentUnsandboxedRetry),
        Box::new(RolloutApprovalDecisionMetadata),
        Box::new(MinimalIndexerSidecarParsing),
    ]
}

/// Product-quality synthetic scenarios. Still offline, but tests
/// editorial behavior rather than only tool shape.
pub fn product() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(DeadAirTrimSuggestionQuality),
        Box::new(BrollOpportunityTranscriptAnchorQuality),
        Box::new(BrollUseThisConcreteAnchorSequence),
        Box::new(VeditDiffRestoreIsAudited),
        Box::new(VeditRevertRecoveryFlow),
    ]
}

/// Real corpus/API-gated scenarios. These skip unless explicitly
/// configured by environment.
pub fn real_corpus() -> Vec<Box<dyn Scenario>> {
    vec![
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

fn write_single_clip_project(
    root: &std::path::Path,
    name: &str,
    uuid: &str,
    transcript_snippet: &str,
) -> Result<()> {
    let mut project = Project::read(root)?;
    let mut timeline = Timeline::empty("eval");
    let mut track = Track::empty("V1", TrackKind::Video);
    let mut clip = Clip::empty(name.to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new("raw/main.mp4"));
    clip.source_range = Some(TimeRange::new(
        RationalTime::zero(24.0),
        RationalTime::new(5.0 * 24.0, 24.0),
    ));
    let meta = clip.metadata.awidat.get_or_insert_with(Default::default);
    meta.extra
        .insert("clip_uuid".into(), serde_json::Value::String(uuid.into()));
    meta.anchor = Some(awidat_proto::awidat_meta::Anchor {
        transcript_snippet: Some(transcript_snippet.into()),
        ..Default::default()
    });
    track.children.push(TrackChild::Clip(clip));
    timeline.tracks.children.push(StackChild::Track(track));
    project.timeline = timeline;
    project.write(root)?;
    Ok(())
}

fn write_named_empty_timeline(root: &std::path::Path, name: &str) -> Result<()> {
    let mut project = Project::read(root)?;
    project.timeline = Timeline::empty(name);
    project.write(root)?;
    Ok(())
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

/// Core edit ops should parse/apply and surface structural vedit
/// changes for the proposal/review path: trim, split, insert, b-roll,
/// PiP, move, and delete all live in one deterministic envelope.
struct EdlCoreOpsProposalDiffSurface;

#[async_trait]
impl Scenario for EdlCoreOpsProposalDiffSurface {
    fn id(&self) -> &'static str {
        "ci::edl_core_ops_proposal_diff_surface"
    }

    fn description(&self) -> &'static str {
        "EDL trim/delete/split/insert/b-roll/PiP/move apply cleanly and vedit reports structural changes."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "core-ops",
            &[
                ClipSpec::video("intro", "intro", "raw/intro.mp4", 0.0, 10.0)
                    .with_transcript("welcome back"),
                ClipSpec::video(
                    "false-start",
                    "false-start",
                    "raw/false-start.mp4",
                    0.0,
                    4.0,
                )
                .with_transcript("let me start over"),
                ClipSpec::video("demo", "demo", "raw/demo.mp4", 0.0, 8.0)
                    .with_transcript("look at the graph"),
            ],
        )?;
        crate::fixtures::write_asset(dir.path(), "raw/insert.mp4")?;
        crate::fixtures::write_asset(dir.path(), "raw/broll/skyline.mp4")?;
        crate::fixtures::write_asset(dir.path(), "raw/broll/pip.mp4")?;

        let repo = awidat_core::vc::open_or_init(dir.path())?;
        let first = awidat_core::vc::commit_current_timeline(&repo, "before core ops", None)?;
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: clip_uuid=intro
+ end: 8
*** Split Clip
@@ anchor: clip_uuid=demo
+ at_s: 3
*** Insert Clip
+ asset: raw/insert.mp4
+ track: V1
+ at_position: 1
+ start: 0
+ end: 2
+ name: inserted-cutaway
*** Insert BRoll
@@ anchor: clip_uuid=demo
+ asset: raw/broll/skyline.mp4
+ duration_s: 2
+ position: overlay
*** Insert PiP
@@ anchor: clip_uuid=intro
+ asset: raw/broll/pip.mp4
+ duration_s: 1.5
*** Move Clip
@@ anchor: clip_uuid=false-start
+ to_position: 0
*** Delete Clip
@@ anchor: clip_uuid=false-start
*** End EDL
";
        let apply = ApplyEdlTool
            .handle(
                make_call(
                    "apply_edl",
                    serde_json::json!({"edl": edl, "dry_run": false, "reasoning": "eval core op coverage"}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let Ok(output) = apply else {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("apply_edl errored: {:?}", apply.err()),
            });
        };
        let diff = VeditDiffTool
            .handle(
                make_call(
                    "vedit_diff",
                    serde_json::json!({"from": first.commit_hash, "to": "HEAD"}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        Ok(match diff {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let change_count = body["change_count"].as_u64().unwrap_or(0);
                if output.content.contains("committed")
                    && change_count >= 4
                    && body["changes"].to_string().contains("trimmed")
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!(
                            "applied multi-op proposal; vedit saw {change_count} changes"
                        ),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!(
                            "unexpected output/diff: {} / {}",
                            output.content, t.content
                        ),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("vedit_diff errored: {e:?}"),
            },
        })
    }
}

/// Dragging a clip and dropping it in its original position should not
/// mutate timeline order. This catches a subtle proposal adjustment
/// regression where same-position moves used to be collapsed wrongly.
struct SamePositionMovePreservesTimeline;

#[async_trait]
impl Scenario for SamePositionMovePreservesTimeline {
    fn id(&self) -> &'static str {
        "ci::same_position_move_preserves_timeline"
    }

    fn description(&self) -> &'static str {
        "Move Clip to its current position preserves clip count and source ranges."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "same-position-move",
            &[
                ClipSpec::video("a", "a", "raw/a.mp4", 0.0, 3.0),
                ClipSpec::video("b", "b", "raw/b.mp4", 2.0, 4.0),
            ],
        )?;
        let edl = "\
*** Begin EDL
*** Move Clip
@@ anchor: clip_uuid=b
+ to_position: 1
*** End EDL
";
        let apply = ApplyEdlTool
            .handle(
                make_call(
                    "apply_edl",
                    serde_json::json!({"edl": edl, "dry_run": false}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let project = Project::read(dir.path())?;
        let range = clip_range_by_uuid(&project.timeline, "b");
        Ok(match apply {
            Ok(_) if clip_count(&project.timeline) == 2 && range == Some((2.0, 4.0)) => {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: "same-position move preserved both clips and source range".into(),
                }
            }
            Ok(t) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!(
                    "unexpected timeline after move: {} range={range:?}",
                    t.content
                ),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("apply_edl errored: {e:?}"),
            },
        })
    }
}

/// Safe sidecar parsing: no model downloads, just the on-disk schema
/// the indexer sidecars promise to write.
struct MinimalIndexerSidecarParsing;

#[async_trait]
impl Scenario for MinimalIndexerSidecarParsing {
    fn id(&self) -> &'static str {
        "ci::minimal_indexer_sidecar_parsing"
    }

    fn description(&self) -> &'static str {
        "read_index parses a tiny synthetic whisper sidecar without invoking Python/model downloads."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "sidecar",
            &[ClipSpec::video("clip", "clip", "raw/clip.mp4", 0.0, 2.0)],
        )?;
        write_whisper_words(
            dir.path(),
            "raw/clip.mp4",
            &[("hello", 0.0, 0.4), ("world", 0.5, 1.0)],
        )?;
        let out = ReadIndexTool
            .handle(
                make_call(
                    "read_index",
                    serde_json::json!({"asset_id": "raw/clip.mp4", "channel": "transcript", "limit": 2}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) if t.content.contains("hello") && t.content.contains("world") => {
                ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Pass,
                    elapsed,
                    message: "synthetic transcript sidecar parsed".into(),
                }
            }
            Ok(t) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("unexpected read_index output: {}", t.content),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("read_index errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: a real dead-air finder should identify only the
/// long pause, preserve surrounding transcript context, and provide a
/// timeline/source anchor an agent can turn into a trim proposal.
struct DeadAirTrimSuggestionQuality;

#[async_trait]
impl Scenario for DeadAirTrimSuggestionQuality {
    fn id(&self) -> &'static str {
        "product::dead_air_trim_suggestion_quality"
    }

    fn description(&self) -> &'static str {
        "find_dead_air surfaces a long silence with transcript context and ignores breath beats."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "dead-air",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 12.0)],
        )?;
        write_silence_ranges(dir.path(), "raw/host.mp4", &[(1.0, 1.4), (5.0, 7.4)])?;
        write_whisper_words(
            dir.path(),
            "raw/host.mp4",
            &[
                ("before", 3.8, 4.1),
                ("pause", 4.2, 4.6),
                ("after", 7.5, 7.9),
                ("resume", 8.0, 8.4),
            ],
        )?;
        let out = FindDeadAirTool
            .handle(
                make_call("find_dead_air", serde_json::json!({"min_duration_s": 1.5})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let findings = body["findings"].as_array().cloned().unwrap_or_default();
                let first = findings.first().cloned().unwrap_or(serde_json::Value::Null);
                let duration_ok = first["duration_s"].as_f64().is_some_and(|d| d > 2.3);
                let context_ok = first["transcript_before"]
                    .as_str()
                    .unwrap_or("")
                    .contains("pause")
                    && first["transcript_after"]
                        .as_str()
                        .unwrap_or("")
                        .contains("after");
                if findings.len() == 1 && duration_ok && context_ok {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "surfaced one actionable long silence with context".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected dead-air findings: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("find_dead_air errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: transcript-aware b-roll suggestions should point
/// to concrete visual language instead of generic filler.
struct BrollOpportunityTranscriptAnchorQuality;

#[async_trait]
impl Scenario for BrollOpportunityTranscriptAnchorQuality {
    fn id(&self) -> &'static str {
        "product::broll_opportunity_transcript_anchor_quality"
    }

    fn description(&self) -> &'static str {
        "find_broll_opportunities emits a concrete query and transcript excerpt for a visual reference."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "broll-opportunity",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 10.0)],
        )?;
        write_whisper_words(
            dir.path(),
            "raw/host.mp4",
            &[
                ("now", 0.0, 0.2),
                ("imagine", 1.0, 1.3),
                ("a", 1.3, 1.4),
                ("busy", 1.5, 1.8),
                ("office", 1.9, 2.2),
                ("meeting", 2.3, 2.8),
            ],
        )?;
        let out = FindBrollOpportunitiesTool
            .handle(
                make_call("find_broll_opportunities", serde_json::json!({})),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let findings = body["findings"].as_array().cloned().unwrap_or_default();
                let first = findings.first().cloned().unwrap_or(serde_json::Value::Null);
                let query = first["pexels_query"].as_str().unwrap_or("");
                let reason = first["reason"].as_str().unwrap_or("");
                if findings.len() == 1
                    && query.contains("office")
                    && reason.contains("imagine")
                    && first["timeline_start_s"].as_f64().is_some()
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!("generated concrete b-roll query '{query}'"),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected b-roll findings: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("find_broll_opportunities errored: {e:?}"),
            },
        })
    }
}

/// B-roll "Use this" flow should carry a concrete anchor through
/// `use_broll` and produce an `apply_edl`-valid fragment.
struct BrollUseThisConcreteAnchorSequence;

#[async_trait]
impl Scenario for BrollUseThisConcreteAnchorSequence {
    fn id(&self) -> &'static str {
        "product::broll_use_this_concrete_anchor_sequence"
    }
    fn description(&self) -> &'static str {
        "use_broll with a concrete note anchor returns a non-placeholder EDL fragment that apply_edl validates."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        write_single_clip_project(dir.path(), "clip-0", "clip-0", "the city skyline")?;
        let broll_path = dir.path().join("raw/broll/pexels-42.mp4");
        std::fs::create_dir_all(
            broll_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("broll path has no parent"))?,
        )?;
        std::fs::write(&broll_path, b"placeholder")?;

        let out = UseBrollTool
            .handle(
                make_call(
                    "use_broll",
                    serde_json::json!({
                        "pexels_id": 42,
                        "anchor": { "transcript_snippet": "the city skyline" },
                        "duration_s": 2.0,
                        "position": "overlay"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let Ok(output) = out else {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("use_broll errored: {:?}", out.err()),
            });
        };
        let body: serde_json::Value = serde_json::from_str(&output.content)?;
        let Some(edl) = body["edl_fragment"].as_str() else {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: "use_broll did not return edl_fragment".into(),
            });
        };
        if edl.contains("...") || !edl.contains("transcript_snippet=\"the city skyline\"") {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("EDL fragment did not carry concrete anchor: {edl}"),
            });
        }
        let apply = ApplyEdlTool
            .handle(
                make_call(
                    "apply_edl",
                    serde_json::json!({"edl": edl, "dry_run": true}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        Ok(match apply {
            Ok(t) if t.content.contains("Validated 1 op") => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "concrete anchor fragment validated through apply_edl".into(),
            },
            Ok(t) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("unexpected apply_edl output: {}", t.content),
            },
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("apply_edl errored: {e:?}"),
            },
        })
    }
}

struct VeditDiffRestoreIsAudited;

#[async_trait]
impl Scenario for VeditDiffRestoreIsAudited {
    fn id(&self) -> &'static str {
        "product::vedit_diff_restore_is_audited"
    }
    fn description(&self) -> &'static str {
        "vedit diff sees a timeline change and restore writes a prior snapshot with a new audit commit."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        Project::init(dir.path())?;
        write_named_empty_timeline(dir.path(), "v1")?;
        let repo = awidat_core::vc::open_or_init(dir.path())?;
        let first = awidat_core::vc::commit_current_timeline(&repo, "v1", None)?;
        write_single_clip_project(dir.path(), "clip-0", "clip-0", "restore eval")?;
        awidat_core::vc::commit_current_timeline(&repo, "v2", None)?;
        let diff = awidat_core::vc::diff_refs(&repo, Some(&first.commit_hash), Some("HEAD"))?;
        let restored = awidat_core::vc::restore_working_timeline(&repo, &first.commit_hash)?;
        let audit = awidat_core::vc::commit_current_timeline(&repo, "restore v1", None)?;
        let elapsed = started.elapsed();
        let current: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("project.otio.json"))?)?;
        if diff.is_empty() {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: "diff was empty after adding a clip".into(),
            });
        }
        if current["name"].as_str() == Some("v1") && audit.commit_hash != restored.commit_hash {
            Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "restore wrote v1 and audit commit landed".into(),
            })
        } else {
            Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: "restore did not write expected snapshot or audit commit".into(),
            })
        }
    }
}

struct VeditRevertRecoveryFlow;

#[async_trait]
impl Scenario for VeditRevertRecoveryFlow {
    fn id(&self) -> &'static str {
        "product::vedit_revert_recovery_flow"
    }

    fn description(&self) -> &'static str {
        "A bad edit can be inspected with vedit_diff and recovered through vedit_revert with an audit commit."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "recover",
            &[
                ClipSpec::video("keeper", "keeper", "raw/keeper.mp4", 0.0, 5.0),
                ClipSpec::video("target", "target", "raw/target.mp4", 0.0, 5.0),
            ],
        )?;
        let repo = awidat_core::vc::open_or_init(dir.path())?;
        let clean = awidat_core::vc::commit_current_timeline(&repo, "clean cut", None)?;

        let bad_edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=target
*** End EDL
";
        ApplyEdlTool
            .handle(
                make_call(
                    "apply_edl",
                    serde_json::json!({"edl": bad_edl, "dry_run": false, "reasoning": "eval bad edit"}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let diff = VeditDiffTool
            .handle(
                make_call(
                    "vedit_diff",
                    serde_json::json!({"from": clean.commit_hash, "to": "HEAD"}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let revert = VeditRevertTool
            .handle(
                make_call(
                    "vedit_revert",
                    serde_json::json!({"refstr": clean.commit_hash, "commit": true, "reasoning": "eval recovery"}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        let project = Project::read(dir.path())?;
        Ok(match revert {
            Ok(t) => {
                let diff_body: serde_json::Value =
                    serde_json::from_str(&diff.content).unwrap_or(serde_json::Value::Null);
                let revert_body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                if diff_body["change_count"].as_u64().unwrap_or(0) > 0
                    && revert_body["audit_commit"]["commit_hash"]
                        .as_str()
                        .is_some()
                    && clip_count(&project.timeline) == 2
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "bad delete diffed and clean cut restored with audit commit"
                            .into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!(
                            "unexpected diff/revert state: {} / {}",
                            diff.content, t.content
                        ),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("vedit_revert errored: {e:?}"),
            },
        })
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
        .or_else(|_| std::env::var("AWIDAT_REAL_CORPUS"))
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
        message: "AWIDAT_REAL_PROJECT/AWIDAT_REAL_CORPUS not set to an indexed project; skipping live corpus eval".into(),
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
        // Pick a raw asset that actually has a transcript sidecar. `read_dir`
        // order is not stable, and real projects often keep scratch media in
        // raw/ that was not indexed.
        let raw_dir = root.join("raw");
        let asset_id = match std::fs::read_dir(&raw_dir) {
            Ok(entries) => {
                let mut assets = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        e.path()
                            .file_name()
                            .map(|n| format!("raw/{}", n.to_string_lossy()))
                    })
                    .collect::<Vec<_>>();
                assets.sort();
                assets.into_iter().find(|asset| {
                    root.join("index")
                        .join("whisper")
                        .join(format!("{asset}.json"))
                        .is_file()
                })
            }
            Err(_) => None,
        };
        let Some(asset_id) = asset_id else {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: "no raw/ assets with transcript sidecars found in real project".into(),
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
