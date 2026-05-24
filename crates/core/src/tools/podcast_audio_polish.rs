//! `podcast_audio_polish` tool — forced audio finishing pass for podcasts.

use async_trait::async_trait;
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Build a podcast audio polish report from existing timeline metadata.
pub struct PodcastAudioPolishTool;

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    target_lufs: Option<f64>,
}

#[async_trait]
impl ToolHandler for PodcastAudioPolishTool {
    fn name(&self) -> &'static str {
        "podcast_audio_polish"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: "Check podcast audio finishing readiness: loudness, clipping, noise, buses, and recommended mix processors.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "target_lufs": {
                        "type": "number",
                        "description": "Integrated loudness target. Default -16 LUFS for stereo podcasts."
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
                "podcast_audio_polish: invalid args ({e}). All fields are optional."
            ))
        })?;
        let target_lufs = args.target_lufs.unwrap_or(-16.0);
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_audio_polish: failed to read project: {e}"
            ))
        })?;
        let finishing = crate::professional::derive_audio_finishing_state(&project.timeline);
        let mut issues = Vec::new();
        let mut recommendations = Vec::new();

        if finishing.buses.is_empty() {
            issues.push(serde_json::json!({
                "kind": "missing_audio_buses",
                "severity": "warning",
                "message": "No audio finishing buses were derived from the timeline."
            }));
        }
        if !finishing
            .chains
            .iter()
            .any(|chain| chain.id == "dialogue_cleanup")
        {
            recommendations
                .push("Add dialogue cleanup chain: noise reduction, de-esser, EQ, compression.");
        }
        if !finishing
            .chains
            .iter()
            .any(|chain| chain.id == "master_delivery")
        {
            recommendations.push("Add master delivery chain: limiter and loudness meter.");
        }

        if finishing.meters.is_empty() {
            issues.push(serde_json::json!({
                "kind": "missing_audio_measurements",
                "severity": "warning",
                "message": "No audio_measurements metadata found; loudness, true peak, noise, and clipping need a meter pass."
            }));
        }
        for meter in &finishing.meters {
            if meter.clipping {
                issues.push(serde_json::json!({
                    "kind": "clipping",
                    "severity": "error",
                    "target": meter.target,
                    "message": "Clipping detected; reduce gain or repair clipped dialogue before render."
                }));
            }
            if let Some(lufs) = meter.integrated_lufs
                && meter.target == "master"
                && (lufs - target_lufs).abs() > 2.0
            {
                issues.push(serde_json::json!({
                    "kind": "loudness_out_of_range",
                    "severity": "warning",
                    "target": meter.target,
                    "integrated_lufs": lufs,
                    "target_lufs": target_lufs,
                    "message": "Master loudness is outside the podcast target window."
                }));
            }
            if let Some(true_peak) = meter.true_peak_db
                && true_peak > -1.0
            {
                issues.push(serde_json::json!({
                    "kind": "true_peak_too_hot",
                    "severity": "warning",
                    "target": meter.target,
                    "true_peak_db": true_peak,
                    "message": "True peak should stay at or below -1 dBTP for delivery safety."
                }));
            }
            if let Some(noise_floor) = meter.noise_floor_db
                && noise_floor > -45.0
            {
                issues.push(serde_json::json!({
                    "kind": "high_noise_floor",
                    "severity": "warning",
                    "target": meter.target,
                    "noise_floor_db": noise_floor,
                    "message": "Noise floor is high enough to need noise reduction or room-tone work."
                }));
            }
        }

        recommendations.extend([
            "Balance speaker dialogue levels before final render.",
            "Use light compression and de-essing on dialogue; avoid over-processing.",
            "Set final loudness target via Set Loudness Target before render.",
        ]);
        let status = if issues.iter().any(|issue| issue["severity"] == "error") {
            "needs_fix"
        } else if issues.is_empty() {
            "ready"
        } else {
            "needs_review"
        };
        let body = serde_json::json!({
            "status": status,
            "summary_for_agent": format!("Audio polish status: {status}. {} issue(s), {} recommendation(s).", issues.len(), recommendations.len()),
            "target_lufs": target_lufs,
            "audio_finishing": finishing,
            "issues": issues,
            "recommendations": recommendations,
            "required_before_render": true,
        });
        serde_json::to_string(&body)
            .map(ToolOutput::text)
            .map_err(|e| FunctionCallError::Fatal(format!("podcast_audio_polish serialize: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::AwidatTimelineMetadata;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
        Timeline, Track, TrackChild, TrackKind,
    };
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_project(root: &std::path::Path) {
        let mut project = Project::init(root).unwrap();
        let mut clip = Clip::empty("clip-1");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/audio.wav"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(48_000.0),
            RationalTime::new(10.0 * 48_000.0, 48_000.0),
        ));
        let mut track = Track::empty("A1 Dialogue", TrackKind::Audio);
        track.children.push(TrackChild::Clip(clip));
        let mut timeline = Timeline::empty("podcast");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        timeline.metadata.awidat = Some(AwidatTimelineMetadata::default());
        timeline.metadata.awidat.as_mut().unwrap().extra.insert(
            "audio_measurements".into(),
            serde_json::json!({
                "master": {
                    "integrated_lufs": -22.5,
                    "true_peak_db": -0.2,
                    "noise_floor_db": -40.0,
                    "clipping": true
                }
            }),
        );
        project.timeline = timeline;
        project.write(root).unwrap();
    }

    #[tokio::test]
    async fn reports_loudness_and_clipping_issues() {
        let dir = tempfile::tempdir().unwrap();
        write_project(dir.path());
        let out = PodcastAudioPolishTool
            .handle(
                ToolInvocation {
                    call_id: "a1".into(),
                    name: "podcast_audio_polish".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "needs_fix");
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "clipping")
        );
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "loudness_out_of_range")
        );
    }
}
