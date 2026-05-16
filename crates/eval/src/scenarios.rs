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

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

use awidat_core::edl::{
    AnchorContext as EdlAnchorContext, apply as apply_edl_envelope, parse as parse_edl,
};
use awidat_core::tool::{ApprovalCache, ApprovalDecision, ApprovalKey, SandboxMode, ToolHandler};
use awidat_core::tools::{
    apply_edl::ApplyEdlTool, assess_edit_quality::AssessEditQualityTool, find_beat::FindBeatTool,
    find_broll_opportunities::FindBrollOpportunitiesTool, find_dead_air::FindDeadAirTool,
    find_moment::FindMomentTool, list_assets::ListAssetsTool, plan_transition::PlanTransitionTool,
    read_index::ReadIndexTool, transition_context::TransitionContextTool, use_broll::UseBrollTool,
    vedit_diff::VeditDiffTool, vedit_revert::VeditRevertTool, view_episode::ViewEpisodeTool,
    view_timeline::ViewTimelineTool,
};
use awidat_index::walk_indexer;
use awidat_proto::awidat_meta::{AudioRelation, CutType, cut_boundary_key};
use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Timeline, Track,
    TrackChild, TrackKind, Transition,
};
use awidat_proto::project::Project;

use crate::fixtures::{
    ClipSpec, clip_count, clip_range_by_uuid, write_motion_magnitudes, write_project_with_clips,
    write_shot_sidecar, write_silence_ranges, write_whisper_segments, write_whisper_words,
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
        Box::new(EditQualityMidSentenceRecommendsRecut),
        Box::new(EditQualitySpeakerTurnRecommendsJCut),
        Box::new(EditQualityBreathBeatRecommendsLCut),
        Box::new(EditQualityMotionUsesVisualContext),
        Box::new(EditQualityCentersSurfaceEyeTraceContext),
        Box::new(EditQualityModelCompositionSurfacesContext),
        Box::new(EditQualityGazeSurfacesEyeTraceContext),
        Box::new(EditQualityMatchCandidateRecommendsMatchCut),
        Box::new(EditQualityOcclusionUsesPassByTransition),
        Box::new(EditQualityTransitionDensitySuppressesTransition),
        Box::new(TransitionContextPlannerFlow),
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
        Box::new(RealProjectEditQualityVisualContext),
        Box::new(RealProjectTransitionPlannerFixture),
        Box::new(RealProjectAssessorProposalFixture),
        Box::new(RealProjectRoughAssemblyFixture),
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

/// Product scenario: edit-quality assessment should recommend moving a
/// mid-sentence cut instead of hiding it with a transition.
struct EditQualityMidSentenceRecommendsRecut;

#[async_trait]
impl Scenario for EditQualityMidSentenceRecommendsRecut {
    fn id(&self) -> &'static str {
        "product::edit_quality_mid_sentence_recommends_recut"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality routes a mid-word cut to recut, not a decorative transition."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_whisper_words(
            dir.path(),
            "raw/host.mp4",
            &[
                ("this", 0.2, 0.5),
                ("sentence", 1.0, 1.8),
                ("continues", 2.0, 2.6),
            ],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({"at_s": 1.3, "kind": "cut"}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let action = body["recommendation"]["action"].as_str().unwrap_or("");
                let transition = &body["recommendation"]["transition"];
                let verdict = body["continuity"]["verdict"].as_str().unwrap_or("");
                if verdict == "dirty" && action == "recut" && transition.is_null() {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "mid-sentence cut recommended recut with no transition".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected edit-quality output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: speaker-turn continuity should route to a J-cut
/// split edit, not a visible transition.
struct EditQualitySpeakerTurnRecommendsJCut;

#[async_trait]
impl Scenario for EditQualitySpeakerTurnRecommendsJCut {
    fn id(&self) -> &'static str {
        "product::edit_quality_speaker_turn_recommends_j_cut"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality routes a risky speaker-turn cut to Set Audio Lead."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-j-cut",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_whisper_segments(
            dir.path(),
            "raw/host.mp4",
            &[("this thought needs audio continuity", 0.0, 5.0, "A")],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({"at_s": 2.0, "kind": "cut"}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let action = body["recommendation"]["action"].as_str().unwrap_or("");
                let edl_op = body["recommendation"]["edl_op"].as_str().unwrap_or("");
                let transition = &body["recommendation"]["transition"];
                let verdict = body["continuity"]["verdict"].as_str().unwrap_or("");
                if verdict == "risky"
                    && action == "use_j_cut"
                    && edl_op == "Set Audio Lead"
                    && transition.is_null()
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "speaker-turn cut recommended J-cut audio lead".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected J-cut output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality J-cut errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: cutting into a breath beat should preserve
/// outgoing audio with an L-cut instead of adding a dissolve.
struct EditQualityBreathBeatRecommendsLCut;

#[async_trait]
impl Scenario for EditQualityBreathBeatRecommendsLCut {
    fn id(&self) -> &'static str {
        "product::edit_quality_breath_beat_recommends_l_cut"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality routes a breath-beat trim to Set Audio Trail."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-l-cut",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_silence_ranges(dir.path(), "raw/host.mp4", &[(1.0, 1.3)])?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({"at_s": 1.15, "kind": "trim_out"}),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let action = body["recommendation"]["action"].as_str().unwrap_or("");
                let edl_op = body["recommendation"]["edl_op"].as_str().unwrap_or("");
                let transition = &body["recommendation"]["transition"];
                let verdict = body["continuity"]["verdict"].as_str().unwrap_or("");
                if verdict == "risky"
                    && action == "preserve_audio_tail"
                    && edl_op == "Set Audio Trail"
                    && transition.is_null()
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "breath-beat trim recommended L-cut audio trail".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected L-cut output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality L-cut errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: edit-quality assessment should use shot/motion
/// visual context when naming a motion-cover transition.
struct EditQualityMotionUsesVisualContext;

#[async_trait]
impl Scenario for EditQualityMotionUsesVisualContext {
    fn id(&self) -> &'static str {
        "product::edit_quality_motion_uses_visual_context"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality uses shot motion context and requested screen direction for motion-cover recommendations."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-motion",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.78, 0.82, 0.2, 0.1, 0.0],
        )?;
        write_shot_sidecar(
            dir.path(),
            "raw/host.mp4",
            &[serde_json::json!({
                "index": 0,
                "start_s": 0.0,
                "end_s": 8.0,
                "type": "wide",
                "motion": "handheld",
                "motion_magnitude": 0.82
            })],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 3.4,
                        "kind": "cut",
                        "objective": "hide_jump direction:right"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let transition = body["recommendation"]["transition"].as_str().unwrap_or("");
                let motion = body["visual_context"]["motion"].as_str().unwrap_or("");
                let verdict = body["continuity"]["verdict"].as_str().unwrap_or("");
                if verdict == "dirty"
                    && transition == "awidat.whip_pan_right"
                    && motion == "handheld"
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "motion-context cut recommended rightward whip-pan cover".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected edit-quality motion output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality motion errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: edit-quality assessment should preserve shot-indexed
/// screen positions so downstream agents can reason about eye trace and
/// framing continuity.
struct EditQualityCentersSurfaceEyeTraceContext;

#[async_trait]
impl Scenario for EditQualityCentersSurfaceEyeTraceContext {
    fn id(&self) -> &'static str {
        "product::edit_quality_centers_surface_eye_trace_context"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality surfaces subject and face center fields from shot visual context."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-centers",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.78, 0.82, 0.2, 0.1, 0.0],
        )?;
        write_shot_sidecar(
            dir.path(),
            "raw/host.mp4",
            &[serde_json::json!({
                "index": 0,
                "start_s": 0.0,
                "end_s": 8.0,
                "type": "medium",
                "motion": "handheld",
                "motion_magnitude": 0.82,
                "subject_center": [0.54, 0.42],
                "face_center": [0.51, 0.36],
                "subject_zone": "center",
                "face_zone": "middle_center",
                "headroom": "balanced",
                "look_space": "direct"
            })],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 3.4,
                        "kind": "cut",
                        "objective": "hide_jump"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let subject_x = body["visual_context"]["subject_center"]
                    .get(0)
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0);
                let face_y = body["visual_context"]["face_center"]
                    .get(1)
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0);
                let subject_zone = body["visual_context"]["subject_zone"]
                    .as_str()
                    .unwrap_or("");
                let face_zone = body["visual_context"]["face_zone"].as_str().unwrap_or("");
                let headroom = body["visual_context"]["headroom"].as_str().unwrap_or("");
                let look_space = body["visual_context"]["look_space"].as_str().unwrap_or("");
                if (subject_x - 0.54).abs() < 1e-9
                    && (face_y - 0.36).abs() < 1e-9
                    && subject_zone == "center"
                    && face_zone == "middle_center"
                    && headroom == "balanced"
                    && look_space == "direct"
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message:
                            "shot centers and composition semantics surfaced in visual context"
                                .into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected center-context output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality center context errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: model-enriched composition labels should survive
/// through `assess_edit_quality` so agents can reason about framing
/// semantics beyond deterministic center buckets.
struct EditQualityModelCompositionSurfacesContext;

#[async_trait]
impl Scenario for EditQualityModelCompositionSurfacesContext {
    fn id(&self) -> &'static str {
        "product::edit_quality_model_composition_surfaces_context"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality surfaces model-backed composition labels from shot visual context."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-model-composition",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.78, 0.82, 0.2, 0.1, 0.0],
        )?;
        write_shot_sidecar(
            dir.path(),
            "raw/host.mp4",
            &[serde_json::json!({
                "index": 0,
                "start_s": 0.0,
                "end_s": 8.0,
                "type": "medium",
                "motion": "handheld",
                "motion_magnitude": 0.82,
                "subject_center": [0.54, 0.42],
                "face_center": [0.51, 0.36],
                "composition_source": "model:composition-v1",
                "composition_confidence": 0.82,
                "subject_role": "primary_speaker",
                "depth_layer": "foreground",
                "framing": "over_the_shoulder"
            })],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 3.4,
                        "kind": "cut",
                        "objective": "hide_jump"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let visual = &body["visual_context"];
                let source = visual["composition_source"].as_str().unwrap_or("");
                let confidence = visual["composition_confidence"].as_f64().unwrap_or(0.0);
                let subject_role = visual["subject_role"].as_str().unwrap_or("");
                let depth_layer = visual["depth_layer"].as_str().unwrap_or("");
                let framing = visual["framing"].as_str().unwrap_or("");
                if source == "model:composition-v1"
                    && (confidence - 0.82).abs() < 1e-9
                    && subject_role == "primary_speaker"
                    && depth_layer == "foreground"
                    && framing == "over_the_shoulder"
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "model composition labels surfaced in visual context".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!(
                            "unexpected model-composition context output: {}",
                            t.content
                        ),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality model composition errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: edit-quality assessment should preserve shot-indexed
/// gaze fields for eye-trace continuity decisions.
struct EditQualityGazeSurfacesEyeTraceContext;

#[async_trait]
impl Scenario for EditQualityGazeSurfacesEyeTraceContext {
    fn id(&self) -> &'static str {
        "product::edit_quality_gaze_surfaces_eye_trace_context"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality surfaces gaze score, at-camera ratio, and eye-trace fields from shot visual context."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-gaze",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.78, 0.82, 0.2, 0.1, 0.0],
        )?;
        write_shot_sidecar(
            dir.path(),
            "raw/host.mp4",
            &[serde_json::json!({
                "index": 0,
                "start_s": 0.0,
                "end_s": 8.0,
                "type": "medium",
                "motion": "handheld",
                "motion_magnitude": 0.82,
                "gaze_score": -0.22,
                "at_camera_ratio": 0.18,
                "eye_trace": "left",
                "gaze_samples": 3
            })],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 3.4,
                        "kind": "cut",
                        "objective": "hide_jump"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let gaze_score = body["visual_context"]["gaze_score"].as_f64().unwrap_or(0.0);
                let at_camera_ratio = body["visual_context"]["at_camera_ratio"]
                    .as_f64()
                    .unwrap_or(1.0);
                let eye_trace = body["visual_context"]["eye_trace"].as_str().unwrap_or("");
                let samples = body["visual_context"]["gaze_samples"].as_u64().unwrap_or(0);
                if (gaze_score + 0.22).abs() < 1e-9
                    && (at_camera_ratio - 0.18).abs() < 1e-9
                    && eye_trace == "left"
                    && samples == 3
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "shot gaze and eye-trace fields surfaced in visual context".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected gaze-context output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality gaze context errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: edit-quality assessment should route strong visual
/// match candidates to semantic match cuts before considering visible
/// motion-cover transitions.
struct EditQualityMatchCandidateRecommendsMatchCut;

#[async_trait]
impl Scenario for EditQualityMatchCandidateRecommendsMatchCut {
    fn id(&self) -> &'static str {
        "product::edit_quality_match_candidate_recommends_match_cut"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality uses shot match candidates for semantic match-cut recommendations."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-match-cut",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.78, 0.82, 0.2, 0.1, 0.0],
        )?;
        write_shot_sidecar(
            dir.path(),
            "raw/host.mp4",
            &[serde_json::json!({
                "index": 0,
                "start_s": 0.0,
                "end_s": 8.0,
                "type": "medium",
                "motion": "handheld",
                "motion_magnitude": 0.82,
                "dominant_direction": "right",
                "action_score": 0.74,
                "match_cut_score": 0.86,
                "match_candidates": [
                    {"asset_id": "raw/guest.mp4", "time_s": 88.1, "similarity": 0.86, "confidence": 0.88, "basis": "shape_and_motion"}
                ]
            })],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 3.4,
                        "kind": "cut",
                        "objective": "hide_jump"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let action = body["recommendation"]["action"].as_str().unwrap_or("");
                let cut_type = body["recommendation"]["cut_type"].as_str().unwrap_or("");
                let transition = &body["recommendation"]["transition"];
                let match_score = body["visual_context"]["match_cut_score"]
                    .as_f64()
                    .unwrap_or(0.0);
                let candidate_time = body["visual_context"]["match_candidate"]["time_s"]
                    .as_f64()
                    .unwrap_or(0.0);
                let candidate_asset = body["visual_context"]["match_candidate"]["asset_id"]
                    .as_str()
                    .unwrap_or("");
                let confidence = body["visual_context"]["match_candidate"]["confidence"]
                    .as_f64()
                    .unwrap_or(0.0);
                let reason = body["recommendation"]["reason"].as_str().unwrap_or("");
                if action == "use_match_cut"
                    && cut_type == "match_cut"
                    && transition.is_null()
                    && match_score >= 0.8
                    && (candidate_time - 88.1).abs() < 1e-9
                    && candidate_asset == "raw/guest.mp4"
                    && confidence >= 0.8
                    && reason.contains("raw/guest.mp4 at 88.10s")
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "strong visual match candidate recommended semantic match cut"
                            .into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected match-cut output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality match-cut errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: edit-quality assessment should prefer pass-by
/// transitions over generic motion blur when shot sidecars identify an
/// occlusion/mask opportunity.
struct EditQualityOcclusionUsesPassByTransition;

#[async_trait]
impl Scenario for EditQualityOcclusionUsesPassByTransition {
    fn id(&self) -> &'static str {
        "product::edit_quality_occlusion_uses_pass_by_transition"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality uses occlusion and screen-direction visual context for pass-by recommendations."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "edit-quality-pass-by",
            &[ClipSpec::video("host", "host", "raw/host.mp4", 0.0, 8.0)],
        )?;
        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.78, 0.82, 0.2, 0.1, 0.0],
        )?;
        write_shot_sidecar(
            dir.path(),
            "raw/host.mp4",
            &[serde_json::json!({
                "index": 0,
                "start_s": 0.0,
                "end_s": 8.0,
                "type": "wide",
                "motion": "fast-cut",
                "motion_magnitude": 0.82,
                "dominant_direction": "left",
                "action_score": 0.78,
                "whip_pan_score": 0.22,
                "occlusion_score": 0.84
            })],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 3.4,
                        "kind": "cut",
                        "objective": "hide_jump"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let transition = body["recommendation"]["transition"].as_str().unwrap_or("");
                let occlusion = body["visual_context"]["occlusion_score"]
                    .as_f64()
                    .unwrap_or(0.0);
                if transition == "awidat.pass_by_left" && occlusion >= 0.8 {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "occlusion-context cut recommended left pass-by cover".into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected occlusion output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality occlusion errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: when a timeline already has high transition
/// density, motion repair should prefer b-roll/cut-on-action over one
/// more visible transition.
struct EditQualityTransitionDensitySuppressesTransition;

#[async_trait]
impl Scenario for EditQualityTransitionDensitySuppressesTransition {
    fn id(&self) -> &'static str {
        "product::edit_quality_transition_density_suppresses_transition"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality avoids adding another transition when recent transition density is already high."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        let clips = (0..5)
            .map(|idx| {
                ClipSpec::video(
                    &format!("host-{idx}"),
                    &format!("host-{idx}"),
                    "raw/host.mp4",
                    (idx as f64) * 2.0,
                    2.0,
                )
            })
            .collect::<Vec<_>>();
        write_project_with_clips(dir.path(), "edit-quality-density", &clips)?;

        let mut project = Project::read(dir.path())?;
        let StackChild::Track(track) = &mut project.timeline.tracks.children[0] else {
            unreachable!("fixture writes a video track")
        };
        for index in [1_usize, 3, 5] {
            track.children.insert(
                index,
                TrackChild::Transition(Transition::symmetric("awidat.flash_white", 0.1, 24.0)),
            );
        }
        project.write(dir.path())?;

        write_motion_magnitudes(
            dir.path(),
            "raw/host.mp4",
            &[0.0, 0.1, 0.2, 0.1, 0.2, 0.1, 0.1, 0.76, 0.82, 0.2],
        )?;
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "at_s": 7.4,
                        "kind": "cut",
                        "objective": "hide_jump direction:right"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await;
        let elapsed = started.elapsed();
        Ok(match out {
            Ok(t) => {
                let body: serde_json::Value =
                    serde_json::from_str(&t.content).unwrap_or(serde_json::Value::Null);
                let transition = &body["recommendation"]["transition"];
                let density = body["style_context"]["transition_density_last_30s"]
                    .as_u64()
                    .unwrap_or(0);
                let intent = body["recommendation"]["intent"].as_str().unwrap_or("");
                if transition.is_null()
                    && density >= 3
                    && intent == "hide_motion_jump_without_transition"
                {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: "high transition density suppressed another motion transition"
                            .into(),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("unexpected density output: {}", t.content),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality density errored: {e:?}"),
            },
        })
    }
}

/// Product scenario: transition planning should be a read-only
/// context-then-plan chain, with hard-cut default and motivated
/// transition only when the caller names a job.
struct TransitionContextPlannerFlow;

#[async_trait]
impl Scenario for TransitionContextPlannerFlow {
    fn id(&self) -> &'static str {
        "product::transition_context_planner_flow"
    }

    fn description(&self) -> &'static str {
        "transition_context feeds plan_transition, preserving hard-cut default and handle-aware visible-transition proposals."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let dir = tempfile::tempdir()?;
        write_project_with_clips(
            dir.path(),
            "transition-planner-flow",
            &[
                ClipSpec::video("clip-a", "clip-a", "raw/a.mp4", 0.0, 5.0),
                ClipSpec::video("clip-b", "clip-b", "raw/b.mp4", 1.0, 4.0),
            ],
        )?;
        write_whisper_words(
            dir.path(),
            "raw/a.mp4",
            &[("outgoing", 4.2, 4.6), ("line", 4.7, 4.9)],
        )?;
        write_whisper_words(
            dir.path(),
            "raw/b.mp4",
            &[("incoming", 1.1, 1.3), ("reply", 1.4, 1.7)],
        )?;

        let context_out = TransitionContextTool
            .handle(
                make_call(
                    "transition_context",
                    serde_json::json!({
                        "between": {
                            "from": {"clip_uuid": "clip-a"},
                            "to": {"clip_uuid": "clip-b"}
                        },
                        "window_s": 2.0
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .context("transition_context")?;
        let context: serde_json::Value = serde_json::from_str(&context_out.content)
            .context("parse transition_context output")?;

        let hard_cut_out = PlanTransitionTool
            .handle(
                make_call(
                    "plan_transition",
                    serde_json::json!({"context": context.clone()}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .context("plan_transition hard cut")?;
        let hard_cut: serde_json::Value =
            serde_json::from_str(&hard_cut_out.content).context("parse hard-cut plan")?;

        let transition_out = PlanTransitionTool
            .handle(
                make_call(
                    "plan_transition",
                    serde_json::json!({
                        "context": context,
                        "objective": "hide_motion_jump",
                        "direction": "left"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .context("plan_transition visible transition")?;
        let transition: serde_json::Value =
            serde_json::from_str(&transition_out.content).context("parse transition plan")?;

        let elapsed = started.elapsed();
        let hard_decision = hard_cut
            .pointer("/recommended/decision")
            .and_then(|value| value.as_str());
        let visible_id = transition
            .pointer("/recommended/id")
            .and_then(|value| value.as_str());
        let visible_edl = transition
            .pointer("/edl_fragment")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let visible_envelope = parse_edl(visible_edl).context("parse planner visible EDL")?;
        let project = Project::read(dir.path())?;
        let edl_ctx = EdlAnchorContext::with_project_root(dir.path());
        let (planned_timeline, _planned_outcome) =
            apply_edl_envelope(&project.timeline, &visible_envelope, &edl_ctx)
                .context("apply planner visible EDL")?;
        let visible_transition_applied = planned_timeline
            .tracks
            .children
            .iter()
            .filter_map(|child| match child {
                StackChild::Track(track) => Some(track),
                _ => None,
            })
            .flat_map(|track| &track.children)
            .any(|child| matches!(child, TrackChild::Transition(transition) if transition.transition_type == "awidat.whip_pan_left"));
        if hard_decision == Some("hard_cut")
            && visible_id == Some("awidat.whip_pan_left")
            && visible_edl.contains("*** Insert Transition")
            && visible_edl.contains("+ duration_s: 0.180")
            && visible_transition_applied
        {
            Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Pass,
                elapsed,
                message: "transition planner preserved hard-cut default and produced a handle-safe motivated transition".into(),
            })
        } else {
            Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!(
                    "unexpected transition plans: hard={} transition={}",
                    hard_cut_out.content, transition_out.content
                ),
            })
        }
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
            vec![],
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

/// Runs `transition_context` and `plan_transition` against a mounted
/// real-project boundary fixture. This promotes the synthetic
/// transition-planner product check into the live corpus tier without
/// committing media.
struct RealProjectTransitionPlannerFixture;

#[async_trait]
impl Scenario for RealProjectTransitionPlannerFixture {
    fn id(&self) -> &'static str {
        "real::transition_planner_fixture"
    }

    fn description(&self) -> &'static str {
        "mounted real-project transition planner fixture validates context, recommendation, and EDL fragment."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        let override_path = transition_planner_fixture_override_path();
        let min_fixtures = env_usize("AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES", 0)?;
        run_transition_planner_fixture_at(
            &root,
            override_path.as_deref(),
            self.id(),
            started,
            min_fixtures,
        )
        .await
    }
}

async fn run_transition_planner_fixture_at(
    root: &std::path::Path,
    override_path: Option<&std::path::Path>,
    scenario_id: &str,
    started: Instant,
    min_fixtures: usize,
) -> Result<ScenarioOutcome> {
    let paths = transition_planner_fixture_paths(root, override_path)?;
    if paths.len() < min_fixtures {
        return Ok(ScenarioOutcome {
            id: scenario_id.into(),
            status: ScenarioStatus::Fail,
            elapsed: started.elapsed(),
            message: format!(
                "expected at least {min_fixtures} transition planner fixture(s), found {}",
                paths.len()
            ),
        });
    }
    if paths.is_empty() {
        return Ok(ScenarioOutcome {
            id: scenario_id.into(),
            status: ScenarioStatus::Skipped,
            elapsed: started.elapsed(),
            message: "no transition planner fixture found; set AWIDAT_REAL_TRANSITION_PLANNER_FIXTURE or add .awidat/eval/transition-planner-flow.json / .awidat/eval/transition-planners/*.json".into(),
        });
    }
    let mut validated_total = 0;
    for path in &paths {
        let fixture = match load_transition_planner_fixture(path) {
            Ok(fixture) => fixture,
            Err(err) => {
                return Ok(ScenarioOutcome {
                    id: scenario_id.into(),
                    status: ScenarioStatus::Fail,
                    elapsed: started.elapsed(),
                    message: format!("{}: {err:#}", path.display()),
                });
            }
        };
        if let Err(err) = validate_transition_planner_fixture(root, &fixture).await {
            return Ok(ScenarioOutcome {
                id: scenario_id.into(),
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!("{}: {err:#}", path.display()),
            });
        }
        validated_total += 1;
    }
    let message = if paths.len() == 1 {
        format!(
            "validated transition planner fixture {}",
            paths[0].display()
        )
    } else {
        format!("validated {validated_total} transition planner fixture(s)")
    };
    Ok(ScenarioOutcome {
        id: scenario_id.into(),
        status: ScenarioStatus::Pass,
        elapsed: started.elapsed(),
        message,
    })
}

fn transition_planner_fixture_override_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("AWIDAT_REAL_TRANSITION_PLANNER_FIXTURE")
        && !path.trim().is_empty()
    {
        return Some(std::path::PathBuf::from(path));
    }
    None
}

fn transition_planner_fixture_paths(
    root: &std::path::Path,
    override_path: Option<&std::path::Path>,
) -> Result<Vec<std::path::PathBuf>> {
    if let Some(path) = override_path {
        return collect_json_fixture_paths(path);
    }
    let default = root
        .join(".awidat")
        .join("eval")
        .join("transition-planner-flow.json");
    let directory = root
        .join(".awidat")
        .join("eval")
        .join("transition-planners");
    collect_default_json_fixture_paths(&default, &directory)
}

fn load_transition_planner_fixture(path: &std::path::Path) -> Result<TransitionPlannerFixture> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

async fn validate_transition_planner_fixture(
    root: &std::path::Path,
    fixture: &TransitionPlannerFixture,
) -> Result<()> {
    if fixture.id.trim().is_empty() {
        bail!("transition planner fixture id is empty");
    }
    if fixture.between.from.trim().is_empty() || fixture.between.to.trim().is_empty() {
        bail!("transition planner fixture boundary clip ids must be non-empty");
    }

    let context_out = TransitionContextTool
        .handle(
            make_call(
                "transition_context",
                serde_json::json!({
                    "between": {
                        "from": {"clip_uuid": fixture.between.from},
                        "to": {"clip_uuid": fixture.between.to},
                    },
                    "window_s": fixture.window_s.unwrap_or(6.0),
                }),
            ),
            ctx_at(root),
        )
        .await
        .context("transition_context")?;
    let context: serde_json::Value =
        serde_json::from_str(&context_out.content).context("parse transition_context output")?;

    let mut plan_args = serde_json::json!({ "context": context });
    if let Some(objective) = &fixture.plan.objective {
        plan_args["objective"] = serde_json::Value::String(objective.clone());
    }
    if let Some(direction) = &fixture.plan.direction {
        plan_args["direction"] = serde_json::Value::String(direction.clone());
    }

    let plan_out = PlanTransitionTool
        .handle(make_call("plan_transition", plan_args), ctx_at(root))
        .await
        .context("plan_transition")?;
    let plan: serde_json::Value =
        serde_json::from_str(&plan_out.content).context("parse plan_transition output")?;
    assert_transition_planner_expectations(root, fixture, &plan).with_context(|| {
        format!(
            "fixture {} got plan_transition output {}",
            fixture.id, plan_out.content
        )
    })
}

fn assert_transition_planner_expectations(
    root: &std::path::Path,
    fixture: &TransitionPlannerFixture,
    plan: &serde_json::Value,
) -> Result<()> {
    validate_transition_planner_expect_contract(fixture)?;

    let decision = plan
        .pointer("/recommended/decision")
        .and_then(|value| value.as_str());
    if decision != Some(fixture.expect.decision.as_str()) {
        bail!(
            "expected decision {:?}, got {:?}",
            fixture.expect.decision,
            decision
        );
    }
    if let Some(expected_id) = &fixture.expect.transition_id {
        let actual_id = plan
            .pointer("/recommended/id")
            .and_then(|value| value.as_str());
        if actual_id != Some(expected_id.as_str()) {
            bail!("expected transition id {expected_id:?}, got {actual_id:?}");
        }
    }
    if let Some(expected_intent) = &fixture.expect.intent {
        let actual_intent = plan
            .pointer("/recommended/intent")
            .and_then(|value| value.as_str());
        if actual_intent != Some(expected_intent.as_str()) {
            bail!("expected intent {expected_intent:?}, got {actual_intent:?}");
        }
    }
    if let Some(needle) = &fixture.expect.reason_contains {
        let reason = plan
            .pointer("/recommended/reason")
            .and_then(|value| value.as_str());
        if !reason.is_some_and(|reason| reason.contains(needle)) {
            bail!("expected recommendation reason to contain {needle:?}, got {reason:?}");
        }
    }

    let edl = plan
        .get("edl_fragment")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if edl.trim().is_empty() {
        bail!("plan_transition returned an empty edl_fragment");
    }
    for snippet in &fixture.expect.edl_contains {
        if !edl.contains(snippet) {
            bail!("edl_fragment expected to contain {snippet:?}");
        }
    }
    for snippet in &fixture.expect.edl_must_not_contain {
        if edl.contains(snippet) {
            bail!("edl_fragment must not contain {snippet:?}");
        }
    }
    if fixture.expect.apply_edl_fragment.unwrap_or(true) {
        let envelope = parse_edl(edl).context("parse planner edl_fragment")?;
        let project = Project::read(root).context("read project for transition planner fixture")?;
        let ctx = EdlAnchorContext::with_project_root(root);
        let (timeline, outcome) =
            apply_edl_envelope(&project.timeline, &envelope, &ctx).context("apply edl_fragment")?;
        if outcome.applied.is_empty() {
            bail!("planner edl_fragment applied zero operations");
        }
        if let Some(expected_id) = &fixture.expect.transition_id {
            let transition_applied = timeline
                .tracks
                .children
                .iter()
                .filter_map(|child| match child {
                    StackChild::Track(track) => Some(track),
                    _ => None,
                })
                .flat_map(|track| &track.children)
                .any(|child| {
                    matches!(child, TrackChild::Transition(transition) if transition.transition_type == *expected_id)
                });
            if !transition_applied {
                bail!("applied edl_fragment did not create transition {expected_id:?}");
            }
        }
    }
    Ok(())
}

fn validate_transition_planner_expect_contract(fixture: &TransitionPlannerFixture) -> Result<()> {
    let decision = fixture.expect.decision.trim();
    if decision.is_empty() {
        bail!("transition planner fixture expect.decision is empty");
    }
    if !matches!(decision, "hard_cut" | "transition") {
        bail!(
            "transition planner fixture has unsupported expect.decision {:?}",
            fixture.expect.decision
        );
    }
    for snippet in &fixture.expect.edl_contains {
        if snippet.trim().is_empty() {
            bail!("transition planner fixture has empty edl_contains snippet");
        }
    }
    for snippet in &fixture.expect.edl_must_not_contain {
        if snippet.trim().is_empty() {
            bail!("transition planner fixture has empty edl_must_not_contain snippet");
        }
    }

    match decision {
        "hard_cut" => {
            if !fixture
                .expect
                .edl_contains
                .iter()
                .any(|snippet| snippet.contains("*** Set Cut Intent"))
            {
                bail!(
                    "hard_cut transition planner fixtures must include *** Set Cut Intent in edl_contains"
                );
            }
            if !fixture
                .expect
                .edl_contains
                .iter()
                .any(|snippet| snippet.contains("+ cut_type: hard_cut"))
            {
                bail!(
                    "hard_cut transition planner fixtures must include + cut_type: hard_cut in edl_contains"
                );
            }
            if !fixture
                .expect
                .edl_must_not_contain
                .iter()
                .any(|snippet| snippet.contains("*** Insert Transition"))
            {
                bail!(
                    "hard_cut transition planner fixtures must include *** Insert Transition in edl_must_not_contain"
                );
            }
        }
        "transition" => {
            let transition_id = fixture
                .expect
                .transition_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "transition planner fixtures with decision transition must include transition_id"
                    )
                })?;
            if !fixture
                .expect
                .edl_contains
                .iter()
                .any(|snippet| snippet.contains("*** Insert Transition"))
            {
                bail!(
                    "transition planner fixtures with decision transition must include *** Insert Transition in edl_contains"
                );
            }
            if !fixture
                .expect
                .edl_contains
                .iter()
                .any(|snippet| snippet.contains(transition_id))
            {
                bail!(
                    "transition planner fixtures with decision transition must include transition_id {transition_id:?} in edl_contains"
                );
            }
        }
        _ => unreachable!("decision was validated above"),
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct TransitionPlannerFixture {
    id: String,
    between: TransitionPlannerBoundary,
    #[serde(default)]
    window_s: Option<f64>,
    #[serde(default)]
    plan: TransitionPlannerPlan,
    expect: TransitionPlannerExpect,
}

#[derive(Debug, serde::Deserialize)]
struct TransitionPlannerBoundary {
    from: String,
    to: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct TransitionPlannerPlan {
    #[serde(default)]
    objective: Option<String>,
    #[serde(default)]
    direction: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct TransitionPlannerExpect {
    decision: String,
    #[serde(default)]
    transition_id: Option<String>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    reason_contains: Option<String>,
    #[serde(default)]
    edl_contains: Vec<String>,
    #[serde(default)]
    edl_must_not_contain: Vec<String>,
    #[serde(default)]
    apply_edl_fragment: Option<bool>,
}

/// Applies a mounted real-project assessor proposal fixture. This
/// promotes the desktop smoke lifecycle (reject first recommendation,
/// accept a follow-up proposal) into the live corpus tier without
/// committing large media.
struct RealProjectAssessorProposalFixture;

#[async_trait]
impl Scenario for RealProjectAssessorProposalFixture {
    fn id(&self) -> &'static str {
        "real::assessor_proposal_fixture"
    }

    fn description(&self) -> &'static str {
        "mounted real-project assessor proposal fixture applies and records rejected/follow-up history."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        let override_path = assessor_proposal_fixture_override_path();
        let min_fixtures = env_usize("AWIDAT_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES", 0)?;
        run_assessor_proposal_fixture_at(
            &root,
            override_path.as_deref(),
            self.id(),
            started,
            min_fixtures,
        )
    }
}

fn run_assessor_proposal_fixture_at(
    root: &std::path::Path,
    override_path: Option<&std::path::Path>,
    scenario_id: &str,
    started: Instant,
    min_fixtures: usize,
) -> Result<ScenarioOutcome> {
    let paths = assessor_proposal_fixture_paths(root, override_path)?;
    if paths.len() < min_fixtures {
        return Ok(ScenarioOutcome {
            id: scenario_id.into(),
            status: ScenarioStatus::Fail,
            elapsed: started.elapsed(),
            message: format!(
                "expected at least {min_fixtures} assessor proposal fixture(s), found {}",
                paths.len()
            ),
        });
    }
    if paths.is_empty() {
        return Ok(ScenarioOutcome {
            id: scenario_id.into(),
            status: ScenarioStatus::Skipped,
            elapsed: started.elapsed(),
            message: "no assessor proposal fixture found; set AWIDAT_REAL_ASSESSOR_PROPOSAL_FIXTURE or add .awidat/eval/assessor-proposal-flow.json / .awidat/eval/assessor-proposals/*.json".into(),
        });
    }
    let elapsed = started.elapsed();
    let mut applied_total = 0;
    for path in &paths {
        match load_assessor_proposal_fixture(path)
            .and_then(|fixture| validate_assessor_proposal_fixture(root, &fixture))
        {
            Ok(applied) => {
                applied_total += applied;
            }
            Err(err) => {
                return Ok(ScenarioOutcome {
                    id: scenario_id.into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!("{}: {err:#}", path.display()),
                });
            }
        }
    }
    let message = if paths.len() == 1 {
        format!(
            "validated {} with {applied_total} applied op(s)",
            paths[0].display()
        )
    } else {
        format!(
            "validated {} fixture(s) with {applied_total} applied op(s)",
            paths.len()
        )
    };
    Ok(ScenarioOutcome {
        id: scenario_id.into(),
        status: ScenarioStatus::Pass,
        elapsed,
        message,
    })
}

fn assessor_proposal_fixture_override_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("AWIDAT_REAL_ASSESSOR_PROPOSAL_FIXTURE")
        && !path.trim().is_empty()
    {
        return Some(std::path::PathBuf::from(path));
    }
    None
}

fn assessor_proposal_fixture_paths(
    root: &std::path::Path,
    override_path: Option<&std::path::Path>,
) -> Result<Vec<std::path::PathBuf>> {
    if let Some(path) = override_path {
        return collect_json_fixture_paths(path);
    }
    let default = root
        .join(".awidat")
        .join("eval")
        .join("assessor-proposal-flow.json");
    let directory = root.join(".awidat").join("eval").join("assessor-proposals");
    collect_default_json_fixture_paths(&default, &directory)
}

fn load_assessor_proposal_fixture(path: &std::path::Path) -> Result<AssessorProposalFixture> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn validate_assessor_proposal_fixture(
    root: &std::path::Path,
    fixture: &AssessorProposalFixture,
) -> Result<usize> {
    if fixture.id.trim().is_empty() {
        bail!("assessor proposal fixture id is empty");
    }
    if fixture.final_edl.trim().is_empty() {
        bail!("assessor proposal fixture final_edl is empty");
    }
    if fixture.proposal_history.is_empty() {
        bail!("assessor proposal fixture has no proposal_history entries");
    }
    if !fixture
        .proposal_history
        .iter()
        .any(|entry| entry.status == "rejected")
        || !fixture
            .proposal_history
            .iter()
            .any(|entry| entry.status == "accepted")
    {
        bail!(
            "assessor proposal fixture must include at least one rejected and one accepted proposal_history entry"
        );
    }
    for entry in &fixture.proposal_history {
        validate_assessor_proposal_history_entry(entry, &fixture.final_edl)?;
    }

    let envelope = parse_edl(&fixture.final_edl).context("parse final_edl")?;
    let project = Project::read(root).context("read project for assessor proposal fixture")?;
    let ctx = EdlAnchorContext::with_project_root(root);
    let (_timeline, outcome) =
        apply_edl_envelope(&project.timeline, &envelope, &ctx).context("apply final_edl")?;
    if outcome.applied.is_empty() {
        bail!("final_edl applied zero operations");
    }
    Ok(outcome.applied.len())
}

fn validate_assessor_proposal_history_entry(
    entry: &AssessorProposalHistoryEntry,
    final_edl: &str,
) -> Result<()> {
    if entry.id.trim().is_empty() {
        bail!("proposal_history entry has empty id");
    }
    if entry.status != "accepted" && entry.status != "rejected" {
        bail!(
            "proposal_history entry {} has unsupported status {:?}",
            entry.id,
            entry.status
        );
    }
    if entry.recommendation.trim().is_empty() {
        bail!(
            "proposal_history entry {} has empty recommendation",
            entry.id
        );
    }
    if entry.edl_contains.is_empty() {
        bail!(
            "proposal_history entry {} must include at least one edl_contains snippet",
            entry.id
        );
    }
    for snippet in &entry.edl_contains {
        if snippet.trim().is_empty() {
            bail!("proposal_history entry {} has empty edl_contains", entry.id);
        }
    }
    validate_proposal_history_edl_lifecycle(
        &entry.id,
        &entry.status,
        &entry.edl_contains,
        final_edl,
    )?;
    validate_proposal_history_final_edl_snippets(
        &entry.id,
        "final_edl_must_contain",
        &entry.final_edl_must_contain,
    )?;
    validate_proposal_history_final_edl_snippets(
        &entry.id,
        "final_edl_must_not_contain",
        &entry.final_edl_must_not_contain,
    )?;
    for snippet in &entry.final_edl_must_contain {
        if !final_edl.contains(snippet) {
            bail!(
                "proposal_history entry {} expected final_edl to contain {:?}",
                entry.id,
                snippet
            );
        }
    }
    for snippet in &entry.final_edl_must_not_contain {
        if final_edl.contains(snippet) {
            bail!(
                "proposal_history entry {} expected final_edl not to contain {:?}",
                entry.id,
                snippet
            );
        }
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct AssessorProposalFixture {
    id: String,
    final_edl: String,
    #[serde(default)]
    proposal_history: Vec<AssessorProposalHistoryEntry>,
}

#[derive(Debug, serde::Deserialize)]
struct AssessorProposalHistoryEntry {
    id: String,
    status: String,
    recommendation: String,
    #[serde(default)]
    edl_contains: Vec<String>,
    #[serde(default)]
    final_edl_must_contain: Vec<String>,
    #[serde(default)]
    final_edl_must_not_contain: Vec<String>,
}

/// Applies a mounted real-project rough-assembly fixture. This promotes
/// the synthetic rough-assembly golden cases into the live tier by
/// validating the same final EDL structure against a mounted project
/// timeline instead of a generated golden project.
struct RealProjectRoughAssemblyFixture;

#[async_trait]
impl Scenario for RealProjectRoughAssemblyFixture {
    fn id(&self) -> &'static str {
        "real::rough_assembly_fixture"
    }

    fn description(&self) -> &'static str {
        "mounted rough-assembly fixture applies final EDL and validates editorial/delivery structure."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        let override_path = rough_assembly_fixture_override_path();
        let min_fixtures = env_usize("AWIDAT_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES", 0)?;
        run_rough_assembly_fixture_at(
            &root,
            override_path.as_deref(),
            self.id(),
            started,
            min_fixtures,
        )
    }
}

fn run_rough_assembly_fixture_at(
    root: &std::path::Path,
    override_path: Option<&std::path::Path>,
    scenario_id: &str,
    started: Instant,
    min_fixtures: usize,
) -> Result<ScenarioOutcome> {
    let paths = rough_assembly_fixture_paths(root, override_path)?;
    if paths.len() < min_fixtures {
        return Ok(ScenarioOutcome {
            id: scenario_id.into(),
            status: ScenarioStatus::Fail,
            elapsed: started.elapsed(),
            message: format!(
                "expected at least {min_fixtures} rough assembly fixture(s), found {}",
                paths.len()
            ),
        });
    }
    if paths.is_empty() {
        return Ok(ScenarioOutcome {
            id: scenario_id.into(),
            status: ScenarioStatus::Skipped,
            elapsed: started.elapsed(),
            message: "no rough assembly fixture found; set AWIDAT_REAL_ROUGH_ASSEMBLY_FIXTURE or add .awidat/eval/rough-assembly-flow.json / .awidat/eval/rough-assemblies/*.json".into(),
        });
    }
    let elapsed = started.elapsed();
    let mut applied_total = 0;
    for path in &paths {
        match load_rough_assembly_fixture(path)
            .and_then(|fixture| validate_rough_assembly_fixture(root, &fixture))
        {
            Ok(applied) => {
                applied_total += applied;
            }
            Err(err) => {
                return Ok(ScenarioOutcome {
                    id: scenario_id.into(),
                    status: ScenarioStatus::Fail,
                    elapsed,
                    message: format!("{}: {err:#}", path.display()),
                });
            }
        }
    }
    let message = if paths.len() == 1 {
        format!(
            "validated {} with {applied_total} applied op(s)",
            paths[0].display()
        )
    } else {
        format!(
            "validated {} fixture(s) with {applied_total} applied op(s)",
            paths.len()
        )
    };
    Ok(ScenarioOutcome {
        id: scenario_id.into(),
        status: ScenarioStatus::Pass,
        elapsed,
        message,
    })
}

fn rough_assembly_fixture_override_path() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("AWIDAT_REAL_ROUGH_ASSEMBLY_FIXTURE")
        && !path.trim().is_empty()
    {
        return Some(std::path::PathBuf::from(path));
    }
    None
}

fn rough_assembly_fixture_paths(
    root: &std::path::Path,
    override_path: Option<&std::path::Path>,
) -> Result<Vec<std::path::PathBuf>> {
    if let Some(path) = override_path {
        return collect_json_fixture_paths(path);
    }
    let default = root
        .join(".awidat")
        .join("eval")
        .join("rough-assembly-flow.json");
    let directory = root.join(".awidat").join("eval").join("rough-assemblies");
    collect_default_json_fixture_paths(&default, &directory)
}

fn collect_default_json_fixture_paths(
    default_file: &std::path::Path,
    default_directory: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    if default_file.is_file() {
        paths.push(default_file.to_path_buf());
    }
    if default_directory.is_dir() {
        paths.extend(collect_json_fixture_paths(default_directory)?);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn collect_json_fixture_paths(path: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    if path.is_dir() {
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
            let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
            let candidate = entry.path();
            if candidate.is_file()
                && candidate
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                paths.push(candidate);
            }
        }
        paths.sort();
        Ok(paths)
    } else {
        Ok(vec![path.to_path_buf()])
    }
}

fn load_rough_assembly_fixture(path: &std::path::Path) -> Result<RoughAssemblyFixture> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn validate_rough_assembly_fixture(
    root: &std::path::Path,
    fixture: &RoughAssemblyFixture,
) -> Result<usize> {
    if fixture.id.trim().is_empty() {
        bail!("rough assembly fixture id is empty");
    }
    if fixture.final_edl.trim().is_empty() {
        bail!("rough assembly fixture final_edl is empty");
    }
    for forbidden in &fixture.expect.forbidden_ops {
        if fixture.final_edl.contains(&format!("*** {forbidden}")) {
            bail!("forbidden op {forbidden:?} appeared in final_edl");
        }
    }
    for entry in &fixture.expect.proposal_history {
        validate_rough_assembly_proposal_history_entry(entry, &fixture.final_edl)?;
    }

    let envelope = parse_edl(&fixture.final_edl).context("parse rough assembly final_edl")?;
    let project = Project::read(root).context("read project for rough assembly fixture")?;
    let ctx = EdlAnchorContext::with_project_root(root);
    let (timeline, outcome) =
        apply_edl_envelope(&project.timeline, &envelope, &ctx).context("apply final_edl")?;
    if outcome.applied.is_empty() {
        bail!("final_edl applied zero operations");
    }
    validate_rough_assembly_expectations(&timeline, &outcome, &fixture.expect)?;
    Ok(outcome.applied.len())
}

fn validate_rough_assembly_expectations(
    timeline: &Timeline,
    outcome: &awidat_core::edl::ApplyOutcome,
    expect: &RoughAssemblyExpect,
) -> Result<()> {
    if let Some(expected) = expect.clip_count
        && clip_count(timeline) != expected
    {
        bail!("expected {expected} clips, got {}", clip_count(timeline));
    }
    let tolerance = expect.tolerance_s.unwrap_or(0.01);
    for expected in &expect.clip_ranges {
        let Some((start_s, duration_s)) = clip_range_by_uuid(timeline, &expected.uuid) else {
            bail!("expected clip uuid {:?} not found", expected.uuid);
        };
        if (start_s - expected.start_s).abs() > tolerance {
            bail!(
                "clip {} start_s expected {:.3}, got {:.3}",
                expected.uuid,
                expected.start_s,
                start_s
            );
        }
        if (duration_s - expected.duration_s).abs() > tolerance {
            bail!(
                "clip {} duration_s expected {:.3}, got {:.3}",
                expected.uuid,
                expected.duration_s,
                duration_s
            );
        }
    }
    for required in &expect.applied_contains {
        if !outcome
            .applied
            .iter()
            .any(|applied| applied.description.contains(required))
        {
            bail!("applied-op log did not contain {required:?}");
        }
    }
    assert_live_cut_boundaries(timeline, &expect.cut_boundaries)?;
    assert_live_output_format(timeline, expect.output_format.as_ref())?;
    assert_live_package_metadata(timeline, expect.package_metadata.as_ref())?;
    Ok(())
}

fn assert_live_cut_boundaries(
    timeline: &Timeline,
    expected_boundaries: &[ExpectedLiveCutBoundary],
) -> Result<()> {
    if expected_boundaries.is_empty() {
        return Ok(());
    }
    let Some(awidat) = timeline.metadata.awidat.as_ref() else {
        bail!("expected cut boundary metadata, but timeline has no awidat metadata");
    };
    for expected in expected_boundaries {
        let key = cut_boundary_key(&expected.from, &expected.to);
        let Some(spec) = awidat.cut_boundaries.get(&key) else {
            bail!("expected cut boundary {key:?} not found");
        };
        if spec.cut_type != expected.cut_type {
            bail!(
                "cut boundary {key} cut_type expected {:?}, got {:?}",
                expected.cut_type,
                spec.cut_type
            );
        }
        if spec.intent != expected.intent {
            bail!(
                "cut boundary {key} intent expected {:?}, got {:?}",
                expected.intent,
                spec.intent
            );
        }
        if let Some(audio_relation) = expected.audio_relation
            && spec.audio_relation != audio_relation
        {
            bail!(
                "cut boundary {key} audio_relation expected {:?}, got {:?}",
                audio_relation,
                spec.audio_relation
            );
        }
        if let Some(reason) = &expected.reason_contains
            && !spec
                .reason
                .as_deref()
                .is_some_and(|actual| actual.contains(reason))
        {
            bail!(
                "cut boundary {key} reason did not contain {:?}: {:?}",
                reason,
                spec.reason
            );
        }
    }
    Ok(())
}

fn assert_live_output_format(
    timeline: &Timeline,
    expected: Option<&ExpectedLiveOutputFormat>,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(awidat) = timeline.metadata.awidat.as_ref() else {
        bail!("expected output_format metadata, but timeline has no awidat metadata");
    };
    let Some(actual) = awidat.extra.get("output_format") else {
        bail!("expected output_format metadata, but none was found");
    };
    assert_live_json_string(
        actual,
        "output_format",
        "aspect_ratio",
        Some(&expected.aspect_ratio),
    )?;
    assert_live_json_string(
        actual,
        "output_format",
        "platform",
        expected.platform.as_deref(),
    )?;
    assert_live_json_string(
        actual,
        "output_format",
        "safe_area",
        expected.safe_area.as_deref(),
    )?;
    Ok(())
}

fn assert_live_package_metadata(
    timeline: &Timeline,
    expected: Option<&ExpectedLivePackageMetadata>,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(awidat) = timeline.metadata.awidat.as_ref() else {
        bail!("expected package_metadata, but timeline has no awidat metadata");
    };
    let Some(actual) = awidat.extra.get("package_metadata") else {
        bail!("expected package_metadata, but none was found");
    };
    assert_live_json_string(
        actual,
        "package_metadata",
        "platform",
        expected.platform.as_deref(),
    )?;
    assert_live_json_string(
        actual,
        "package_metadata",
        "title",
        expected.title.as_deref(),
    )?;
    if let Some(description) = &expected.description {
        assert_live_json_string(actual, "package_metadata", "description", Some(description))?;
    }
    if let Some(needle) = &expected.description_contains {
        let got = actual.get("description").and_then(|value| value.as_str());
        if !got.is_some_and(|text| text.contains(needle)) {
            bail!("package_metadata description expected to contain {needle:?}, got {got:?}");
        }
    }
    if let Some(expected_tags) = &expected.tags_contains {
        let got = actual.get("tags").and_then(|value| value.as_str());
        for expected_tag in expected_tags {
            if !got.is_some_and(|tags| tags.split(',').any(|tag| tag.trim() == expected_tag)) {
                bail!("package_metadata tags expected to contain {expected_tag:?}, got {got:?}");
            }
        }
    }
    Ok(())
}

fn assert_live_json_string(
    actual: &serde_json::Value,
    namespace: &str,
    field: &str,
    expected: Option<&str>,
) -> Result<()> {
    match expected {
        Some(expected) => {
            let got = actual.get(field).and_then(|value| value.as_str());
            if got != Some(expected) {
                bail!("{namespace} field {field:?} expected {expected:?}, got {got:?}");
            }
        }
        None => {
            if actual.get(field).is_some_and(|value| !value.is_null()) {
                bail!(
                    "{namespace} field {field:?} expected null or absent, got {:?}",
                    actual.get(field)
                );
            }
        }
    }
    Ok(())
}

fn validate_rough_assembly_proposal_history_entry(
    entry: &ExpectedRoughAssemblyProposalHistory,
    final_edl: &str,
) -> Result<()> {
    if entry.id.trim().is_empty() {
        bail!("proposal_history entry has empty id");
    }
    if entry.status != "accepted" && entry.status != "rejected" {
        bail!(
            "proposal_history entry {} has unsupported status {:?}",
            entry.id,
            entry.status
        );
    }
    if entry.recommendation.trim().is_empty() {
        bail!(
            "proposal_history entry {} has empty recommendation",
            entry.id
        );
    }
    if let Some(reason) = &entry.reason_contains
        && reason.trim().is_empty()
    {
        bail!(
            "proposal_history entry {} has empty reason_contains",
            entry.id
        );
    }
    if entry.edl_contains.is_empty() {
        bail!(
            "proposal_history entry {} must include at least one edl_contains snippet",
            entry.id
        );
    }
    for snippet in &entry.edl_contains {
        if snippet.trim().is_empty() {
            bail!(
                "proposal_history entry {} has empty edl_contains snippet",
                entry.id
            );
        }
    }
    validate_proposal_history_edl_lifecycle(
        &entry.id,
        &entry.status,
        &entry.edl_contains,
        final_edl,
    )?;
    validate_proposal_history_final_edl_snippets(
        &entry.id,
        "final_edl_must_contain",
        &entry.final_edl_must_contain,
    )?;
    validate_proposal_history_final_edl_snippets(
        &entry.id,
        "final_edl_must_not_contain",
        &entry.final_edl_must_not_contain,
    )?;
    for snippet in &entry.final_edl_must_contain {
        if !final_edl.contains(snippet) {
            bail!(
                "proposal_history entry {} expected final_edl to contain {:?}",
                entry.id,
                snippet
            );
        }
    }
    for snippet in &entry.final_edl_must_not_contain {
        if final_edl.contains(snippet) {
            bail!(
                "proposal_history entry {} expected final_edl not to contain {:?}",
                entry.id,
                snippet
            );
        }
    }
    Ok(())
}

fn validate_proposal_history_final_edl_snippets(
    entry_id: &str,
    field: &str,
    snippets: &[String],
) -> Result<()> {
    for snippet in snippets {
        if snippet.trim().is_empty() {
            bail!("proposal_history entry {entry_id} has empty {field} snippet");
        }
    }
    Ok(())
}

fn validate_proposal_history_edl_lifecycle(
    entry_id: &str,
    status: &str,
    edl_contains: &[String],
    final_edl: &str,
) -> Result<()> {
    match status {
        "accepted" => {
            for snippet in edl_contains {
                if !final_edl.contains(snippet) {
                    bail!(
                        "accepted proposal_history entry {entry_id} expected final_edl to contain accepted proposal snippet {snippet:?}"
                    );
                }
            }
        }
        "rejected" => {
            for snippet in edl_contains {
                if final_edl.contains(snippet) {
                    bail!(
                        "rejected proposal_history entry {entry_id} expected final_edl not to contain rejected proposal snippet {snippet:?}"
                    );
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct RoughAssemblyFixture {
    id: String,
    final_edl: String,
    expect: RoughAssemblyExpect,
}

#[derive(Debug, Default, serde::Deserialize)]
struct RoughAssemblyExpect {
    #[serde(default)]
    tolerance_s: Option<f64>,
    #[serde(default)]
    clip_count: Option<usize>,
    #[serde(default)]
    clip_ranges: Vec<ExpectedLiveClipRange>,
    #[serde(default)]
    cut_boundaries: Vec<ExpectedLiveCutBoundary>,
    #[serde(default)]
    output_format: Option<ExpectedLiveOutputFormat>,
    #[serde(default)]
    package_metadata: Option<ExpectedLivePackageMetadata>,
    #[serde(default)]
    proposal_history: Vec<ExpectedRoughAssemblyProposalHistory>,
    #[serde(default)]
    applied_contains: Vec<String>,
    #[serde(default)]
    forbidden_ops: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedLiveClipRange {
    uuid: String,
    start_s: f64,
    duration_s: f64,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedLiveCutBoundary {
    from: String,
    to: String,
    cut_type: CutType,
    intent: String,
    #[serde(default)]
    audio_relation: Option<AudioRelation>,
    #[serde(default)]
    reason_contains: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedLiveOutputFormat {
    aspect_ratio: String,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    safe_area: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedLivePackageMetadata {
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    description_contains: Option<String>,
    #[serde(default)]
    tags_contains: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedRoughAssemblyProposalHistory {
    id: String,
    status: String,
    recommendation: String,
    #[serde(default)]
    reason_contains: Option<String>,
    #[serde(default)]
    edl_contains: Vec<String>,
    #[serde(default)]
    final_edl_must_contain: Vec<String>,
    #[serde(default)]
    final_edl_must_not_contain: Vec<String>,
}

/// `assess_edit_quality` against a real indexed shot sidecar. This is
/// the live gate for the visual metadata path: synthetic product tests
/// prove the schema, while this catches stale real corpora that have not
/// been re-indexed with enough composition / match-candidate fields.
struct RealProjectEditQualityVisualContext;

#[async_trait]
impl Scenario for RealProjectEditQualityVisualContext {
    fn id(&self) -> &'static str {
        "real::edit_quality_visual_context"
    }

    fn description(&self) -> &'static str {
        "assess_edit_quality against real shot sidecars surfaces visual context metadata."
    }

    async fn run(&self) -> Result<ScenarioOutcome> {
        let started = Instant::now();
        let Some(root) = real_project_root() else {
            return Ok(skip_no_real_project(self.id(), started));
        };
        let coverage = real_visual_context_coverage(&root);
        let thresholds = match VisualContextThresholds::from_env() {
            Ok(thresholds) => thresholds,
            Err(err) => {
                return Ok(ScenarioOutcome {
                    id: self.id().into(),
                    status: ScenarioStatus::Fail,
                    elapsed: started.elapsed(),
                    message: format!("invalid visual metadata threshold configuration: {err:#}"),
                });
            }
        };
        if !coverage.meets(thresholds) {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!(
                    "visual metadata coverage below threshold: {}; required at least {} metadata shots, {:.0}% metadata coverage, {} match-candidate shots, {} model-composition shots, {} composition-model regions, and at most {} invalid composition-model regions",
                    coverage.summary(),
                    thresholds.min_metadata_shots,
                    thresholds.min_metadata_ratio * 100.0,
                    thresholds.min_match_candidate_shots,
                    thresholds.min_model_composition_shots,
                    thresholds.min_composition_model_regions,
                    thresholds.max_invalid_composition_model_regions,
                ),
            });
        };
        let Some((asset_id, at_s)) = coverage.probe.clone() else {
            return Ok(ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed: started.elapsed(),
                message: format!(
                    "visual metadata coverage had no probe despite meeting thresholds: {}",
                    coverage.summary()
                ),
            });
        };
        let out = AssessEditQualityTool
            .handle(
                make_call(
                    "assess_edit_quality",
                    serde_json::json!({
                        "asset_id": asset_id,
                        "at_s": at_s,
                        "kind": "cut",
                        "objective": "real corpus visual metadata check",
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
                let visual = &body["visual_context"];
                if visual.is_null() {
                    return Ok(ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!("visual_context was null for {asset_id} at {at_s:.2}s"),
                    });
                }
                let fields = [
                    "subject_zone",
                    "face_zone",
                    "headroom",
                    "look_space",
                    "match_candidate",
                    "match_cut_score",
                ];
                let surfaced = fields
                    .iter()
                    .copied()
                    .filter(|field| !visual[*field].is_null())
                    .collect::<Vec<_>>();
                if surfaced.is_empty() {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Fail,
                        elapsed,
                        message: format!(
                            "visual_context for {asset_id} at {at_s:.2}s lacked composition/match fields"
                        ),
                    }
                } else {
                    ScenarioOutcome {
                        id: self.id().into(),
                        status: ScenarioStatus::Pass,
                        elapsed,
                        message: format!(
                            "{asset_id} at {at_s:.2}s surfaced {}; {}",
                            surfaced.join(", "),
                            coverage.summary(),
                        ),
                    }
                }
            }
            Err(e) => ScenarioOutcome {
                id: self.id().into(),
                status: ScenarioStatus::Fail,
                elapsed,
                message: format!("assess_edit_quality errored: {e}"),
            },
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct VisualContextThresholds {
    min_metadata_shots: usize,
    min_metadata_ratio: f64,
    min_match_candidate_shots: usize,
    min_model_composition_shots: usize,
    min_composition_model_regions: usize,
    max_invalid_composition_model_regions: usize,
}

impl Default for VisualContextThresholds {
    fn default() -> Self {
        Self {
            min_metadata_shots: 3,
            min_metadata_ratio: 0.25,
            min_match_candidate_shots: 1,
            min_model_composition_shots: 0,
            min_composition_model_regions: 0,
            max_invalid_composition_model_regions: 0,
        }
    }
}

impl VisualContextThresholds {
    fn from_env() -> Result<Self> {
        let default = Self::default();
        Ok(Self {
            min_metadata_shots: env_usize(
                "AWIDAT_REAL_VISUAL_MIN_METADATA_SHOTS",
                default.min_metadata_shots,
            )?,
            min_metadata_ratio: env_ratio(
                "AWIDAT_REAL_VISUAL_MIN_METADATA_RATIO",
                default.min_metadata_ratio,
            )?,
            min_match_candidate_shots: env_usize(
                "AWIDAT_REAL_VISUAL_MIN_MATCH_CANDIDATE_SHOTS",
                default.min_match_candidate_shots,
            )?,
            min_model_composition_shots: env_usize(
                "AWIDAT_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS",
                default.min_model_composition_shots,
            )?,
            min_composition_model_regions: env_usize(
                "AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS",
                default.min_composition_model_regions,
            )?,
            max_invalid_composition_model_regions: env_usize(
                "AWIDAT_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS",
                default.max_invalid_composition_model_regions,
            )?,
        })
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
struct VisualContextCoverage {
    total_shots: usize,
    metadata_shots: usize,
    match_candidate_shots: usize,
    model_composition_shots: usize,
    composition_model_regions: usize,
    invalid_composition_model_regions: usize,
    probe: Option<(String, f64)>,
}

impl VisualContextCoverage {
    fn metadata_ratio(&self) -> f64 {
        if self.total_shots == 0 {
            0.0
        } else {
            self.metadata_shots as f64 / self.total_shots as f64
        }
    }

    fn meets(&self, thresholds: VisualContextThresholds) -> bool {
        self.total_shots > 0
            && self.metadata_shots >= thresholds.min_metadata_shots
            && self.metadata_ratio() >= thresholds.min_metadata_ratio
            && self.match_candidate_shots >= thresholds.min_match_candidate_shots
            && self.model_composition_shots >= thresholds.min_model_composition_shots
            && self.composition_model_regions >= thresholds.min_composition_model_regions
            && self.invalid_composition_model_regions
                <= thresholds.max_invalid_composition_model_regions
            && self.probe.is_some()
    }

    fn summary(&self) -> String {
        format!(
            "{} metadata shots / {} total ({:.0}%), {} with match candidates, {} with model composition, {} valid composition-model regions, {} invalid composition-model regions",
            self.metadata_shots,
            self.total_shots,
            self.metadata_ratio() * 100.0,
            self.match_candidate_shots,
            self.model_composition_shots,
            self.composition_model_regions,
            self.invalid_composition_model_regions,
        )
    }
}

fn real_visual_context_coverage(root: &std::path::Path) -> VisualContextCoverage {
    let Ok(entries) = walk_indexer(root, "shot") else {
        return VisualContextCoverage::default();
    };
    let mut sidecars = entries.collect::<Vec<_>>();
    sidecars.sort_by(|(left, _), (right, _)| left.cmp(right));
    let mut coverage = visual_context_coverage(sidecars);
    let composition_model = composition_model_region_counts(root);
    coverage.composition_model_regions = composition_model.valid;
    coverage.invalid_composition_model_regions = composition_model.invalid;
    coverage
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct CompositionModelRegionCounts {
    valid: usize,
    invalid: usize,
}

fn composition_model_region_counts(root: &std::path::Path) -> CompositionModelRegionCounts {
    let Ok(entries) = walk_indexer(root, "composition-model") else {
        return CompositionModelRegionCounts::default();
    };
    let mut counts = CompositionModelRegionCounts::default();
    for (_, body) in entries {
        let Some(regions) = body
            .pointer("/data/regions")
            .or_else(|| body.get("regions"))
            .and_then(|value| value.as_array())
        else {
            continue;
        };
        for region in regions {
            if is_valid_composition_model_region(region) {
                counts.valid += 1;
            } else {
                counts.invalid += 1;
            }
        }
    }
    counts
}

fn is_valid_composition_model_region(region: &serde_json::Value) -> bool {
    let Some(start_s) = region.get("start_s").and_then(|value| value.as_f64()) else {
        return false;
    };
    let Some(end_s) = region.get("end_s").and_then(|value| value.as_f64()) else {
        return false;
    };
    if end_s <= start_s {
        return false;
    }
    if !region
        .get("composition_source")
        .and_then(|value| value.as_str())
        .is_some_and(|source| source.starts_with("model:"))
    {
        return false;
    }
    if !region
        .get("composition_confidence")
        .and_then(|value| value.as_f64())
        .is_some_and(|confidence| (0.0..=1.0).contains(&confidence))
    {
        return false;
    }
    model_label_is_allowed(
        region.get("subject_role").and_then(|value| value.as_str()),
        &[
            "environment",
            "background_person",
            "featured_subject",
            "primary_speaker",
            "secondary_subject",
            "object_detail",
        ],
    ) && model_label_is_allowed(
        region.get("depth_layer").and_then(|value| value.as_str()),
        &["foreground", "midground", "background", "mixed_depth"],
    ) && model_label_is_allowed(
        region.get("framing").and_then(|value| value.as_str()),
        &[
            "extreme_close_up",
            "single_close",
            "single_medium",
            "wide_context",
            "two_shot",
            "group",
            "insert",
        ],
    )
}

fn model_label_is_allowed(value: Option<&str>, allowed: &[&str]) -> bool {
    value.is_some_and(|value| allowed.contains(&value))
}

fn visual_context_coverage(sidecars: Vec<(String, serde_json::Value)>) -> VisualContextCoverage {
    let mut coverage = VisualContextCoverage::default();
    for (asset_id, body) in sidecars {
        let Some(shots) = body
            .pointer("/data/shots")
            .or_else(|| body.get("shots"))
            .and_then(|value| value.as_array())
        else {
            continue;
        };
        for shot in shots {
            coverage.total_shots += 1;
            if shot_has_real_visual_metadata(shot) {
                coverage.metadata_shots += 1;
                if coverage.probe.is_none() {
                    let start = shot
                        .get("start_s")
                        .or_else(|| shot.get("start"))
                        .and_then(|value| value.as_f64())
                        .unwrap_or(0.0);
                    let end = shot
                        .get("end_s")
                        .or_else(|| shot.get("end"))
                        .and_then(|value| value.as_f64())
                        .unwrap_or(start);
                    coverage.probe = Some((asset_id.clone(), (start + end) / 2.0));
                }
            }
            if shot_has_match_candidates(shot) {
                coverage.match_candidate_shots += 1;
            }
            if shot_has_model_composition(shot) {
                coverage.model_composition_shots += 1;
            }
        }
    }
    coverage
}

fn parse_env_usize(key: &str, value: Option<&str>, default: usize) -> Result<usize> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.trim().is_empty() {
        return Ok(default);
    }
    value
        .parse::<usize>()
        .with_context(|| format!("{key} must be a non-negative integer, got {value:?}"))
}

fn env_usize(key: &str, default: usize) -> Result<usize> {
    parse_env_usize(key, std::env::var(key).ok().as_deref(), default)
}

fn parse_env_ratio(key: &str, value: Option<&str>, default: f64) -> Result<f64> {
    let Some(value) = value else {
        return Ok(default);
    };
    if value.trim().is_empty() {
        return Ok(default);
    }
    let ratio = value
        .parse::<f64>()
        .with_context(|| format!("{key} must be a ratio in [0, 1], got {value:?}"))?;
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        bail!("{key} must be a finite ratio in [0, 1], got {value:?}");
    }
    Ok(ratio)
}

fn env_ratio(key: &str, default: f64) -> Result<f64> {
    parse_env_ratio(key, std::env::var(key).ok().as_deref(), default)
}

fn shot_has_real_visual_metadata(shot: &serde_json::Value) -> bool {
    [
        "subject_zone",
        "face_zone",
        "headroom",
        "look_space",
        "composition_source",
        "composition_confidence",
        "subject_role",
        "depth_layer",
        "framing",
    ]
    .iter()
    .any(|key| shot.get(*key).is_some_and(|value| !value.is_null()))
        || shot_has_match_candidates(shot)
        || shot
            .get("match_cut_score")
            .is_some_and(|value| !value.is_null())
}

fn shot_has_match_candidates(shot: &serde_json::Value) -> bool {
    shot.get("match_candidates")
        .and_then(|value| value.as_array())
        .is_some_and(|candidates| !candidates.is_empty())
}

fn shot_has_model_composition(shot: &serde_json::Value) -> bool {
    shot.get("composition_source")
        .and_then(|value| value.as_str())
        .is_some_and(|source| source.starts_with("model:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_all;

    fn expect_anyhow_err<T>(result: Result<T>, message: &str) -> anyhow::Error {
        match result {
            Ok(_) => panic!("{message}"),
            Err(err) => err,
        }
    }

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

    #[test]
    fn real_corpus_includes_edit_quality_visual_context_check() {
        let ids = real_corpus()
            .iter()
            .map(|scenario| scenario.id())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"real::edit_quality_visual_context"));
        assert!(ids.contains(&"real::transition_planner_fixture"));
        assert!(ids.contains(&"real::assessor_proposal_fixture"));
        assert!(ids.contains(&"real::rough_assembly_fixture"));
    }

    #[tokio::test]
    async fn transition_planner_fixture_default_path_runs_against_temp_project() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-transition-default-path",
            &[
                ClipSpec::video("clip-a", "clip-a", "raw/a.mp4", 0.0, 5.0),
                ClipSpec::video("clip-b", "clip-b", "raw/b.mp4", 1.0, 4.0),
            ],
        )
        .unwrap();
        let fixture_path = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("transition-planner-flow.json");
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(
            &fixture_path,
            include_str!("../fixtures/real/transition-planner-flow.sample.json"),
        )
        .unwrap();

        let outcome = run_transition_planner_fixture_at(
            dir.path(),
            None,
            "real::transition_planner_fixture",
            Instant::now(),
            0,
        )
        .await
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("transition planner"));
    }

    #[tokio::test]
    async fn checked_in_transition_planner_sample_fixture_validates() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-transition-sample",
            &[
                ClipSpec::video("clip-a", "clip-a", "raw/a.mp4", 0.0, 5.0),
                ClipSpec::video("clip-b", "clip-b", "raw/b.mp4", 1.0, 4.0),
            ],
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("real")
            .join("transition-planner-flow.sample.json");
        let fixture = load_transition_planner_fixture(&path).unwrap();

        validate_transition_planner_fixture(dir.path(), &fixture)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn checked_in_transition_planner_directory_samples_validate() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-transition-directory-samples",
            &[
                ClipSpec::video("clip-a", "clip-a", "raw/a.mp4", 0.0, 5.0),
                ClipSpec::video("clip-b", "clip-b", "raw/b.mp4", 1.0, 4.0),
            ],
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("real")
            .join("transition-planners");

        let outcome = run_transition_planner_fixture_at(
            dir.path(),
            Some(&path),
            "real::transition_planner_fixture",
            Instant::now(),
            2,
        )
        .await
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("transition planner"));
    }

    #[test]
    fn transition_planner_fixture_rejects_forbidden_edl_snippets() {
        let fixture: TransitionPlannerFixture = serde_json::from_value(serde_json::json!({
            "id": "hard-cut-forbids-visible-transition",
            "between": {
                "from": "clip-a",
                "to": "clip-b"
            },
            "expect": {
                "decision": "hard_cut",
                "edl_contains": ["*** Set Cut Intent", "+ cut_type: hard_cut"],
                "edl_must_not_contain": ["*** Insert Transition"],
                "apply_edl_fragment": false
            }
        }))
        .unwrap();
        let plan = serde_json::json!({
            "recommended": {
                "decision": "hard_cut",
                "intent": "clean_story_rhythm",
                "reason": "The transition context does not show a named job."
            },
            "edl_fragment": "*** Begin EDL\n*** Set Cut Intent\n@@ between: clip_uuid=clip-a and clip_uuid=clip-b\n+ cut_type: hard_cut\n*** Insert Transition\n@@ between: clip_uuid=clip-a and clip_uuid=clip-b\n+ id: awidat.motion_blur\n+ duration_s: 0.180\n*** End EDL"
        });

        let err = expect_anyhow_err(
            assert_transition_planner_expectations(
                std::path::Path::new("/tmp/nonexistent-awidat-fixture"),
                &fixture,
                &plan,
            ),
            "forbidden EDL snippet should fail validation",
        );

        assert!(
            err.to_string().contains("edl_fragment must not contain"),
            "{err:#}"
        );
    }

    #[test]
    fn transition_planner_fixture_requires_hard_cut_edl_proofs() {
        let fixture: TransitionPlannerFixture = serde_json::from_value(serde_json::json!({
            "id": "hard-cut-without-proofs",
            "between": {
                "from": "clip-a",
                "to": "clip-b"
            },
            "expect": {
                "decision": "hard_cut",
                "apply_edl_fragment": false
            }
        }))
        .unwrap();
        let plan = serde_json::json!({
            "recommended": {
                "decision": "hard_cut",
                "intent": "clean_story_rhythm",
                "reason": "The transition context does not show a named job."
            },
            "edl_fragment": "*** Begin EDL\n*** Set Cut Intent\n@@ between: clip_uuid=clip-a and clip_uuid=clip-b\n+ cut_type: hard_cut\n*** End EDL"
        });

        let err = expect_anyhow_err(
            assert_transition_planner_expectations(
                std::path::Path::new("/tmp/nonexistent-awidat-fixture"),
                &fixture,
                &plan,
            ),
            "hard-cut fixtures should require positive and negative EDL proofs",
        );

        assert!(
            err.to_string()
                .contains("hard_cut transition planner fixtures must include"),
            "{err:#}"
        );
    }

    #[test]
    fn transition_planner_fixture_requires_visible_transition_edl_proofs() {
        let fixture: TransitionPlannerFixture = serde_json::from_value(serde_json::json!({
            "id": "transition-without-edl-proof",
            "between": {
                "from": "clip-a",
                "to": "clip-b"
            },
            "expect": {
                "decision": "transition",
                "transition_id": "awidat.whip_pan_left",
                "edl_contains": ["*** Insert Transition"],
                "apply_edl_fragment": false
            }
        }))
        .unwrap();
        let plan = serde_json::json!({
            "recommended": {
                "decision": "transition",
                "id": "awidat.whip_pan_left",
                "intent": "hide_motion_jump",
                "reason": "hide_motion_jump gives this transition a named job."
            },
            "edl_fragment": "*** Begin EDL\n*** Insert Transition\n@@ between: clip_uuid=clip-a and clip_uuid=clip-b\n+ id: awidat.whip_pan_left\n*** End EDL"
        });

        let err = expect_anyhow_err(
            assert_transition_planner_expectations(
                std::path::Path::new("/tmp/nonexistent-awidat-fixture"),
                &fixture,
                &plan,
            ),
            "visible transition fixtures should prove their transition id in EDL",
        );

        assert!(
            err.to_string()
                .contains("transition planner fixtures with decision transition must include"),
            "{err:#}"
        );
    }

    #[test]
    fn assessor_proposal_fixture_validates_rejected_and_followup_edl_against_project() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-fixture",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let fixture: AssessorProposalFixture = serde_json::from_value(serde_json::json!({
            "id": "real-proposal-flow",
            "final_edl": "*** Begin EDL\n*** Set Audio Trail\n@@ anchor: clip_uuid=expert\n+ trail_s: 0.45\n+ reason: accepted follow-up L-cut after rejecting early J-cut\n+ confidence: 0.86\n*** Set Cut Intent\n@@ between: clip_uuid=expert and clip_uuid=reaction\n+ cut_type: l_cut\n+ intent: hold_expert_thought\n+ audio_relation: audio_trails\n+ confidence: 0.86\n+ reason: accepted follow-up L-cut after rejecting early J-cut\n*** End EDL\n",
            "proposal_history": [
                {
                    "id": "j-cut-1",
                    "status": "rejected",
                    "recommendation": "Set Audio Lead",
                    "edl_contains": ["*** Set Audio Lead", "+ cut_type: j_cut"],
                    "final_edl_must_not_contain": ["*** Set Audio Lead", "+ cut_type: j_cut"]
                },
                {
                    "id": "l-cut-follow-up",
                    "status": "accepted",
                    "recommendation": "Set Audio Trail",
                    "edl_contains": ["*** Set Audio Trail", "+ cut_type: l_cut"],
                    "final_edl_must_contain": ["*** Set Audio Trail", "+ cut_type: l_cut"]
                }
            ]
        }))
        .unwrap();

        let applied = validate_assessor_proposal_fixture(dir.path(), &fixture).unwrap();

        assert_eq!(applied, 2);
    }

    #[test]
    fn assessor_proposal_fixture_requires_reject_and_accept_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-lifecycle-required",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let fixture: AssessorProposalFixture = serde_json::from_value(serde_json::json!({
            "id": "accepted-only-flow",
            "final_edl": "*** Begin EDL\n*** Set Audio Trail\n@@ anchor: clip_uuid=expert\n+ trail_s: 0.45\n*** End EDL\n",
            "proposal_history": [
                {
                    "id": "l-cut-follow-up",
                    "status": "accepted",
                    "recommendation": "Set Audio Trail",
                    "edl_contains": ["*** Set Audio Trail"]
                }
            ]
        }))
        .unwrap();

        let err = expect_anyhow_err(
            validate_assessor_proposal_fixture(dir.path(), &fixture),
            "assessor proposal fixtures must include rejected and accepted entries",
        );

        assert!(
            err.to_string()
                .contains("must include at least one rejected and one accepted"),
            "{err:#}"
        );
    }

    #[test]
    fn assessor_proposal_history_rejects_rejected_snippets_in_final_edl() {
        let entry: AssessorProposalHistoryEntry = serde_json::from_value(serde_json::json!({
            "id": "rejected-j-cut",
            "status": "rejected",
            "recommendation": "Set Audio Lead",
            "edl_contains": ["*** Set Audio Lead"]
        }))
        .unwrap();

        let err = expect_anyhow_err(
            validate_assessor_proposal_history_entry(
                &entry,
                "*** Begin EDL\n*** Set Audio Lead\n@@ anchor: clip_uuid=expert\n+ lead_s: 0.4\n*** End EDL\n",
            ),
            "rejected proposal snippets must be absent from final EDL",
        );

        assert!(err.to_string().contains("rejected proposal"), "{err:#}");
    }

    #[test]
    fn assessor_proposal_history_requires_accepted_snippets_in_final_edl() {
        let entry: AssessorProposalHistoryEntry = serde_json::from_value(serde_json::json!({
            "id": "accepted-l-cut",
            "status": "accepted",
            "recommendation": "Set Audio Trail",
            "edl_contains": ["*** Set Audio Trail"]
        }))
        .unwrap();

        let err = expect_anyhow_err(
            validate_assessor_proposal_history_entry(
                &entry,
                "*** Begin EDL\n*** Set Cut Intent\n@@ between: clip_uuid=expert and clip_uuid=reaction\n+ cut_type: hard_cut\n*** End EDL\n",
            ),
            "accepted proposal snippets must be present in final EDL",
        );

        assert!(err.to_string().contains("accepted proposal"), "{err:#}");
    }

    #[test]
    fn assessor_proposal_history_requires_edl_snippets() {
        let entry: AssessorProposalHistoryEntry = serde_json::from_value(serde_json::json!({
            "id": "empty-snippet-proof",
            "status": "accepted",
            "recommendation": "Set Audio Trail"
        }))
        .unwrap();

        let err = expect_anyhow_err(
            validate_assessor_proposal_history_entry(
                &entry,
                "*** Begin EDL\n*** Set Audio Trail\n@@ anchor: clip_uuid=expert\n+ trail_s: 0.45\n*** End EDL\n",
            ),
            "proposal history without snippets should not validate",
        );

        assert!(
            err.to_string().contains("at least one edl_contains"),
            "{err:#}"
        );
    }

    #[test]
    fn assessor_proposal_history_rejects_empty_final_edl_assertions() {
        let entry: AssessorProposalHistoryEntry = serde_json::from_value(serde_json::json!({
            "id": "empty-final-proof",
            "status": "accepted",
            "recommendation": "Set Audio Trail",
            "edl_contains": ["*** Set Audio Trail"],
            "final_edl_must_contain": [""]
        }))
        .unwrap();

        let err = expect_anyhow_err(
            validate_assessor_proposal_history_entry(
                &entry,
                "*** Begin EDL\n*** Set Audio Trail\n@@ anchor: clip_uuid=expert\n+ trail_s: 0.45\n*** End EDL\n",
            ),
            "empty final_edl assertion snippets should not validate",
        );

        assert!(
            err.to_string().contains("empty final_edl_must_contain"),
            "{err:#}"
        );
    }

    #[test]
    fn checked_in_assessor_proposal_sample_fixture_validates() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-sample",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("real")
            .join("assessor-proposal-flow.sample.json");
        let fixture = load_assessor_proposal_fixture(&path).unwrap();

        let applied = validate_assessor_proposal_fixture(dir.path(), &fixture).unwrap();

        assert_eq!(applied, 2);
    }

    #[test]
    fn checked_in_assessor_proposal_directory_samples_validate() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-directory-samples",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("real")
            .join("assessor-proposals");

        let outcome = run_assessor_proposal_fixture_at(
            dir.path(),
            Some(&path),
            "real::assessor_proposal_fixture",
            Instant::now(),
            1,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("fixture"));
    }

    #[test]
    fn assessor_proposal_fixture_default_path_runs_against_temp_project() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-default-path",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let fixture_path = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("assessor-proposal-flow.json");
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(
            &fixture_path,
            include_str!("../fixtures/real/assessor-proposal-flow.sample.json"),
        )
        .unwrap();

        let outcome = run_assessor_proposal_fixture_at(
            dir.path(),
            None,
            "real::assessor_proposal_fixture",
            Instant::now(),
            0,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("2 applied op"));
    }

    #[test]
    fn assessor_proposal_fixture_directory_runs_all_json_fixtures() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-directory",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let fixture_dir = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("assessor-proposals");
        std::fs::create_dir_all(&fixture_dir).unwrap();
        std::fs::write(
            fixture_dir.join("01-follow-up.json"),
            include_str!("../fixtures/real/assessor-proposal-flow.sample.json"),
        )
        .unwrap();
        std::fs::write(
            fixture_dir.join("02-follow-up.json"),
            include_str!("../fixtures/real/assessor-proposal-flow.sample.json"),
        )
        .unwrap();

        let outcome = run_assessor_proposal_fixture_at(
            dir.path(),
            None,
            "real::assessor_proposal_fixture",
            Instant::now(),
            0,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("2 fixture"));
        assert!(outcome.message.contains("4 applied op"));
    }

    #[test]
    fn assessor_proposal_fixture_minimum_fails_when_corpus_is_undercovered() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-proposal-minimum",
            &[
                ClipSpec::video("expert", "expert", "raw/expert.mp4", 0.0, 8.0),
                ClipSpec::video("reaction", "reaction", "raw/reaction.mp4", 12.0, 5.0),
            ],
        )
        .unwrap();
        let fixture_path = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("assessor-proposal-flow.json");
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(
            &fixture_path,
            include_str!("../fixtures/real/assessor-proposal-flow.sample.json"),
        )
        .unwrap();

        let outcome = run_assessor_proposal_fixture_at(
            dir.path(),
            None,
            "real::assessor_proposal_fixture",
            Instant::now(),
            2,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Fail);
        assert!(outcome.message.contains("expected at least 2"));
        assert!(outcome.message.contains("found 1"));
    }

    #[test]
    fn rough_assembly_fixture_default_path_runs_against_temp_project() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-rough-assembly-default-path",
            &[
                ClipSpec::video("hook-rough", "hook-rough", "raw/hook.mp4", 0.0, 8.0),
                ClipSpec::video("host-reset", "host-reset", "raw/host-reset.mp4", 32.0, 4.0),
                ClipSpec::video(
                    "guest-proof",
                    "guest-proof",
                    "raw/guest-proof.mp4",
                    55.0,
                    9.0,
                ),
                ClipSpec::video(
                    "product-insert",
                    "product-insert",
                    "raw/product-closeup.mp4",
                    4.0,
                    3.0,
                ),
                ClipSpec::video("cta", "cta", "raw/cta.mp4", 80.0, 5.0),
            ],
        )
        .unwrap();
        let fixture_path = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("rough-assembly-flow.json");
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(
            &fixture_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "id": "real-rough-assembly-flow",
                "final_edl": "*** Begin EDL\n*** Trim Clip\n@@ anchor: clip_uuid=hook-rough\n+ start: 3.0\n+ end: 8.0\n*** Delete Clip\n@@ anchor: clip_uuid=host-reset\n*** Set Audio Lead\n@@ anchor: clip_uuid=guest-proof\n+ lead_s: 0.42\n+ reason: let the guest proof begin under the end of the hook instead of adding a visible transition\n+ confidence: 0.87\n*** Set Cut Intent\n@@ between: clip_uuid=hook-rough and clip_uuid=guest-proof\n+ cut_type: j_cut\n+ intent: short_form_speaker_handoff\n+ audio_relation: audio_leads\n+ confidence: 0.87\n+ reason: rough assembly cleanup keeps pace by leading the guest proof under the hook\n*** Set Cut Intent\n@@ between: clip_uuid=guest-proof and clip_uuid=product-insert\n+ cut_type: insert\n+ intent: visual_evidence_cover\n+ confidence: 0.83\n+ reason: product insert covers the explanation without implying a new scene\n*** Set Output Format\n+ aspect_ratio: 9:16\n+ platform: youtube_shorts\n+ safe_area: mobile\n*** Set Package Metadata\n+ platform: youtube_shorts\n+ title: Edit the pause before the answer\n+ description: Short-form rough assembly cleanup with a J-cut and product insert.\n+ tags: editing,j-cut,shorts\n*** End EDL\n",
                "expect": {
                    "clip_count": 4,
                    "clip_ranges": [{"uuid": "hook-rough", "start_s": 3.0, "duration_s": 5.0}],
                    "cut_boundaries": [
                        {
                            "from": "hook-rough",
                            "to": "guest-proof",
                            "cut_type": "j_cut",
                            "intent": "short_form_speaker_handoff",
                            "audio_relation": "audio_leads",
                            "reason_contains": "guest proof"
                        },
                        {
                            "from": "guest-proof",
                            "to": "product-insert",
                            "cut_type": "insert",
                            "intent": "visual_evidence_cover",
                            "reason_contains": "product insert"
                        }
                    ],
                    "output_format": {
                        "aspect_ratio": "9:16",
                        "platform": "youtube_shorts",
                        "safe_area": "mobile"
                    },
                    "package_metadata": {
                        "platform": "youtube_shorts",
                        "title": "Edit the pause before the answer",
                        "description_contains": "Short-form rough assembly cleanup",
                        "tags_contains": ["editing", "j-cut", "shorts"]
                    },
                    "applied_contains": [
                        "trimmed",
                        "deleted",
                        "set audio lead",
                        "set cut intent",
                        "set output format",
                        "set package metadata"
                    ],
                    "forbidden_ops": ["Insert Transition", "Insert BRoll", "Move Clip"]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let outcome = run_rough_assembly_fixture_at(
            dir.path(),
            None,
            "real::rough_assembly_fixture",
            Instant::now(),
            0,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("applied op"));
    }

    #[test]
    fn checked_in_rough_assembly_sample_fixture_validates() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-rough-assembly-sample",
            &[
                ClipSpec::video("hook-rough", "hook-rough", "raw/hook.mp4", 0.0, 8.0),
                ClipSpec::video("host-reset", "host-reset", "raw/host-reset.mp4", 32.0, 4.0),
                ClipSpec::video(
                    "guest-proof",
                    "guest-proof",
                    "raw/guest-proof.mp4",
                    55.0,
                    9.0,
                ),
                ClipSpec::video(
                    "product-insert",
                    "product-insert",
                    "raw/product-closeup.mp4",
                    4.0,
                    3.0,
                ),
                ClipSpec::video("cta", "cta", "raw/cta.mp4", 80.0, 5.0),
            ],
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("real")
            .join("rough-assembly-flow.sample.json");
        let fixture = load_rough_assembly_fixture(&path).unwrap();

        let applied = validate_rough_assembly_fixture(dir.path(), &fixture).unwrap();

        assert!(applied >= 6);
    }

    #[test]
    fn checked_in_rough_assembly_directory_samples_validate() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-rough-assembly-directory-samples",
            &[
                ClipSpec::video("hook-rough", "hook-rough", "raw/hook.mp4", 0.0, 8.0),
                ClipSpec::video("host-reset", "host-reset", "raw/host-reset.mp4", 32.0, 4.0),
                ClipSpec::video(
                    "guest-proof",
                    "guest-proof",
                    "raw/guest-proof.mp4",
                    55.0,
                    9.0,
                ),
                ClipSpec::video(
                    "product-insert",
                    "product-insert",
                    "raw/product-closeup.mp4",
                    4.0,
                    3.0,
                ),
                ClipSpec::video("cta", "cta", "raw/cta.mp4", 80.0, 5.0),
            ],
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("real")
            .join("rough-assemblies");

        let outcome = run_rough_assembly_fixture_at(
            dir.path(),
            Some(&path),
            "real::rough_assembly_fixture",
            Instant::now(),
            1,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("fixture"));
    }

    #[test]
    fn rough_assembly_fixture_directory_runs_all_json_fixtures() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-rough-assembly-directory",
            &[
                ClipSpec::video("hook-rough", "hook-rough", "raw/hook.mp4", 0.0, 8.0),
                ClipSpec::video("host-reset", "host-reset", "raw/host-reset.mp4", 32.0, 4.0),
                ClipSpec::video(
                    "guest-proof",
                    "guest-proof",
                    "raw/guest-proof.mp4",
                    55.0,
                    9.0,
                ),
                ClipSpec::video(
                    "product-insert",
                    "product-insert",
                    "raw/product-closeup.mp4",
                    4.0,
                    3.0,
                ),
                ClipSpec::video("cta", "cta", "raw/cta.mp4", 80.0, 5.0),
            ],
        )
        .unwrap();
        let fixture_dir = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("rough-assemblies");
        std::fs::create_dir_all(&fixture_dir).unwrap();
        std::fs::write(
            fixture_dir.join("01-shorts-cleanup.json"),
            include_str!("../fixtures/real/rough-assembly-flow.sample.json"),
        )
        .unwrap();
        std::fs::write(
            fixture_dir.join("02-shorts-cleanup.json"),
            include_str!("../fixtures/real/rough-assembly-flow.sample.json"),
        )
        .unwrap();

        let outcome = run_rough_assembly_fixture_at(
            dir.path(),
            None,
            "real::rough_assembly_fixture",
            Instant::now(),
            0,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Pass);
        assert!(outcome.message.contains("2 fixture"));
        assert!(outcome.message.contains("applied op"));
    }

    #[test]
    fn rough_assembly_fixture_minimum_fails_when_corpus_is_undercovered() {
        let dir = tempfile::tempdir().unwrap();
        write_project_with_clips(
            dir.path(),
            "real-rough-assembly-minimum",
            &[
                ClipSpec::video("hook-rough", "hook-rough", "raw/hook.mp4", 0.0, 8.0),
                ClipSpec::video("host-reset", "host-reset", "raw/host-reset.mp4", 32.0, 4.0),
                ClipSpec::video(
                    "guest-proof",
                    "guest-proof",
                    "raw/guest-proof.mp4",
                    55.0,
                    9.0,
                ),
                ClipSpec::video(
                    "product-insert",
                    "product-insert",
                    "raw/product-closeup.mp4",
                    4.0,
                    3.0,
                ),
                ClipSpec::video("cta", "cta", "raw/cta.mp4", 80.0, 5.0),
            ],
        )
        .unwrap();
        let fixture_path = dir
            .path()
            .join(".awidat")
            .join("eval")
            .join("rough-assembly-flow.json");
        std::fs::create_dir_all(fixture_path.parent().unwrap()).unwrap();
        std::fs::write(
            &fixture_path,
            include_str!("../fixtures/real/rough-assembly-flow.sample.json"),
        )
        .unwrap();

        let outcome = run_rough_assembly_fixture_at(
            dir.path(),
            None,
            "real::rough_assembly_fixture",
            Instant::now(),
            2,
        )
        .unwrap();

        assert_eq!(outcome.status, ScenarioStatus::Fail);
        assert!(outcome.message.contains("expected at least 2"));
        assert!(outcome.message.contains("found 1"));
    }

    #[test]
    fn rough_assembly_proposal_history_rejects_empty_final_edl_assertions() {
        let entry: ExpectedRoughAssemblyProposalHistory =
            serde_json::from_value(serde_json::json!({
                "id": "empty-final-negative-proof",
                "status": "rejected",
                "recommendation": "Insert Transition",
                "edl_contains": ["*** Insert Transition"],
                "final_edl_must_not_contain": [""]
            }))
            .unwrap();

        let err = expect_anyhow_err(
            validate_rough_assembly_proposal_history_entry(
                &entry,
                "*** Begin EDL\n*** Set Cut Intent\n@@ between: clip_uuid=hook and clip_uuid=guest\n+ cut_type: j_cut\n*** End EDL\n",
            ),
            "empty final_edl assertion snippets should not validate",
        );

        assert!(
            err.to_string().contains("empty final_edl_must_not_contain"),
            "{err:#}"
        );
    }

    #[test]
    fn visual_context_coverage_counts_metadata_and_candidates() {
        let sidecars = vec![(
            "raw/a.mp4".to_string(),
            serde_json::json!({
                "data": {
                    "shots": [
                        {
                            "start_s": 0.0,
                            "end_s": 2.0,
                            "subject_zone": "left_third",
                            "match_candidates": [
                                {"asset_id": "raw/b.mp4", "time_s": 8.0, "similarity": 0.84}
                            ]
                        },
                        {
                            "start_s": 2.0,
                            "end_s": 4.0,
                            "subject_zone": null,
                            "match_candidates": []
                        },
                        {
                            "start_s": 4.0,
                            "end_s": 6.0,
                            "match_cut_score": 0.81
                        },
                        {
                            "start_s": 6.0,
                            "end_s": 8.0,
                            "composition_source": "model:composition-v1",
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        }
                    ]
                }
            }),
        )];

        let coverage = visual_context_coverage(sidecars);

        assert_eq!(coverage.total_shots, 4);
        assert_eq!(coverage.metadata_shots, 3);
        assert_eq!(coverage.match_candidate_shots, 1);
        assert_eq!(coverage.model_composition_shots, 1);
        assert_eq!(coverage.probe, Some(("raw/a.mp4".to_string(), 1.0)));
        assert!(coverage.meets(VisualContextThresholds {
            min_metadata_shots: 2,
            min_metadata_ratio: 0.5,
            min_match_candidate_shots: 1,
            min_model_composition_shots: 1,
            min_composition_model_regions: 0,
            max_invalid_composition_model_regions: 0,
        }));
        assert!(!coverage.meets(VisualContextThresholds {
            min_metadata_shots: 3,
            min_metadata_ratio: 0.8,
            min_match_candidate_shots: 2,
            min_model_composition_shots: 2,
            min_composition_model_regions: 0,
            max_invalid_composition_model_regions: 0,
        }));
    }

    #[test]
    fn visual_context_coverage_requires_real_composition_model_regions() {
        let sidecars = vec![(
            "raw/a.mp4".to_string(),
            serde_json::json!({
                "data": {
                    "shots": [
                        {
                            "start_s": 0.0,
                            "end_s": 2.0,
                            "composition_source": "model:composition-v1",
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        }
                    ]
                }
            }),
        )];

        let mut coverage = visual_context_coverage(sidecars);
        coverage.composition_model_regions = 0;

        assert!(!coverage.meets(VisualContextThresholds {
            min_metadata_shots: 1,
            min_metadata_ratio: 1.0,
            min_match_candidate_shots: 0,
            min_model_composition_shots: 1,
            min_composition_model_regions: 1,
            max_invalid_composition_model_regions: 0,
        }));

        coverage.composition_model_regions = 1;

        assert!(coverage.meets(VisualContextThresholds {
            min_metadata_shots: 1,
            min_metadata_ratio: 1.0,
            min_match_candidate_shots: 0,
            min_model_composition_shots: 1,
            min_composition_model_regions: 1,
            max_invalid_composition_model_regions: 0,
        }));
    }

    #[test]
    fn visual_context_coverage_rejects_too_many_invalid_composition_model_regions() {
        let coverage = VisualContextCoverage {
            total_shots: 4,
            metadata_shots: 4,
            match_candidate_shots: 1,
            model_composition_shots: 2,
            composition_model_regions: 4,
            invalid_composition_model_regions: 1,
            probe: Some(("raw/a.mp4".to_string(), 1.0)),
        };

        assert!(!coverage.meets(VisualContextThresholds {
            min_metadata_shots: 3,
            min_metadata_ratio: 0.25,
            min_match_candidate_shots: 1,
            min_model_composition_shots: 2,
            min_composition_model_regions: 4,
            max_invalid_composition_model_regions: 0,
        }));
        assert!(coverage.meets(VisualContextThresholds {
            min_metadata_shots: 3,
            min_metadata_ratio: 0.25,
            min_match_candidate_shots: 1,
            min_model_composition_shots: 2,
            min_composition_model_regions: 4,
            max_invalid_composition_model_regions: 1,
        }));
    }

    #[test]
    fn live_threshold_integer_parser_rejects_invalid_values() {
        let err = expect_anyhow_err(
            parse_env_usize(
                "AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS",
                Some("many"),
                0,
            ),
            "invalid threshold integers must fail instead of defaulting",
        );

        assert!(
            err.to_string()
                .contains("AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS"),
            "{err:#}"
        );
    }

    #[test]
    fn live_threshold_ratio_parser_rejects_invalid_values() {
        let err = expect_anyhow_err(
            parse_env_ratio(
                "AWIDAT_REAL_VISUAL_MIN_METADATA_RATIO",
                Some("not-a-ratio"),
                0.25,
            ),
            "invalid threshold ratios must fail instead of defaulting",
        );

        assert!(
            err.to_string()
                .contains("AWIDAT_REAL_VISUAL_MIN_METADATA_RATIO"),
            "{err:#}"
        );
    }

    #[test]
    fn live_threshold_ratio_parser_rejects_out_of_range_values() {
        let err = expect_anyhow_err(
            parse_env_ratio("AWIDAT_REAL_VISUAL_MIN_METADATA_RATIO", Some("1.25"), 0.25),
            "out-of-range threshold ratios must fail instead of clamping",
        );

        assert!(err.to_string().contains("in [0, 1]"), "{err:#}");
    }

    #[test]
    fn composition_model_region_count_reads_real_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("index")
            .join("composition-model")
            .join("raw")
            .join("a.mp4.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "composition-model",
                "data": {
                    "regions": [
                        {
                            "start_s": 0.0,
                            "end_s": 1.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.91,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        },
                        {
                            "start_s": 1.0,
                            "end_s": 2.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.87,
                            "subject_role": "secondary_subject",
                            "depth_layer": "midground",
                            "framing": "two_shot"
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(composition_model_region_counts(dir.path()).valid, 2);
    }

    #[test]
    fn composition_model_region_count_ignores_invalid_regions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("index")
            .join("composition-model")
            .join("raw")
            .join("a.mp4.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "composition-model",
                "data": {
                    "regions": [
                        {
                            "start_s": 0.0,
                            "end_s": 1.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.91,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        },
                        {
                            "start_s": 1.0,
                            "end_s": 2.0,
                            "composition_source": "heuristic:composition-v1",
                            "composition_confidence": 0.91,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        },
                        {
                            "start_s": 2.0,
                            "end_s": 1.5,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.91,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        },
                        {
                            "start_s": 2.0,
                            "end_s": 3.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 1.5,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        },
                        {
                            "start_s": 3.0,
                            "end_s": 4.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.91,
                            "subject_role": "banana",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(composition_model_region_counts(dir.path()).valid, 1);
    }

    #[test]
    fn composition_model_region_counts_report_invalid_regions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("index")
            .join("composition-model")
            .join("raw")
            .join("a.mp4.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "indexer": "composition-model",
                "data": {
                    "regions": [
                        {
                            "start_s": 0.0,
                            "end_s": 1.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.91,
                            "subject_role": "primary_speaker",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        },
                        {
                            "start_s": 1.0,
                            "end_s": 2.0,
                            "composition_source": "model:composition-v2",
                            "composition_confidence": 0.91,
                            "subject_role": "unknown_subject",
                            "depth_layer": "foreground",
                            "framing": "single_close"
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let counts = composition_model_region_counts(dir.path());

        assert_eq!(counts.valid, 1);
        assert_eq!(counts.invalid, 1);

        let coverage = VisualContextCoverage {
            composition_model_regions: counts.valid,
            invalid_composition_model_regions: counts.invalid,
            ..Default::default()
        };
        assert!(
            coverage
                .summary()
                .contains("1 invalid composition-model regions")
        );
    }
}
