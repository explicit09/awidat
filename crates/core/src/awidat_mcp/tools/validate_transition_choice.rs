//! `validate_transition_choice` — post-application motion validator.
//! Ported from `crates/core/src/tools/validate_transition_choice.rs`
//! to the in-process MCP server. Compares a transition's declared
//! motion alignment against measured `dominant_direction` from each
//! side's shot sidecar.

use awidat_proto::transitions::{MotionAlignment, lookup_builtin_transition};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;
use crate::visual_signals::{MotionMatch, load_boundary_signals};

/// Arguments to `validate_transition_choice`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ValidateTransitionChoiceArgs {
    /// Stable transition id (e.g. `awidat.whip_pan_left`).
    pub transition_id: String,
    /// Outgoing clip's source asset id (matches the index sidecar key).
    pub outgoing_asset_id: String,
    /// Source-time at the boundary on the outgoing side.
    pub outgoing_source_end_s: f64,
    /// Incoming clip's source asset id.
    pub incoming_asset_id: String,
    /// Source-time at the boundary on the incoming side.
    pub incoming_source_start_s: f64,
}

/// Run `validate_transition_choice` against the project resolved
/// from [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// unknown transition ids return `Err(String)`.
pub fn run(args: ValidateTransitionChoiceArgs, ctx: McpToolCtx) -> Result<String, String> {
    let Some(transition) = lookup_builtin_transition(&args.transition_id) else {
        return Err(format!(
            "validate_transition_choice: unknown transition id {:?}",
            args.transition_id
        ));
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
    Ok(body.to_string())
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

pub const DESCRIPTION: &str = "\
After applying a motion-sensitive transition (whip_pan_*, pass_by_*, \
motion_blur, slide_*, zoom_in, etc.), call this to verify the chosen \
direction matches the source clips' measured motion. Returns the \
transition's predicted direction, the measured directions from each \
side's shot sidecar, a boolean motion_match, and an editorial verdict \
('acceptable' / 'wrong_direction' / 'no_signal'). This is the \
closed-loop check that lets future plan_transition calls weight \
direction predictions more carefully.\
";
