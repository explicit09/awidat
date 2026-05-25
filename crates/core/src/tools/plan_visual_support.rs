//! Read-only planner for routing visual-support requests to the right lane.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Visual-support lane the agent should use first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualSupportLane {
    /// Structural timeline edits such as trimming, cutting, or rearranging.
    TimelineEdit,
    /// Footage-like support: project footage, screenshots, stock, web, or generated b-roll.
    Broll,
    /// Freeform designed graphics: explainers, diagrams, kinetic text, charts.
    MotionScene,
    /// Simple titles, lower thirds, captions, or annotations.
    TitleAnnotation,
    /// Direct source-pixel/audio polish using existing render primitives.
    EffectsFinishing,
}

/// Routing result returned to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VisualSupportRoute {
    /// Recommended primary lane.
    pub primary_lane: VisualSupportLane,
    /// Additional lanes that may be needed for hybrid requests.
    pub supporting_lanes: Vec<VisualSupportLane>,
    /// Tools the agent should generally consider next.
    pub next_tools: Vec<String>,
    /// Short explanation for review and self-correction.
    pub rationale: String,
}

/// Read-only tool wrapper.
pub struct PlanVisualSupportTool;

#[derive(Debug, Deserialize)]
struct PlanVisualSupportArgs {
    request: String,
}

/// Route a natural-language visual-support request to the smallest useful lane.
pub fn route_visual_support_request(request: &str) -> VisualSupportRoute {
    let lower = request.to_lowercase();
    let supporting_lanes = supporting_lanes_for_request(&lower);

    if contains_any(
        &lower,
        &[
            "trim",
            "cut out",
            "remove pause",
            "remove pauses",
            "tighten",
            "reorder",
            "split",
            "delete",
        ],
    ) {
        return route(
            VisualSupportLane::TimelineEdit,
            supporting_lanes,
            &["find_dead_air", "find_filler_words", "apply_edl"],
            "The request changes edit structure rather than adding visual support.",
        );
    }

    if contains_any(
        &lower,
        &[
            "blur",
            "warmer",
            "color",
            "lut",
            "crop",
            "reframe",
            "stabilize",
            "volume",
            "loudness",
            "transition",
            "fade",
            "speed",
        ],
    ) {
        return route(
            VisualSupportLane::EffectsFinishing,
            supporting_lanes,
            &["inspect_clip", "plan_emphasis", "apply_edl"],
            "The request modifies existing footage or audio with renderable effects.",
        );
    }

    if contains_any(
        &lower,
        &[
            "guest name",
            "name and title",
            "lower third",
            "caption",
            "subtitle",
            "label this",
            "arrow",
            "circle",
            "rectangle",
            "annotation",
        ],
    ) && !contains_any(&lower, &["animated", "animate", "diagram", "explainer"])
    {
        return route(
            VisualSupportLane::TitleAnnotation,
            supporting_lanes,
            &["view_timeline", "apply_edl"],
            "A single title, caption, or annotation is enough; avoid a freeform scene.",
        );
    }

    if contains_any(
        &lower,
        &[
            "visualize",
            "explainer",
            "diagram",
            "animated",
            "animate",
            "kinetic",
            "motion graphic",
            "framework",
            "step-by-step",
            "three-step",
            "chart",
            "timeline graphic",
            "process",
            "callout",
            "callouts",
        ],
    ) || wants_still_graphic_motion_scene(&lower)
    {
        return route(
            VisualSupportLane::MotionScene,
            supporting_lanes,
            &["view_timeline", "plan_motion_scene", "apply_edl"],
            "The request is best answered with designed, timed, layered graphics.",
        );
    }

    if contains_any(
        &lower,
        &[
            "b-roll",
            "broll",
            "footage",
            "screenshot",
            "show supporting",
            "show the",
            "stock",
            "youtube",
            "generated video",
            "ai-generated video",
            "real-world",
            "product shot",
        ],
    ) {
        return route(
            VisualSupportLane::Broll,
            supporting_lanes,
            &[
                "find_broll_opportunities",
                "search_broll",
                "use_broll",
                "download_yt_clip",
            ],
            "The request asks for footage-like visual evidence or cutaway support.",
        );
    }

    route(
        VisualSupportLane::Broll,
        supporting_lanes,
        &["find_broll_opportunities", "search_broll", "view_timeline"],
        "Default to footage-like support when the visual intent is underspecified.",
    )
}

fn route(
    primary_lane: VisualSupportLane,
    supporting_lanes: Vec<VisualSupportLane>,
    next_tools: &[&str],
    rationale: &str,
) -> VisualSupportRoute {
    let supporting_lanes = supporting_lanes
        .into_iter()
        .filter(|lane| *lane != primary_lane)
        .collect();
    VisualSupportRoute {
        primary_lane,
        supporting_lanes,
        next_tools: next_tools.iter().map(|tool| (*tool).to_string()).collect(),
        rationale: rationale.to_string(),
    }
}

fn supporting_lanes_for_request(lower: &str) -> Vec<VisualSupportLane> {
    let mut lanes = Vec::new();
    if contains_any(
        lower,
        &[
            "b-roll",
            "broll",
            "footage",
            "screenshot",
            "stock",
            "youtube",
            "generated video",
            "ai-generated video",
        ],
    ) {
        lanes.push(VisualSupportLane::Broll);
    }
    if wants_still_graphic_motion_scene(lower) {
        lanes.push(VisualSupportLane::MotionScene);
    }
    if contains_any(
        lower,
        &[
            "caption",
            "captions",
            "subtitle",
            "subtitles",
            "lower third",
            "guest name",
            "name and title",
            "annotation",
        ],
    ) {
        lanes.push(VisualSupportLane::TitleAnnotation);
    }
    if contains_any(
        lower,
        &[
            "animated",
            "animate",
            "callout",
            "callouts",
            "diagram",
            "explainer",
        ],
    ) {
        lanes.push(VisualSupportLane::MotionScene);
    }
    lanes
}

fn wants_still_graphic_motion_scene(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "logo",
            "screenshot",
            "product still",
            "product image",
            "still image",
            "chart",
            "diagram",
            "generated png",
        ],
    ) && contains_any(
        lower,
        &[
            "overlay",
            "card",
            "callout",
            "panel",
            "explainer",
            "animate",
            "animated",
            "motion",
            "show",
            "place",
            "add",
        ],
    )
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[async_trait]
impl ToolHandler for PlanVisualSupportTool {
    fn name(&self) -> &'static str {
        "plan_visual_support"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "request": {
                        "type": "string",
                        "description": "The user's visual-support request or the agent's proposed visual need."
                    }
                },
                "required": ["request"]
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
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: PlanVisualSupportArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_visual_support: invalid args ({e}). Required: request."
            ))
        })?;
        if args.request.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "plan_visual_support: request must be non-empty.".into(),
            ));
        }
        let route = route_visual_support_request(&args.request);
        let body = serde_json::to_string_pretty(&route).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "plan_visual_support: failed to serialize route: {e}"
            ))
        })?;
        Ok(ToolOutput::text(body))
    }
}

const DESCRIPTION: &str = "\
Read-only visual-support router. Given a user request or agent-detected \
visual need, choose the smallest useful lane: timeline edit, b-roll, \
motion scene, title/annotation, or effects/finishing. Use before choosing \
between b-roll, generated video, freeform motion graphics, and direct \
FFmpeg/Rust render primitives.\
";
