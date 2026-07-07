//! Read-only planner for routing visual-support requests to the right lane.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

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

/// Why the agent believes the segment needs visual support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualSupportNeedKind {
    /// Abstract concept, explanation, or metaphor that needs visual clarity.
    AbstractExplanation,
    /// Mention of a product, asset, UI, screenshot, logo, chart, or diagram.
    ProductOrAssetMention,
    /// Real-world entity, place, event, or claim that benefits from evidence.
    FactualReference,
    /// Ordered steps, list, process, system, or framework.
    ListOrProcess,
    /// A quote or moment that needs extra emphasis.
    EmotionalEmphasis,
    /// A visible discontinuity or jump cut that needs cover.
    JumpCutCover,
    /// Topic boundary or chapter transition.
    ChapterTransition,
    /// Sponsor, offer, CTA, or sales moment.
    SponsorCta,
    /// Existing source media needs direct finishing rather than new visuals.
    ExistingFootagePolish,
}

/// Editorial intent behind the visual-support choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisualSupportIntent {
    /// Make an idea easier to understand.
    Explain,
    /// Show concrete visual evidence.
    ShowEvidence,
    /// Condense a topic or transition into a short visual.
    Summarize,
    /// Add light visual texture without changing meaning.
    DecorateLightly,
    /// Cover an edit boundary or visual discontinuity.
    HideEdit,
    /// Make a quote, reaction, or claim land harder.
    EmphasizeQuote,
    /// Open a chapter/topic/segment.
    IntroduceChapter,
    /// Compare states, options, before/after, or alternatives.
    CompareBeforeAfter,
}

/// Detected need with a concise agent-facing explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VisualSupportNeed {
    /// Need category.
    pub kind: VisualSupportNeedKind,
    /// Short reason the agent can show to a reviewer.
    pub rationale: String,
}

/// Ordered step in a hybrid visual support plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VisualSupportPlanStep {
    /// Lane responsible for this step.
    pub lane: VisualSupportLane,
    /// Tool the agent should call next for this step.
    pub tool: String,
    /// Concrete action the agent should take.
    pub action: String,
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
    /// Detected visual-support needs that drove the route.
    pub needs: Vec<VisualSupportNeed>,
    /// Editorial intents the agent should satisfy.
    pub intents: Vec<VisualSupportIntent>,
    /// Ordered hybrid execution plan.
    pub plan_steps: Vec<VisualSupportPlanStep>,
    /// Short explanation for review and self-correction.
    pub rationale: String,
}

impl VisualSupportRoute {
    /// When a specific `plan_motion_scene` template was matched from the
    /// request's vocabulary, rewrite the MotionScene lane's `plan_motion_scene`
    /// plan step (and the rationale) to name it explicitly as
    /// `template=<x>`, so the agent's next call is obvious instead of falling
    /// back to the freeform heuristic planner.
    fn with_template_hint(mut self, template: Option<&'static str>) -> Self {
        let Some(template) = template else {
            return self;
        };
        for step in &mut self.plan_steps {
            if step.lane == VisualSupportLane::MotionScene && step.tool == "plan_motion_scene" {
                step.action = format!(
                    "Call `plan_motion_scene` with `template={template}` and its typed fields \
                     for that template (see plan_motion_scene's schema); this replaces the \
                     freeform heuristic layer builder."
                );
            }
        }
        self.rationale = format!(
            "{} Use plan_motion_scene with template={template}.",
            self.rationale
        );
        self
    }
}

/// Read-only tool wrapper.
pub struct PlanVisualSupportTool;

#[derive(Debug, Deserialize)]
struct PlanVisualSupportArgs {
    request: String,
}

/// Read-only match for `plan_motion_scene`'s `template=<x>` field, drawn from
/// the request's own vocabulary. `None` means the freeform heuristic planner
/// applies instead of a specific prebuilt template.
fn motion_template_hint_for_request(lower: &str) -> Option<&'static str> {
    if contains_any(lower, &["kinetic text", "kinetic-text"]) {
        return Some("kinetic_text");
    }
    if contains_any(lower, &["progress bar", "progress-bar"]) {
        return Some("progress_bar");
    }
    if contains_any(lower, &["highlight box", "highlight-box"]) || lower.contains("highlight the") {
        return Some("highlight_box");
    }
    if contains_any(lower, &["lower third", "lower-third"]) && wants_lower_third_template(lower) {
        return Some("lower_third");
    }
    None
}

/// Whether a "lower third" request implies the animated/template lane rather
/// than a single static title-annotation overlay.
fn wants_lower_third_template(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "animated",
            "animate",
            "template",
            "name and role",
            "name/role",
        ],
    )
}

/// Route a natural-language visual-support request to the smallest useful lane.
pub fn route_visual_support_request(request: &str) -> VisualSupportRoute {
    let lower = request.to_lowercase();
    let needs = visual_needs_for_request(&lower);
    let intents = visual_intents_for_request(&lower, &needs);
    let supporting_lanes = supporting_lanes_for_request(&lower);
    let template_hint = motion_template_hint_for_request(&lower);

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
            needs,
            intents,
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
            needs,
            intents,
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
        && template_hint != Some("lower_third")
    {
        return route(
            VisualSupportLane::TitleAnnotation,
            supporting_lanes,
            &["view_timeline", "apply_edl"],
            needs,
            intents,
            "A single title, caption, or annotation is enough; avoid a freeform scene.",
        );
    }

    if template_hint.is_some()
        || contains_any(
            &lower,
            &[
                "visualize",
                "explainer",
                "diagram",
                "animated",
                "animate",
                "kinetic",
                "kinetic text",
                "motion graphic",
                "framework",
                "step-by-step",
                "three-step",
                "chart",
                "timeline graphic",
                "process",
                "callout",
                "callouts",
                "progress bar",
                "highlight box",
                "highlight the",
            ],
        )
        || wants_still_graphic_motion_scene(&lower)
    {
        return route(
            VisualSupportLane::MotionScene,
            supporting_lanes,
            &["view_timeline", "plan_motion_scene", "apply_edl"],
            needs,
            intents,
            "The request is best answered with designed, timed, layered graphics.",
        )
        .with_template_hint(template_hint);
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
            needs,
            intents,
            "The request asks for footage-like visual evidence or cutaway support.",
        );
    }

    route(
        VisualSupportLane::Broll,
        supporting_lanes,
        &["find_broll_opportunities", "search_broll", "view_timeline"],
        needs,
        intents,
        "Default to footage-like support when the visual intent is underspecified.",
    )
}

fn route(
    primary_lane: VisualSupportLane,
    supporting_lanes: Vec<VisualSupportLane>,
    next_tools: &[&str],
    needs: Vec<VisualSupportNeed>,
    intents: Vec<VisualSupportIntent>,
    rationale: &str,
) -> VisualSupportRoute {
    let supporting_lanes: Vec<VisualSupportLane> = supporting_lanes
        .into_iter()
        .filter(|lane| *lane != primary_lane)
        .collect();
    let next_tools = next_tools.iter().map(|tool| (*tool).to_string()).collect();
    let plan_steps = plan_steps_for_route(primary_lane, &supporting_lanes);
    VisualSupportRoute {
        primary_lane,
        supporting_lanes,
        next_tools,
        needs,
        intents,
        plan_steps,
        rationale: rationale.to_string(),
    }
}

fn visual_needs_for_request(lower: &str) -> Vec<VisualSupportNeed> {
    let mut needs = Vec::new();
    if contains_any(
        lower,
        &[
            "explain",
            "explainer",
            "visualize",
            "abstract",
            "concept",
            "framework",
            "system",
        ],
    ) {
        needs.push(need(
            VisualSupportNeedKind::AbstractExplanation,
            "The request asks the agent to make an idea visually understandable.",
        ));
    }
    if contains_any(
        lower,
        &[
            "product",
            "screenshot",
            "logo",
            "dashboard",
            "website",
            "chart",
            "diagram",
            "image",
            "still",
        ],
    ) {
        needs.push(need(
            VisualSupportNeedKind::ProductOrAssetMention,
            "The segment references a concrete visual asset that can be shown or overlaid.",
        ));
    }
    if contains_any(
        lower,
        &[
            "real-world",
            "evidence",
            "location",
            "place",
            "event",
            "supporting footage",
            "supporting b-roll",
        ],
    ) {
        needs.push(need(
            VisualSupportNeedKind::FactualReference,
            "The request needs footage-like evidence rather than only graphics.",
        ));
    }
    if contains_any(
        lower,
        &[
            "step",
            "steps",
            "step-by-step",
            "three-step",
            "3 step",
            "process",
            "list",
            "framework",
        ],
    ) {
        needs.push(need(
            VisualSupportNeedKind::ListOrProcess,
            "The content is a sequence or process that benefits from a structured explainer.",
        ));
    }
    if contains_any(
        lower,
        &["energetic", "powerful", "emotional", "quote", "punch"],
    ) {
        needs.push(need(
            VisualSupportNeedKind::EmotionalEmphasis,
            "The request is trying to make a moment land visually.",
        ));
    }
    if contains_any(
        lower,
        &["jump cut", "hide the cut", "cover the cut", "dirty cut"],
    ) {
        needs.push(need(
            VisualSupportNeedKind::JumpCutCover,
            "The visual support should cover an edit discontinuity.",
        ));
    }
    if contains_any(lower, &["chapter", "topic transition", "segment intro"]) {
        needs.push(need(
            VisualSupportNeedKind::ChapterTransition,
            "The visual support introduces or separates a topic.",
        ));
    }
    if contains_any(lower, &["sponsor", "cta", "call to action", "offer"]) {
        needs.push(need(
            VisualSupportNeedKind::SponsorCta,
            "The visual support is attached to a sponsor or action prompt.",
        ));
    }
    if contains_any(
        lower,
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
        needs.push(need(
            VisualSupportNeedKind::ExistingFootagePolish,
            "The request changes existing footage/audio instead of introducing new visual material.",
        ));
    }
    needs
}

fn visual_intents_for_request(
    lower: &str,
    needs: &[VisualSupportNeed],
) -> Vec<VisualSupportIntent> {
    let mut intents = Vec::new();
    if needs.iter().any(|need| {
        matches!(
            need.kind,
            VisualSupportNeedKind::AbstractExplanation | VisualSupportNeedKind::ListOrProcess
        )
    }) {
        push_unique(&mut intents, VisualSupportIntent::Explain);
    }
    if needs.iter().any(|need| {
        matches!(
            need.kind,
            VisualSupportNeedKind::ProductOrAssetMention | VisualSupportNeedKind::FactualReference
        )
    }) {
        push_unique(&mut intents, VisualSupportIntent::ShowEvidence);
    }
    if needs
        .iter()
        .any(|need| need.kind == VisualSupportNeedKind::ChapterTransition)
    {
        push_unique(&mut intents, VisualSupportIntent::IntroduceChapter);
        push_unique(&mut intents, VisualSupportIntent::Summarize);
    }
    if needs
        .iter()
        .any(|need| need.kind == VisualSupportNeedKind::JumpCutCover)
    {
        push_unique(&mut intents, VisualSupportIntent::HideEdit);
    }
    if needs
        .iter()
        .any(|need| need.kind == VisualSupportNeedKind::EmotionalEmphasis)
    {
        push_unique(&mut intents, VisualSupportIntent::EmphasizeQuote);
    }
    if contains_any(
        lower,
        &["compare", "before after", "before/after", "versus", "vs"],
    ) {
        push_unique(&mut intents, VisualSupportIntent::CompareBeforeAfter);
    }
    if intents.is_empty() {
        push_unique(&mut intents, VisualSupportIntent::DecorateLightly);
    }
    intents
}

fn need(kind: VisualSupportNeedKind, rationale: &str) -> VisualSupportNeed {
    VisualSupportNeed {
        kind,
        rationale: rationale.to_string(),
    }
}

fn push_unique(intents: &mut Vec<VisualSupportIntent>, intent: VisualSupportIntent) {
    if !intents.contains(&intent) {
        intents.push(intent);
    }
}

fn plan_steps_for_route(
    primary_lane: VisualSupportLane,
    supporting_lanes: &[VisualSupportLane],
) -> Vec<VisualSupportPlanStep> {
    let mut lanes = vec![primary_lane];
    lanes.extend_from_slice(supporting_lanes);
    let mut steps = Vec::new();
    for lane in lanes {
        match lane {
            VisualSupportLane::MotionScene => {
                // Keep the MotionScene lane as a two-step contract so agents
                // do not stop at planning and forget to persist the returned EDL.
                steps.push(step(
                    lane,
                    "plan_motion_scene",
                    "Draft a native layered MotionScene using transcript-backed content: pass `headline` or `evidence_text`, and pass exact `step_labels` for step/process scenes.",
                ));
                steps.push(step(
                    lane,
                    "apply_edl",
                    "Apply the returned Set Motion Scene EDL after review.",
                ));
            }
            VisualSupportLane::Broll => steps.push(step(
                lane,
                "find_broll_opportunities",
                "Find or generate footage-like visual evidence before inserting it as b-roll or PiP.",
            )),
            VisualSupportLane::TitleAnnotation => steps.push(step(
                lane,
                "apply_edl",
                "Add a simple title, lower third, caption, label, or annotation.",
            )),
            VisualSupportLane::EffectsFinishing => steps.push(step(
                lane,
                "plan_emphasis",
                "Use existing footage/audio finishing primitives such as reframe, blur, color, speed, or emphasis.",
            )),
            VisualSupportLane::TimelineEdit => steps.push(step(
                lane,
                "apply_edl",
                "Make the structural timeline edit before adding visual support.",
            )),
        }
    }
    steps
}

fn step(lane: VisualSupportLane, tool: &str, action: &str) -> VisualSupportPlanStep {
    VisualSupportPlanStep {
        lane,
        tool: tool.to_string(),
        action: action.to_string(),
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
