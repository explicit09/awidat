//! `validate_transition_choice` agent tool.
//!
//! After applying a motion-sensitive transition (any `whip_pan_*`,
//! `pass_by_*`, `motion_blur`, `slide_*`, `zoom_in`, …), the agent
//! can ask this tool whether the choice actually matched the source
//! footage's measured motion. The tool reads the shot sidecars at the
//! two source-time boundaries — the same data
//! [`crate::visual_signals`] already exposes to `transition_context`
//! — and compares the transition's declared `motion_alignment`
//! against the measured `dominant_direction` from each side.
//!
//! The tool is read-only and intentionally cheap (no FFmpeg
//! subprocess, no optical flow over the rendered file). When the
//! editor later grows a real post-render flow-based check, this
//! function becomes a fallback for the index-only verdict.

use async_trait::async_trait;
use awidat_proto::transitions::{MotionAlignment, lookup_builtin_transition};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::visual_signals::{MotionMatch, load_boundary_signals};

/// Post-application motion validator.
pub struct ValidateTransitionChoiceTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Stable transition id (e.g. `awidat.whip_pan_left`).
    transition_id: String,
    /// Outgoing clip's source asset id (matches the index sidecar key).
    outgoing_asset_id: String,
    /// Source-time at the boundary on the outgoing side.
    outgoing_source_end_s: f64,
    /// Incoming clip's source asset id.
    incoming_asset_id: String,
    /// Source-time at the boundary on the incoming side.
    incoming_source_start_s: f64,
}

#[async_trait]
impl ToolHandler for ValidateTransitionChoiceTool {
    fn name(&self) -> &'static str {
        "validate_transition_choice"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "validate_transition_choice".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "transition_id": { "type": "string" },
                    "outgoing_asset_id": { "type": "string" },
                    "outgoing_source_end_s": { "type": "number" },
                    "incoming_asset_id": { "type": "string" },
                    "incoming_source_start_s": { "type": "number" }
                },
                "required": [
                    "transition_id",
                    "outgoing_asset_id",
                    "outgoing_source_end_s",
                    "incoming_asset_id",
                    "incoming_source_start_s"
                ]
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
                "validate_transition_choice: invalid args ({e})."
            ))
        })?;

        let Some(transition) = lookup_builtin_transition(&args.transition_id) else {
            return Err(FunctionCallError::RespondToModel(format!(
                "validate_transition_choice: unknown transition id {:?}",
                args.transition_id
            )));
        };

        let signals = load_boundary_signals(
            &ctx.project_root,
            &args.outgoing_asset_id,
            args.outgoing_source_end_s,
            &args.incoming_asset_id,
            args.incoming_source_start_s,
        );

        let predicted = transition.motion_alignment;
        let outgoing = signals.outgoing.motion_direction;
        let incoming = signals.incoming.motion_direction;
        let motion_match = match predicted {
            None => true, // not a motion-sensitive transition
            Some(p) => {
                let outgoing_ok = outgoing.is_some_and(|d| d.agrees_with(p));
                let incoming_ok = incoming.is_some_and(|d| d.agrees_with(p));
                outgoing_ok && incoming_ok
            }
        };
        let motion_confidence = signals.motion_match_confidence();
        let verdict = editorial_verdict(transition, predicted, outgoing, incoming, &signals);

        let body = serde_json::json!({
            "transition_id": transition.id,
            "predicted_direction": predicted.map(direction_str),
            "actual_direction_outgoing": outgoing.map(direction_str),
            "actual_direction_incoming": incoming.map(direction_str),
            "motion_match": motion_match,
            "motion_confidence": motion_confidence.map(round3),
            "motion_match_class": signals.motion_match().as_str(),
            "editorial_verdict": verdict.kind,
            "reason": verdict.reason,
            "requires_motion_continuity": transition.requires_motion_continuity,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

struct Verdict {
    kind: &'static str,
    reason: String,
}

fn editorial_verdict(
    transition: &awidat_proto::transitions::BuiltinTransition,
    predicted: Option<MotionAlignment>,
    outgoing: Option<MotionAlignment>,
    incoming: Option<MotionAlignment>,
    signals: &crate::visual_signals::BoundaryVisualSignals,
) -> Verdict {
    if !transition.requires_motion_continuity && predicted.is_none() {
        return Verdict {
            kind: "acceptable",
            reason: format!(
                "{} is not motion-sensitive; nothing to validate against measured flow.",
                transition.display_name
            ),
        };
    }
    let Some(predicted) = predicted else {
        return Verdict {
            kind: "no_signal",
            reason: format!(
                "{} declares no motion direction even though it requires motion continuity; \
                 unable to validate.",
                transition.display_name
            ),
        };
    };
    if outgoing.is_none() && incoming.is_none() {
        return Verdict {
            kind: "no_signal",
            reason: "neither side of the cut has a measured motion direction in the shot \
                     sidecars; cannot validate"
                .into(),
        };
    }
    let mismatched_sides = [outgoing, incoming]
        .into_iter()
        .filter(|d| d.is_some_and(|d| !d.agrees_with(predicted)))
        .count();
    if mismatched_sides == 0 {
        return Verdict {
            kind: "acceptable",
            reason: format!(
                "predicted {pred} matches measured motion on both sides; transition reads correctly.",
                pred = direction_str(predicted)
            ),
        };
    }
    if signals.motion_match() == MotionMatch::Opposed {
        return Verdict {
            kind: "wrong_direction",
            reason: format!(
                "outgoing and incoming motion are opposed; {} would cut against the action.",
                transition.display_name
            ),
        };
    }
    Verdict {
        kind: "wrong_direction",
        reason: format!(
            "predicted {pred} does not match measured motion (outgoing={out:?}, incoming={inc:?}); \
             {name} reads against the footage motion.",
            pred = direction_str(predicted),
            out = outgoing.map(direction_str),
            inc = incoming.map(direction_str),
            name = transition.display_name
        ),
    }
}

fn direction_str(d: MotionAlignment) -> &'static str {
    d.as_str()
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

const DESCRIPTION: &str = "\
After applying a motion-sensitive transition (whip_pan_*, pass_by_*, \
motion_blur, slide_*, zoom_in, etc.), call this to verify the chosen \
direction matches the source clips' measured motion. Returns the \
transition's predicted direction, the measured directions from each \
side's shot sidecar, a boolean motion_match, and an editorial verdict \
('acceptable' / 'wrong_direction' / 'no_signal'). This is the \
closed-loop check that lets future plan_transition calls weight \
direction predictions more carefully.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::SandboxMode;
    use std::path::Path;

    fn ctx_at(project_root: &Path) -> ToolContext {
        let (tx, _) = tokio::sync::broadcast::channel(8);
        ToolContext {
            project_root: project_root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
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
            name: "validate_transition_choice".into(),
            args,
        }
    }

    fn write_shot_sidecar(project_root: &Path, asset_id: &str, shots: serde_json::Value) {
        let path = project_root
            .join("index")
            .join("shot")
            .join(format!("{asset_id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({ "data": { "shots": shots } });
        std::fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
    }

    #[test]
    fn validator_is_read_only() {
        assert!(
            !ValidateTransitionChoiceTool.is_mutating(&invoke(serde_json::json!({
                "transition_id": "awidat.whip_pan_left",
                "outgoing_asset_id": "raw_a",
                "outgoing_source_end_s": 1.0,
                "incoming_asset_id": "raw_b",
                "incoming_source_start_s": 0.0,
            })))
        );
    }

    #[tokio::test]
    async fn whip_pan_left_against_rightward_motion_reports_wrong_direction() {
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.7, "dominant_direction": "right"}
            ]),
        );
        write_shot_sidecar(
            dir.path(),
            "raw_b",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.6, "dominant_direction": "right"}
            ]),
        );
        let out = ValidateTransitionChoiceTool
            .handle(
                invoke(serde_json::json!({
                    "transition_id": "awidat.whip_pan_left",
                    "outgoing_asset_id": "raw_a",
                    "outgoing_source_end_s": 2.0,
                    "incoming_asset_id": "raw_b",
                    "incoming_source_start_s": 0.5,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/motion_match").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            body.pointer("/editorial_verdict").and_then(|v| v.as_str()),
            Some("wrong_direction")
        );
        assert_eq!(
            body.pointer("/predicted_direction")
                .and_then(|v| v.as_str()),
            Some("left")
        );
    }

    #[tokio::test]
    async fn whip_pan_left_against_leftward_motion_reports_acceptable() {
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.7, "dominant_direction": "left"}
            ]),
        );
        write_shot_sidecar(
            dir.path(),
            "raw_b",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.6, "dominant_direction": "left"}
            ]),
        );
        let out = ValidateTransitionChoiceTool
            .handle(
                invoke(serde_json::json!({
                    "transition_id": "awidat.whip_pan_left",
                    "outgoing_asset_id": "raw_a",
                    "outgoing_source_end_s": 2.0,
                    "incoming_asset_id": "raw_b",
                    "incoming_source_start_s": 0.5,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/motion_match").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            body.pointer("/editorial_verdict").and_then(|v| v.as_str()),
            Some("acceptable")
        );
    }

    #[tokio::test]
    async fn missing_sidecars_report_no_signal() {
        let dir = tempfile::tempdir().unwrap();
        let out = ValidateTransitionChoiceTool
            .handle(
                invoke(serde_json::json!({
                    "transition_id": "awidat.whip_pan_left",
                    "outgoing_asset_id": "raw_unknown_a",
                    "outgoing_source_end_s": 1.0,
                    "incoming_asset_id": "raw_unknown_b",
                    "incoming_source_start_s": 0.0,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/editorial_verdict").and_then(|v| v.as_str()),
            Some("no_signal")
        );
        assert_eq!(
            body.pointer("/motion_match").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn non_motion_sensitive_transition_is_always_acceptable() {
        let dir = tempfile::tempdir().unwrap();
        let out = ValidateTransitionChoiceTool
            .handle(
                invoke(serde_json::json!({
                    "transition_id": "awidat.cross_dissolve",
                    "outgoing_asset_id": "raw_anything",
                    "outgoing_source_end_s": 0.0,
                    "incoming_asset_id": "raw_anything",
                    "incoming_source_start_s": 0.0,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(
            body.pointer("/editorial_verdict").and_then(|v| v.as_str()),
            Some("acceptable")
        );
    }

    #[tokio::test]
    async fn unknown_transition_id_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = ValidateTransitionChoiceTool
            .handle(
                invoke(serde_json::json!({
                    "transition_id": "awidat.not_real",
                    "outgoing_asset_id": "raw_a",
                    "outgoing_source_end_s": 0.0,
                    "incoming_asset_id": "raw_b",
                    "incoming_source_start_s": 0.0,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        let FunctionCallError::RespondToModel(msg) = err else {
            panic!("expected RespondToModel error");
        };
        assert!(msg.contains("unknown transition"));
    }

    /// Phase 5 perf budget: the validator must complete in <2 seconds
    /// for a typical 0.2s transition window.
    #[tokio::test]
    async fn validator_runs_within_perf_budget() {
        use std::time::Instant;
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.5, "dominant_direction": "left"}
            ]),
        );
        write_shot_sidecar(
            dir.path(),
            "raw_b",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.5, "dominant_direction": "left"}
            ]),
        );
        let start = Instant::now();
        let _ = ValidateTransitionChoiceTool
            .handle(
                invoke(serde_json::json!({
                    "transition_id": "awidat.whip_pan_left",
                    "outgoing_asset_id": "raw_a",
                    "outgoing_source_end_s": 1.0,
                    "incoming_asset_id": "raw_b",
                    "incoming_source_start_s": 0.0,
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 2000,
            "validator took {}ms, exceeds 2s budget",
            elapsed.as_millis()
        );
    }
}
