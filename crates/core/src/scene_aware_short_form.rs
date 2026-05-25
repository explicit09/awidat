//! Scene-aware short-form edit planning.
//!
//! This module is deliberately read-only: it turns existing evidence
//! sidecars into structured recommendations and EDL fragments that other
//! tools or skills can review and apply.

#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneAwareShortFormInput {
    pub asset_id: String,
    pub clip_id: String,
    pub source_width: u32,
    pub source_height: u32,
    #[serde(default)]
    pub transcript: serde_json::Value,
    #[serde(default)]
    pub topics: serde_json::Value,
    #[serde(default)]
    pub editorial_moments: serde_json::Value,
    #[serde(default)]
    pub audio_energy: serde_json::Value,
    #[serde(default)]
    pub face: serde_json::Value,
    #[serde(default)]
    pub gaze: serde_json::Value,
    #[serde(default)]
    pub scenes: serde_json::Value,
    #[serde(default)]
    pub shot: serde_json::Value,
    #[serde(default)]
    pub frame_quality: serde_json::Value,
    #[serde(default)]
    pub composition: serde_json::Value,
    #[serde(default)]
    pub clip: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneAwareShortFormPlan {
    pub asset_id: String,
    pub clip_id: String,
    pub target: ShortFormTarget,
    pub shots: Vec<SceneShotAnalysis>,
    pub caption_plan: Vec<CaptionRecommendation>,
    pub recommendations: Vec<EditRecommendation>,
    pub readiness: OutputReadiness,
    pub verification_plan: Vec<String>,
    pub edl_fragment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortFormTarget {
    pub aspect_ratio: String,
    pub safe_area: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneShotAnalysis {
    pub start_s: f64,
    pub end_s: f64,
    pub subject_center: Option<Point>,
    pub face_box: Option<BoxRegion>,
    pub eye_line_y: Option<f64>,
    pub mouth_band: Option<VerticalBand>,
    pub headroom: Option<f64>,
    pub negative_space: Option<NegativeSpace>,
    pub safe_text_zones: Vec<CaptionPlacement>,
    pub busy_regions: Vec<CaptionPlacement>,
    pub clip_matches: Vec<ClipMatchCandidate>,
    pub motion_intensity: f64,
    pub framing_quality: FramingQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoxRegion {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VerticalBand {
    pub top: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegativeSpace {
    pub side: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipMatchCandidate {
    pub asset_id: Option<String>,
    pub time_s: f64,
    pub similarity: f64,
    pub confidence: f64,
    pub basis: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionPlacement {
    Bottom,
    Upper,
    Left,
    Right,
}

impl CaptionPlacement {
    fn edl_value(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Upper => "top",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FramingQuality {
    Strong,
    Usable,
    Weak,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionRecommendation {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub word_timings: Vec<CaptionWordTiming>,
    pub placement: CaptionPlacement,
    pub style: CaptionStyle,
    pub transcript_reason: String,
    pub visual_reason: String,
    pub safety_reason: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionWordTiming {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionStyle {
    Plain,
    Boxed,
    Minimal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditRecommendation {
    pub kind: RecommendationKind,
    pub start_s: f64,
    pub end_s: f64,
    pub operation: String,
    pub transcript_reason: String,
    pub visual_reason: String,
    pub pacing_reason: String,
    pub safety_reason: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationKind {
    SetOutputFormat,
    Reframe,
    Caption,
    PunchIn,
    Hold,
    Broll,
    KeywordOverlay,
    MotionScene,
    Trim,
    Speed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputReadiness {
    pub status: ReadinessStatus,
    pub issues: Vec<String>,
    pub checks: ReadinessChecks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    ReadyForReview,
    NeedsReview,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessChecks {
    pub hook_first_3s: bool,
    pub captions_present: bool,
    pub caption_visual_safety: bool,
    pub mobile_safe_area: bool,
    pub rhythm_varies: bool,
    pub render_integrity_deferred: bool,
}

pub fn build_scene_aware_short_form_plan(
    input: SceneAwareShortFormInput,
) -> SceneAwareShortFormPlan {
    let shots = analyze_shots(&input);
    let caption_plan = plan_captions(&input, &shots);
    let mut recommendations = Vec::new();
    recommendations.push(output_format_recommendation());
    recommendations.extend(plan_reframes(&input, &shots));
    recommendations.extend(caption_plan.iter().map(caption_recommendation));
    recommendations.extend(plan_pacing(&input, &shots));
    recommendations.extend(plan_support_visuals(&input, &shots));
    let edl_fragment = build_edl_fragment(&input, &caption_plan, &recommendations);
    let readiness = readiness(&input, &caption_plan, &recommendations);

    SceneAwareShortFormPlan {
        asset_id: input.asset_id,
        clip_id: input.clip_id,
        target: ShortFormTarget {
            aspect_ratio: "9:16".into(),
            safe_area: "mobile".into(),
        },
        shots,
        caption_plan,
        recommendations,
        readiness,
        verification_plan: vec![
            "Confirm hook lands inside the first 3 seconds of the proposed timeline.".into(),
            "Run caption readability and mobile safe-area checks before render handoff.".into(),
            "Inspect timeline diff to confirm scene-aware captions, reframes, overlays, b-roll, and pacing match this plan.".into(),
            "Run render verification for black frames, broken audio, and awkward cut timing when a render exists.".into(),
        ],
        edl_fragment,
    }
}

fn analyze_shots(input: &SceneAwareShortFormInput) -> Vec<SceneShotAnalysis> {
    let raw_shots = input
        .shot
        .pointer("/shots")
        .or_else(|| input.scenes.pointer("/shots"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_else(|| {
            vec![serde_json::json!({"start_s": 0.0, "end_s": inferred_end_s(&input.transcript)})]
        });

    raw_shots
        .iter()
        .map(|shot| {
            let start_s = number_field(shot, "start_s", 0.0);
            let end_s = number_field(shot, "end_s", start_s + 2.0);
            let face_box = median_face_box(&input.face, start_s, end_s);
            let negative_space = best_negative_space(&input.composition, start_s, end_s);
            let framing_quality =
                framing_quality(&input.composition, &input.frame_quality, start_s, end_s);
            let motion_intensity = motion_intensity(shot);
            let composition_safe_zones = composition_zones(
                &input.composition,
                start_s,
                end_s,
                &["safe_text_zones", "safe_regions"],
            );
            let composition_busy_regions = composition_zones(
                &input.composition,
                start_s,
                end_s,
                &["busy_regions", "unsafe_text_zones", "protected_regions"],
            );
            let safe_text_zones = safe_text_zones(
                face_box,
                negative_space.as_ref(),
                &composition_safe_zones,
                &composition_busy_regions,
            );
            let busy_regions =
                busy_regions(face_box, negative_space.as_ref(), &composition_busy_regions);
            SceneShotAnalysis {
                start_s,
                end_s,
                subject_center: face_box.map(|face| Point {
                    x: round3(face.x + face.w / 2.0),
                    y: round3(face.y + face.h / 2.0),
                }),
                face_box,
                eye_line_y: face_box.map(|face| round3(face.y + face.h * 0.28)),
                mouth_band: face_box.map(|face| VerticalBand {
                    top: round3(face.y + face.h * 0.56),
                    bottom: round3(face.y + face.h * 0.86),
                }),
                headroom: face_box.map(|face| round3(face.y)),
                negative_space,
                safe_text_zones,
                busy_regions,
                clip_matches: clip_match_candidates(shot),
                motion_intensity,
                framing_quality,
            }
        })
        .collect()
}

fn plan_captions(
    input: &SceneAwareShortFormInput,
    shots: &[SceneShotAnalysis],
) -> Vec<CaptionRecommendation> {
    transcript_segments(&input.transcript)
        .into_iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .map(|segment| {
            let shot = shot_at(shots, segment.start_s);
            let placement = shot
                .and_then(|shot| shot.safe_text_zones.first().copied())
                .unwrap_or(CaptionPlacement::Bottom);
            let style = if shot.is_some_and(|shot| shot.busy_regions.contains(&placement)) {
                CaptionStyle::Boxed
            } else if shot.is_some_and(|shot| shot.motion_intensity > 0.7) {
                CaptionStyle::Minimal
            } else {
                CaptionStyle::Plain
            };
            let visual_reason = shot
                .map(caption_visual_reason)
                .unwrap_or_else(|| "uses default mobile caption zone".into());
            let safety_reason = if shot.is_some_and(|shot| shot.face_box.is_some()) {
                format!(
                    "placement avoids detected face/eye/mouth regions; bottom_safe={}",
                    !shot.is_some_and(|shot| placement_overlaps_face(
                        CaptionPlacement::Bottom,
                        shot.face_box
                    ))
                )
            } else {
                "no face region detected in this shot".into()
            };
            CaptionRecommendation {
                start_s: segment.start_s,
                end_s: segment.end_s,
                text: segment.text,
                word_timings: segment.word_timings,
                placement,
                style,
                transcript_reason: "phrase-level transcript segment with source timings".into(),
                visual_reason,
                safety_reason,
                confidence: shot.map(confidence_for_shot).unwrap_or(0.58),
            }
        })
        .collect()
}

fn output_format_recommendation() -> EditRecommendation {
    EditRecommendation {
        kind: RecommendationKind::SetOutputFormat,
        start_s: 0.0,
        end_s: 0.0,
        operation: "Set Output Format".into(),
        transcript_reason: "short-form delivery request".into(),
        visual_reason: "mobile-first vertical output is required for short-form review".into(),
        pacing_reason: "sets the canvas before caption, reframe, and support-visual decisions"
            .into(),
        safety_reason: "mobile safe area is explicitly requested".into(),
        confidence: 1.0,
    }
}

fn plan_reframes(
    input: &SceneAwareShortFormInput,
    shots: &[SceneShotAnalysis],
) -> Vec<EditRecommendation> {
    let aspect = f64::from(input.source_width.max(1)) / f64::from(input.source_height.max(1));
    shots
        .iter()
        .filter(|shot| {
            aspect > 0.8
                || shot.framing_quality == FramingQuality::Weak
                || shot
                    .headroom
                    .is_some_and(|headroom| !(0.04..=0.22).contains(&headroom))
        })
        .map(|shot| EditRecommendation {
            kind: RecommendationKind::Reframe,
            start_s: shot.start_s,
            end_s: shot.end_s,
            operation: "Set Effect / awidat.reframe".into(),
            transcript_reason: "selected short-form clip should preserve the speaker or action"
                .into(),
            visual_reason: "source framing needs vertical subject-aware composition".into(),
            pacing_reason: "stable reframe avoids unnecessary cuts for a layout problem".into(),
            safety_reason: "keeps face and important subject regions away from caption zones"
                .into(),
            confidence: confidence_for_shot(shot),
        })
        .collect()
}

fn caption_recommendation(caption: &CaptionRecommendation) -> EditRecommendation {
    EditRecommendation {
        kind: RecommendationKind::Caption,
        start_s: caption.start_s,
        end_s: caption.end_s,
        operation: "Insert Caption".into(),
        transcript_reason: caption.transcript_reason.clone(),
        visual_reason: caption.visual_reason.clone(),
        pacing_reason: "caption cue follows source speech timing".into(),
        safety_reason: caption.safety_reason.clone(),
        confidence: caption.confidence,
    }
}

fn plan_pacing(
    input: &SceneAwareShortFormInput,
    shots: &[SceneShotAnalysis],
) -> Vec<EditRecommendation> {
    let mut out = Vec::new();
    for shot in shots {
        let duration = shot.end_s - shot.start_s;
        if duration >= 2.2
            && shot.motion_intensity <= 0.25
            && shot.framing_quality != FramingQuality::Strong
        {
            out.push(EditRecommendation {
                kind: RecommendationKind::PunchIn,
                start_s: shot.start_s,
                end_s: (shot.start_s + 0.75).min(shot.end_s),
                operation: "Set Effect / subtle punch-in animation".into(),
                transcript_reason: text_near(&input.transcript, shot.start_s)
                    .unwrap_or_else(|| "static spoken section".into()),
                visual_reason: "low visual change in a static shot".into(),
                pacing_reason: "adds a rhythm change without random filler".into(),
                safety_reason: "subtle scale motion preserves face/reaction value".into(),
                confidence: 0.7,
            });
        }
        if is_hold_moment(input, shot) {
            out.push(EditRecommendation {
                kind: RecommendationKind::Hold,
                start_s: shot.start_s,
                end_s: shot.end_s,
                operation: "Hold".into(),
                transcript_reason: "editorial moment suggests emotional, reveal, or payoff value"
                    .into(),
                visual_reason: "preserve original face/reaction/action value".into(),
                pacing_reason: "avoid cutting across important reaction or motion".into(),
                safety_reason: "no cover visual recommended over the key moment".into(),
                confidence: 0.78,
            });
        }
    }
    for silence in input
        .audio_energy
        .pointer("/silences")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        let start_s = number_field(silence, "start_s", 0.0);
        let end_s = number_field(silence, "end_s", start_s);
        if end_s - start_s >= 0.8 {
            out.push(EditRecommendation {
                kind: RecommendationKind::Trim,
                start_s,
                end_s,
                operation: "Trim / remove low-value silence".into(),
                transcript_reason: "no transcript words during detected silence".into(),
                visual_reason: "safe only when not crossing important motion".into(),
                pacing_reason: "short-form static silence reads slow".into(),
                safety_reason: "review cut timing against facial reaction before applying".into(),
                confidence: 0.68,
            });
        }
    }
    out
}

fn plan_support_visuals(
    input: &SceneAwareShortFormInput,
    shots: &[SceneShotAnalysis],
) -> Vec<EditRecommendation> {
    let mut out = Vec::new();
    for recommendation in broll_recommendations(input) {
        let start_s = number_field(recommendation, "start_s", f64::NAN);
        let end_s = number_field(recommendation, "end_s", f64::NAN);
        if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
            continue;
        }
        let Some(shot) = shot_at(shots, start_s) else {
            continue;
        };
        let category = string_field(recommendation, "category").unwrap_or("broll");
        let strategy = string_field(recommendation, "strategy").unwrap_or("support_visual");
        let rationale =
            string_field(recommendation, "rationale").unwrap_or("fused B-roll recommendation");
        let kind = match category {
            "statistic" | "technical_concept" => RecommendationKind::MotionScene,
            _ if should_cover_with_broll(shot) => RecommendationKind::Broll,
            _ => RecommendationKind::KeywordOverlay,
        };
        out.push(EditRecommendation {
            kind,
            start_s,
            end_s: end_s.min(start_s + 4.0),
            operation: match kind {
                RecommendationKind::MotionScene => "Set Motion Scene / Insert Title".into(),
                RecommendationKind::KeywordOverlay => "Insert Title / keyword overlay".into(),
                _ => "Insert B-roll / PiP".into(),
            },
            transcript_reason: format!("fused B-roll recommendation: {rationale}"),
            visual_reason: format!("{category} visual opportunity using {strategy} strategy"),
            pacing_reason:
                "uses the shared understanding spine instead of re-scanning transcript keywords"
                    .into(),
            safety_reason: "review placement against face/reaction value before applying".into(),
            confidence: number_field(recommendation, "score", 0.74).clamp(0.0, 0.95),
        });
    }
    for segment in transcript_segments(&input.transcript) {
        let Some(shot) = shot_at(shots, segment.start_s) else {
            continue;
        };
        let lower = segment.text.to_lowercase();
        if contains_concrete_reference(&lower) && should_cover_with_broll(shot) {
            let visual_reason = if shot.clip_matches.is_empty() {
                "current shot is weak or low-value enough to cover".into()
            } else {
                format!(
                    "current shot is weak enough to cover; CLIP match candidates available ({})",
                    shot.clip_matches.len()
                )
            };
            out.push(EditRecommendation {
                kind: RecommendationKind::Broll,
                start_s: segment.start_s,
                end_s: segment.end_s.min(segment.start_s + 3.0),
                operation: "Insert B-roll / PiP".into(),
                transcript_reason: format!("concrete visual reference: {:?}", segment.text),
                visual_reason,
                pacing_reason: "adds visual variety tied to spoken content".into(),
                safety_reason: "use reviewable b-roll, not random filler; preserve face when reaction is the value".into(),
                confidence: 0.74,
            });
        }
        if contains_abstract_visualizable_claim(&lower) {
            out.push(EditRecommendation {
                kind: RecommendationKind::MotionScene,
                start_s: segment.start_s,
                end_s: segment.end_s.min(segment.start_s + 3.0),
                operation: "Set Motion Scene / Insert Title".into(),
                transcript_reason: format!("abstract or structured claim: {:?}", segment.text),
                visual_reason: "designed overlay explains the idea better than stock footage"
                    .into(),
                pacing_reason: "adds a semantic rhythm change without cutting away randomly".into(),
                safety_reason:
                    "place in safe negative space or full-screen only when the shot is weak".into(),
                confidence: 0.72,
            });
            out.push(EditRecommendation {
                kind: RecommendationKind::KeywordOverlay,
                start_s: segment.start_s,
                end_s: segment.end_s.min(segment.start_s + 2.0),
                operation: "Insert Title / keyword overlay".into(),
                transcript_reason: format!("highlight structured claim: {:?}", segment.text),
                visual_reason: "uses safe text space instead of covering the subject".into(),
                pacing_reason: "adds a quick semantic beat before returning to the shot".into(),
                safety_reason: "title overlay remains mobile safe-area aware".into(),
                confidence: 0.69,
            });
        }
    }
    out
}

fn broll_recommendations(input: &SceneAwareShortFormInput) -> Vec<&serde_json::Value> {
    input
        .clip
        .pointer("/broll_recommendations")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .collect()
}

fn build_edl_fragment(
    input: &SceneAwareShortFormInput,
    captions: &[CaptionRecommendation],
    recommendations: &[EditRecommendation],
) -> String {
    let mut lines = vec![
        "*** Begin EDL".to_string(),
        "*** Set Output Format".to_string(),
        "+ aspect_ratio: 9:16".to_string(),
        "+ safe_area: mobile".to_string(),
        "+ platform: vertical_short".to_string(),
    ];

    if recommendations
        .iter()
        .any(|rec| rec.kind == RecommendationKind::Reframe)
    {
        let center = recommendations
            .iter()
            .find(|rec| rec.kind == RecommendationKind::Reframe)
            .and_then(|rec| rec_visual_center(input, rec.start_s))
            .unwrap_or(Point { x: 0.5, y: 0.42 });
        let source_aspect =
            f64::from(input.source_width.max(1)) / f64::from(input.source_height.max(1));
        let zoom = if source_aspect > 9.0 / 16.0 {
            (source_aspect / (9.0 / 16.0)).clamp(1.05, 3.0)
        } else {
            1.05
        };
        lines.extend([
            "*** Set Effect".to_string(),
            format!("@@ anchor: clip_uuid={}", input.clip_id),
            "+ effect: awidat.reframe".to_string(),
            format!(
                "+ params_json: {{\"zoom\":{},\"x\":{},\"y\":{}}}",
                fmt_number(zoom),
                fmt_number((center.x - 0.5) * 2.0),
                fmt_number((center.y - 0.5) * 2.0)
            ),
            "+ rationale: Scene-aware vertical reframe".to_string(),
        ]);
    }

    for caption in captions {
        lines.extend([
            "*** Insert Caption".to_string(),
            format!("+ start_s: {}", fmt_number(caption.start_s)),
            format!("+ end_s: {}", fmt_number(caption.end_s)),
            format!("+ text: {}", json_string(&caption.text)),
            format!("+ position: {}", caption.placement.edl_value()),
            "+ font_size: 56".to_string(),
            "+ color: #FFFFFF".to_string(),
            "+ safe_area: mobile".to_string(),
            format!(
                "+ word_timings_json: {}",
                serde_json::to_string(&caption.word_timings).unwrap_or_else(|_| "[]".into())
            ),
        ]);
    }

    for rec in recommendations {
        match rec.kind {
            RecommendationKind::PunchIn => {
                let animation_json = serde_json::json!({
                    "id": format!("scene-aware-{}-punch-{}", safe_id(&input.clip_id), millis(rec.start_s)),
                    "target": {
                        "kind": "clip_parameter",
                        "clip_id": input.clip_id,
                        "parameter": "overlay.scale"
                    },
                    "keyframes": [
                        {"time_s": rec.start_s, "value": 1.0},
                        {"time_s": rec.start_s + 0.25, "value": 1.06},
                        {"time_s": rec.end_s, "value": 1.0}
                    ],
                    "metadata_only": false,
                    "rationale": rec.pacing_reason
                });
                lines.extend([
                    "*** Set Parameter Animation".to_string(),
                    format!("+ animation_json: {}", animation_json),
                ]);
            }
            RecommendationKind::Trim => {
                if let Some(trim_lines) = edge_trim_edl(input, rec) {
                    lines.extend(trim_lines);
                }
            }
            RecommendationKind::KeywordOverlay => lines.extend([
                "*** Insert Title".to_string(),
                format!("+ start_s: {}", fmt_number(rec.start_s)),
                format!("+ end_s: {}", fmt_number(rec.end_s)),
                format!("+ text: {}", json_string(&keyword_overlay_text(rec))),
                "+ position: top".to_string(),
                "+ font_size: 64".to_string(),
                "+ color: #FFFFFF".to_string(),
                "+ font_weight: bold".to_string(),
                "+ animation: fade_in_out".to_string(),
                "+ safe_area: mobile".to_string(),
            ]),
            RecommendationKind::MotionScene => {
                let scene_json = serde_json::json!({
                    "id": format!("scene-aware-{}-{}", safe_id(&input.clip_id), millis(rec.start_s)),
                    "duration_s": (rec.end_s - rec.start_s).max(0.5),
                    "fps": 30.0,
                    "width": 1080,
                    "height": 1920,
                    "layers": [{
                        "id": "headline",
                        "kind": "text",
                        "from_s": 0.0,
                        "duration_s": (rec.end_s - rec.start_s).max(0.5),
                        "z_index": 10,
                        "params": {
                            "text": keyword_overlay_text(rec),
                            "layout": "center_safe",
                            "animation": "fade_slide_in"
                        }
                    }],
                    "rationale": rec.visual_reason
                });
                lines.extend([
                    "*** Set Motion Scene".to_string(),
                    format!("+ scene_json: {}", scene_json),
                ]);
            }
            _ => {}
        }
    }

    lines.push("*** End EDL".to_string());
    lines.join("\n") + "\n"
}

fn readiness(
    input: &SceneAwareShortFormInput,
    captions: &[CaptionRecommendation],
    recommendations: &[EditRecommendation],
) -> OutputReadiness {
    let hook_first_3s = hook_start_s(input).unwrap_or(0.0) <= 3.0;
    let captions_present = !captions.is_empty();
    let caption_visual_safety = captions
        .iter()
        .all(|caption| !caption.safety_reason.contains("overlaps"));
    let rhythm_varies = recommendations.iter().any(|rec| {
        matches!(
            rec.kind,
            RecommendationKind::PunchIn
                | RecommendationKind::Broll
                | RecommendationKind::MotionScene
                | RecommendationKind::Trim
        )
    });
    let checks = ReadinessChecks {
        hook_first_3s,
        captions_present,
        caption_visual_safety,
        mobile_safe_area: true,
        rhythm_varies,
        render_integrity_deferred: true,
    };
    let mut issues = Vec::new();
    if !hook_first_3s {
        issues.push("hook does not land inside the first 3 seconds".into());
    }
    if !captions_present {
        issues.push("captions missing".into());
    }
    if !caption_visual_safety {
        issues.push("caption placement may cover important visual regions".into());
    }
    if !rhythm_varies {
        issues.push("visual rhythm has no planned short-form variation".into());
    }
    OutputReadiness {
        status: if issues.is_empty() {
            ReadinessStatus::ReadyForReview
        } else {
            ReadinessStatus::NeedsReview
        },
        issues,
        checks,
    }
}

fn edge_trim_edl(
    input: &SceneAwareShortFormInput,
    rec: &EditRecommendation,
) -> Option<Vec<String>> {
    let duration_s = inferred_end_s(&input.transcript).max(max_shot_end_s(input));
    let trim_field = if rec.start_s <= 0.12 {
        Some(format!("+ start: {}", fmt_number(rec.end_s)))
    } else if duration_s - rec.end_s <= 0.12 {
        Some(format!("+ end: {}", fmt_number(rec.start_s)))
    } else {
        None
    }?;
    Some(vec![
        "*** Trim Clip".to_string(),
        format!("@@ anchor: clip_uuid={}", input.clip_id),
        trim_field,
    ])
}

#[derive(Debug)]
struct TranscriptSegment {
    start_s: f64,
    end_s: f64,
    text: String,
    word_timings: Vec<CaptionWordTiming>,
}

fn transcript_segments(transcript: &serde_json::Value) -> Vec<TranscriptSegment> {
    transcript
        .pointer("/segments")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .map(|segment| TranscriptSegment {
            start_s: number_field(segment, "start_s", number_field(segment, "start", 0.0)),
            end_s: number_field(segment, "end_s", number_field(segment, "end", 0.0)),
            text: segment
                .get("text")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            word_timings: word_timings(segment),
        })
        .collect()
}

fn word_timings(segment: &serde_json::Value) -> Vec<CaptionWordTiming> {
    segment
        .get("words")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|word| {
            let text = word
                .get("text")
                .or_else(|| word.get("word"))
                .and_then(|value| value.as_str())?
                .trim()
                .to_string();
            if text.is_empty() {
                return None;
            }
            let start_s = number_field(word, "start_s", number_field(word, "start", f64::NAN));
            let end_s = number_field(word, "end_s", number_field(word, "end", f64::NAN));
            if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
                return None;
            }
            Some(CaptionWordTiming {
                text,
                start_s: round3(start_s),
                end_s: round3(end_s),
            })
        })
        .collect()
}

fn inferred_end_s(transcript: &serde_json::Value) -> f64 {
    transcript_segments(transcript)
        .last()
        .map(|segment| segment.end_s)
        .unwrap_or(3.0)
}

fn max_shot_end_s(input: &SceneAwareShortFormInput) -> f64 {
    input
        .shot
        .pointer("/shots")
        .or_else(|| input.scenes.pointer("/shots"))
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .map(|shot| number_field(shot, "end_s", 0.0))
        .max_by(f64::total_cmp)
        .unwrap_or(0.0)
}

fn median_face_box(face: &serde_json::Value, start_s: f64, end_s: f64) -> Option<BoxRegion> {
    let mut boxes = Vec::new();
    for frame in face
        .pointer("/per_frame")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        let t_s = number_field(frame, "t_s", number_field(frame, "time_s", start_s));
        if t_s < start_s || t_s > end_s {
            continue;
        }
        let Some(faces) = frame.get("faces").and_then(|value| value.as_array()) else {
            continue;
        };
        let best = faces.iter().max_by(|left, right| {
            number_field(left, "confidence", 0.0).total_cmp(&number_field(right, "confidence", 0.0))
        })?;
        if let Some(region) = parse_box(best) {
            boxes.push(region);
        }
    }
    if boxes.is_empty() {
        return None;
    }
    Some(BoxRegion {
        x: round3(median(boxes.iter().map(|region| region.x).collect())),
        y: round3(median(boxes.iter().map(|region| region.y).collect())),
        w: round3(median(boxes.iter().map(|region| region.w).collect())),
        h: round3(median(boxes.iter().map(|region| region.h).collect())),
    })
}

fn parse_box(value: &serde_json::Value) -> Option<BoxRegion> {
    let region = if value.get("bbox").is_some() {
        value.get("bbox")?
    } else {
        value
    };
    if ["x", "y", "w", "h"]
        .iter()
        .all(|key| region.get(*key).is_some())
    {
        return Some(BoxRegion {
            x: clamp01(number_field(region, "x", 0.0)),
            y: clamp01(number_field(region, "y", 0.0)),
            w: clamp01(number_field(region, "w", 0.0)),
            h: clamp01(number_field(region, "h", 0.0)),
        });
    }
    if ["left", "top", "right", "bottom"]
        .iter()
        .all(|key| region.get(*key).is_some())
    {
        let left = number_field(region, "left", 0.0);
        let top = number_field(region, "top", 0.0);
        let right = number_field(region, "right", left);
        let bottom = number_field(region, "bottom", top);
        return Some(BoxRegion {
            x: clamp01(left),
            y: clamp01(top),
            w: clamp01(right - left),
            h: clamp01(bottom - top),
        });
    }
    None
}

fn best_negative_space(
    composition: &serde_json::Value,
    start_s: f64,
    end_s: f64,
) -> Option<NegativeSpace> {
    composition
        .pointer("/regions")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|region| overlaps_time(region, start_s, end_s))
        .filter_map(|region| {
            let space = region.get("negative_space")?;
            Some(NegativeSpace {
                side: space
                    .get("side")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                score: round3(number_field(space, "score", 0.0)),
            })
        })
        .max_by(|left, right| left.score.total_cmp(&right.score))
}

fn composition_zones(
    composition: &serde_json::Value,
    start_s: f64,
    end_s: f64,
    fields: &[&str],
) -> Vec<CaptionPlacement> {
    let mut zones = Vec::new();
    for region in composition
        .pointer("/regions")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|region| overlaps_time(region, start_s, end_s))
    {
        for field in fields {
            collect_placements(region.get(*field), &mut zones);
        }
    }
    zones.sort_by_key(|placement| *placement as u8);
    zones.dedup();
    zones
}

fn collect_placements(value: Option<&serde_json::Value>, zones: &mut Vec<CaptionPlacement>) {
    match value {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                collect_placements(Some(item), zones);
            }
        }
        Some(serde_json::Value::Object(_)) => {
            let placement = value
                .and_then(|item| item.get("placement"))
                .or_else(|| value.and_then(|item| item.get("side")))
                .or_else(|| value.and_then(|item| item.get("zone")))
                .and_then(|item| item.as_str())
                .and_then(caption_placement_from_str);
            if let Some(placement) = placement
                && !zones.contains(&placement)
            {
                zones.push(placement);
            }
        }
        Some(serde_json::Value::String(label)) => {
            if let Some(placement) = caption_placement_from_str(label)
                && !zones.contains(&placement)
            {
                zones.push(placement);
            }
        }
        _ => {}
    }
}

fn caption_placement_from_str(label: &str) -> Option<CaptionPlacement> {
    match label.to_lowercase().as_str() {
        "top" | "upper" | "upper_third" => Some(CaptionPlacement::Upper),
        "left" | "left_third" => Some(CaptionPlacement::Left),
        "right" | "right_third" => Some(CaptionPlacement::Right),
        "bottom" | "lower" | "lower_third" => Some(CaptionPlacement::Bottom),
        _ => None,
    }
}

fn framing_quality(
    composition: &serde_json::Value,
    frame_quality: &serde_json::Value,
    start_s: f64,
    end_s: f64,
) -> FramingQuality {
    let framing = composition
        .pointer("/regions")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .find(|region| overlaps_time(region, start_s, end_s))
        .and_then(|region| region.get("framing"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_lowercase();
    if framing.contains("weak") || sharp_ratio(frame_quality, start_s, end_s) < 0.45 {
        FramingQuality::Weak
    } else if framing.contains("strong") || framing.contains("good") {
        FramingQuality::Strong
    } else if framing == "unknown" {
        FramingQuality::Unknown
    } else {
        FramingQuality::Usable
    }
}

fn sharp_ratio(frame_quality: &serde_json::Value, start_s: f64, end_s: f64) -> f64 {
    let mut total = 0.0;
    let mut sharp = 0.0;
    for frame in frame_quality
        .pointer("/per_frame")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        let t_s = number_field(frame, "t_s", number_field(frame, "time_s", start_s));
        if t_s < start_s || t_s > end_s {
            continue;
        }
        total += 1.0;
        if frame
            .get("is_sharp")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            sharp += 1.0;
        }
    }
    if total == 0.0 { 1.0 } else { sharp / total }
}

fn motion_intensity(shot: &serde_json::Value) -> f64 {
    if let Some(value) = shot
        .get("motion_intensity")
        .and_then(|value| value.as_f64())
    {
        return value.clamp(0.0, 1.0);
    }
    match shot
        .get("motion")
        .and_then(|value| value.as_str())
        .unwrap_or("static")
    {
        "whip" | "fast" | "fast-cut" | "handheld" => 0.85,
        "slow-pan" | "pan" | "tilt" => 0.45,
        _ => 0.12,
    }
}

fn safe_text_zones(
    face_box: Option<BoxRegion>,
    negative_space: Option<&NegativeSpace>,
    composition_safe_zones: &[CaptionPlacement],
    composition_busy_regions: &[CaptionPlacement],
) -> Vec<CaptionPlacement> {
    let mut zones = Vec::new();
    for placement in composition_safe_zones {
        if !composition_busy_regions.contains(placement)
            && !placement_overlaps_face(*placement, face_box)
        {
            zones.push(*placement);
        }
    }
    if !composition_safe_zones.is_empty() && !zones.is_empty() {
        return zones;
    }
    if let Some(space) = negative_space
        && space.score >= 0.6
        && let Some(placement) = caption_placement_from_str(&space.side)
        && !composition_busy_regions.contains(&placement)
        && !zones.contains(&placement)
    {
        zones.push(placement);
    }
    for placement in [
        CaptionPlacement::Bottom,
        CaptionPlacement::Upper,
        CaptionPlacement::Left,
        CaptionPlacement::Right,
    ] {
        if !zones.contains(&placement)
            && !composition_busy_regions.contains(&placement)
            && !placement_overlaps_face(placement, face_box)
        {
            zones.push(placement);
        }
    }
    if zones.is_empty() {
        zones.push(CaptionPlacement::Bottom);
    }
    zones
}

fn busy_regions(
    face_box: Option<BoxRegion>,
    negative_space: Option<&NegativeSpace>,
    composition_busy_regions: &[CaptionPlacement],
) -> Vec<CaptionPlacement> {
    let mut busy = composition_busy_regions.to_vec();
    for placement in [
        CaptionPlacement::Bottom,
        CaptionPlacement::Upper,
        CaptionPlacement::Left,
        CaptionPlacement::Right,
    ] {
        if placement_overlaps_face(placement, face_box) {
            busy.push(placement);
        }
    }
    if negative_space.is_none_or(|space| space.score < 0.45) {
        busy.push(CaptionPlacement::Bottom);
    }
    busy.sort_by_key(|placement| *placement as u8);
    busy.dedup();
    busy
}

fn clip_match_candidates(shot: &serde_json::Value) -> Vec<ClipMatchCandidate> {
    shot.get("match_candidates")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|candidate| {
            let time_s = number_field(candidate, "time_s", f64::NAN);
            if !time_s.is_finite() {
                return None;
            }
            Some(ClipMatchCandidate {
                asset_id: candidate
                    .get("asset_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
                time_s: round3(time_s),
                similarity: round3(number_field(candidate, "similarity", 0.0)),
                confidence: round3(number_field(candidate, "confidence", 0.0)),
                basis: candidate
                    .get("basis")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
            })
        })
        .collect()
}

fn caption_visual_reason(shot: &SceneShotAnalysis) -> String {
    if let Some(placement) = shot.safe_text_zones.first()
        && let Some(space_placement) = shot
            .negative_space
            .as_ref()
            .and_then(|space| caption_placement_from_str(&space.side))
        && space_placement != *placement
    {
        return format!("uses composition safe text zone: {placement:?}");
    }
    shot.negative_space
        .as_ref()
        .map(|space| format!("uses {} negative space", space.side))
        .unwrap_or_else(|| "uses default mobile caption zone".into())
}

fn placement_overlaps_face(placement: CaptionPlacement, face_box: Option<BoxRegion>) -> bool {
    let Some(face) = face_box else {
        return false;
    };
    let (x1, y1, x2, y2): (f64, f64, f64, f64) = match placement {
        CaptionPlacement::Bottom => (0.08, 0.66, 0.92, 0.9),
        CaptionPlacement::Upper => (0.08, 0.08, 0.92, 0.28),
        CaptionPlacement::Left => (0.05, 0.2, 0.38, 0.82),
        CaptionPlacement::Right => (0.62, 0.2, 0.95, 0.82),
    };
    let face_x2 = face.x + face.w;
    let face_y2 = face.y + face.h;
    x2.min(face_x2) > x1.max(face.x) && y2.min(face_y2) > y1.max(face.y)
}

fn shot_at(shots: &[SceneShotAnalysis], t_s: f64) -> Option<&SceneShotAnalysis> {
    shots
        .iter()
        .find(|shot| t_s >= shot.start_s && t_s < shot.end_s)
        .or_else(|| shots.first())
}

fn should_cover_with_broll(shot: &SceneShotAnalysis) -> bool {
    shot.framing_quality == FramingQuality::Weak
        || (shot.face_box.is_none() && shot.motion_intensity < 0.3)
}

fn contains_concrete_reference(text: &str) -> bool {
    [
        "look at", "screen", "laptop", "phone", "product", "city", "office", "event", "camera",
        "chart", "graph", "website",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn contains_abstract_visualizable_claim(text: &str) -> bool {
    [
        "three reasons",
        "two reasons",
        "number",
        "percent",
        "%",
        "compare",
        "comparison",
        "framework",
        "steps",
        "claim",
        "because",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn string_field<'a>(value: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(|field| field.as_str())
}

fn is_hold_moment(input: &SceneAwareShortFormInput, shot: &SceneShotAnalysis) -> bool {
    input
        .editorial_moments
        .pointer("/moments")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .any(|moment| {
            overlaps_time(moment, shot.start_s, shot.end_s)
                && moment
                    .get("kind")
                    .and_then(|value| value.as_str())
                    .is_some_and(|kind| {
                        matches!(kind, "emotional_beat" | "reveal" | "payoff" | "reaction")
                    })
        })
}

fn hook_start_s(input: &SceneAwareShortFormInput) -> Option<f64> {
    input
        .editorial_moments
        .pointer("/moments")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter(|moment| {
            moment
                .get("kind")
                .and_then(|value| value.as_str())
                .is_some_and(|kind| kind == "hook")
        })
        .map(|moment| number_field(moment, "start_s", 99.0))
        .min_by(f64::total_cmp)
        .or_else(|| {
            transcript_segments(&input.transcript)
                .first()
                .map(|segment| segment.start_s)
        })
}

fn text_near(transcript: &serde_json::Value, t_s: f64) -> Option<String> {
    transcript_segments(transcript)
        .into_iter()
        .find(|segment| t_s >= segment.start_s && t_s < segment.end_s)
        .map(|segment| segment.text)
}

fn rec_visual_center(input: &SceneAwareShortFormInput, start_s: f64) -> Option<Point> {
    analyze_shots(input)
        .into_iter()
        .find(|shot| start_s >= shot.start_s && start_s < shot.end_s)
        .and_then(|shot| shot.subject_center)
}

fn overlaps_time(value: &serde_json::Value, start_s: f64, end_s: f64) -> bool {
    let item_start = number_field(value, "start_s", start_s);
    let item_end = number_field(value, "end_s", end_s);
    item_start < end_s && item_end > start_s
}

fn confidence_for_shot(shot: &SceneShotAnalysis) -> f64 {
    let face_signal = if shot.face_box.is_some() { 0.18 } else { 0.0 };
    let space_signal = shot
        .negative_space
        .as_ref()
        .map(|space| space.score * 0.18)
        .unwrap_or(0.0);
    let framing_signal = match shot.framing_quality {
        FramingQuality::Strong => 0.2,
        FramingQuality::Usable => 0.14,
        FramingQuality::Weak => 0.08,
        FramingQuality::Unknown => 0.04,
    };
    round3((0.46 + face_signal + space_signal + framing_signal).clamp(0.0, 0.95))
}

fn number_field(value: &serde_json::Value, field: &str, default: f64) -> f64 {
    value
        .get(field)
        .and_then(|field| field.as_f64())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn fmt_number(value: f64) -> String {
    let rounded = round3(value);
    if rounded.fract() == 0.0 {
        format!("{rounded:.1}")
    } else {
        rounded.to_string()
    }
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn keyword_overlay_text(rec: &EditRecommendation) -> String {
    rec.transcript_reason
        .split_once(':')
        .map(|(_, text)| text.trim().trim_matches('"').to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Key idea".into())
}

fn safe_id(value: &str) -> String {
    let id: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    id.trim_matches('_').to_string()
}

fn millis(value: f64) -> i64 {
    (value * 1000.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avoids_bottom_captions_when_face_occupies_lower_frame() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1920,
            source_height: 1080,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.0,
                    "end_s": 2.4,
                    "text": "This product launch changed everything",
                    "words": [
                        {"start_s": 0.0, "end_s": 0.3, "text": "This"},
                        {"start_s": 0.3, "end_s": 0.7, "text": "product"},
                        {"start_s": 0.7, "end_s": 1.1, "text": "launch"},
                        {"start_s": 1.1, "end_s": 1.5, "text": "changed"},
                        {"start_s": 1.5, "end_s": 2.1, "text": "everything"}
                    ]
                }]
            }),
            face: serde_json::json!({
                "per_frame": [
                    {"t_s": 0.2, "faces": [{"confidence": 0.98, "x": 0.38, "y": 0.48, "w": 0.24, "h": 0.34}]},
                    {"t_s": 1.2, "faces": [{"confidence": 0.97, "x": 0.39, "y": 0.49, "w": 0.24, "h": 0.34}]}
                ]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 2.5,
                    "framing": "usable",
                    "negative_space": {"side": "top", "score": 0.82}
                }]
            }),
            shot: serde_json::json!({
                "shots": [{"start_s": 0.0, "end_s": 2.5, "type": "medium", "motion": "static"}]
            }),
            frame_quality: serde_json::json!({
                "per_frame": [{"t_s": 0.2, "is_sharp": true}, {"t_s": 1.2, "is_sharp": true}]
            }),
            editorial_moments: serde_json::json!({
                "moments": [{"start_s": 0.0, "end_s": 2.4, "kind": "hook", "score": 0.9}]
            }),
            ..SceneAwareShortFormInput::default()
        });

        assert_eq!(plan.caption_plan[0].placement, CaptionPlacement::Upper);
        assert!(plan.caption_plan[0].safety_reason.contains("face"));
        assert!(plan.edl_fragment.contains("*** Insert Caption"));
        assert!(plan.readiness.checks.caption_visual_safety);
    }

    #[test]
    fn composition_safe_and_busy_zones_override_negative_space() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1080,
            source_height: 1920,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "text": "The dashboard changed right here"
                }]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "negative_space": {"side": "bottom", "score": 0.9},
                    "safe_text_zones": ["upper", "right"],
                    "busy_regions": ["bottom"]
                }]
            }),
            shot: serde_json::json!({
                "shots": [{"start_s": 0.0, "end_s": 2.0, "motion": "static"}]
            }),
            ..SceneAwareShortFormInput::default()
        });

        assert_eq!(plan.caption_plan[0].placement, CaptionPlacement::Upper);
        assert_eq!(
            plan.shots[0].safe_text_zones,
            vec![CaptionPlacement::Upper, CaptionPlacement::Right]
        );
        assert!(
            plan.shots[0]
                .busy_regions
                .contains(&CaptionPlacement::Bottom)
        );
        assert!(
            plan.caption_plan[0]
                .visual_reason
                .contains("composition safe text zone")
        );
    }

    #[test]
    fn shot_clip_matches_are_preserved_as_visual_evidence() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1080,
            source_height: 1920,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "text": "Look at the laptop screen"
                }]
            }),
            shot: serde_json::json!({
                "shots": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "motion": "static",
                    "match_candidates": [{
                        "asset_id": "raw/demo.mp4",
                        "time_s": 14.2,
                        "similarity": 0.82,
                        "confidence": 0.71,
                        "basis": "clip_embedding"
                    }]
                }]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "framing": "weak"
                }]
            }),
            ..SceneAwareShortFormInput::default()
        });

        assert_eq!(plan.shots[0].clip_matches.len(), 1);
        assert_eq!(
            plan.shots[0].clip_matches[0].asset_id.as_deref(),
            Some("raw/demo.mp4")
        );
        assert!(
            plan.recommendations
                .iter()
                .any(|rec| rec.kind == RecommendationKind::Broll
                    && rec.visual_reason.contains("CLIP match"))
        );
    }

    #[test]
    fn caption_edl_preserves_source_word_timings() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1080,
            source_height: 1920,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.0,
                    "end_s": 1.2,
                    "text": "Watch this",
                    "words": [
                        {"start_s": 0.0, "end_s": 0.4, "text": "Watch"},
                        {"start_s": 0.4, "end_s": 0.8, "word": "this"}
                    ]
                }]
            }),
            ..SceneAwareShortFormInput::default()
        });

        assert_eq!(plan.caption_plan[0].word_timings.len(), 2);
        assert!(plan.edl_fragment.contains("\"text\":\"Watch\""));
        assert!(plan.edl_fragment.contains("\"text\":\"this\""));
    }

    #[test]
    fn routes_concrete_and_abstract_support_visuals_differently() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1080,
            source_height: 1920,
            transcript: serde_json::json!({
                "segments": [
                    {"start_s": 0.0, "end_s": 2.0, "text": "Look at the laptop screen"},
                    {"start_s": 2.0, "end_s": 4.0, "text": "There are three reasons this works"}
                ]
            }),
            shot: serde_json::json!({
                "shots": [
                    {"start_s": 0.0, "end_s": 2.0, "type": "no-face", "motion": "static"},
                    {"start_s": 2.0, "end_s": 4.0, "type": "medium", "motion": "static"}
                ]
            }),
            composition: serde_json::json!({
                "regions": [
                    {"start_s": 0.0, "end_s": 2.0, "framing": "weak", "negative_space": {"side": "bottom", "score": 0.3}},
                    {"start_s": 2.0, "end_s": 4.0, "framing": "usable", "negative_space": {"side": "right", "score": 0.7}}
                ]
            }),
            ..SceneAwareShortFormInput::default()
        });

        assert!(
            plan.recommendations
                .iter()
                .any(|rec| rec.kind == RecommendationKind::Broll)
        );
        assert!(
            plan.recommendations
                .iter()
                .any(|rec| rec.kind == RecommendationKind::MotionScene)
        );
        assert!(
            plan.verification_plan
                .iter()
                .any(|item| item.contains("timeline diff"))
        );
        assert!(
            !plan
                .edl_fragment
                .contains("B-roll placeholder: review concrete visual")
        );
    }

    #[test]
    fn fused_broll_recommendations_drive_scene_aware_support_visuals() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1080,
            source_height: 1920,
            transcript: serde_json::json!({
                "segments": [{"start_s": 5.0, "end_s": 8.0, "text": "This claim needs support"}]
            }),
            shot: serde_json::json!({
                "shots": [{"start_s": 5.0, "end_s": 8.0, "type": "medium", "motion": "static"}]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 5.0,
                    "end_s": 8.0,
                    "framing": "usable",
                    "negative_space": {"side": "top", "score": 0.75}
                }]
            }),
            clip: serde_json::json!({
                "broll_recommendations": [{
                    "id": "broll-rec:asset:stat:5000",
                    "moment_id": "understanding:asset:stat:5000",
                    "start_s": 5.0,
                    "end_s": 8.0,
                    "score": 0.91,
                    "category": "statistic",
                    "strategy": "motion_graphic",
                    "rationale": "Animated stat card for the numeric claim."
                }]
            }),
            ..SceneAwareShortFormInput::default()
        });

        let recommendation = plan
            .recommendations
            .iter()
            .find(|rec| {
                rec.transcript_reason
                    .contains("Animated stat card for the numeric claim")
            })
            .expect("fused B-roll recommendation should become a scene-aware recommendation");
        assert_eq!(recommendation.kind, RecommendationKind::MotionScene);
        assert!(
            recommendation
                .pacing_reason
                .contains("shared understanding spine")
        );
    }

    #[test]
    fn generated_edl_fragment_is_parseable_for_reviewable_ops() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1920,
            source_height: 1080,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "text": "There are three reasons this works",
                    "words": [
                        {"start_s": 0.0, "end_s": 0.3, "text": "There"},
                        {"start_s": 0.3, "end_s": 0.6, "text": "are"}
                    ]
                }]
            }),
            shot: serde_json::json!({
                "shots": [{"start_s": 0.0, "end_s": 2.0, "type": "medium", "motion": "static"}]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "framing": "usable",
                    "negative_space": {"side": "top", "score": 0.75}
                }]
            }),
            ..SceneAwareShortFormInput::default()
        });

        let parsed = crate::edl::parse(&plan.edl_fragment).expect("scene-aware EDL should parse");
        assert!(parsed.ops.len() >= 4);
    }

    #[test]
    fn pacing_recommendations_emit_reviewable_animation_and_edge_trim_edl() {
        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1080,
            source_height: 1920,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.9,
                    "end_s": 4.0,
                    "text": "This section is visually static but important"
                }]
            }),
            audio_energy: serde_json::json!({
                "silences": [{"start_s": 0.0, "end_s": 0.85}]
            }),
            shot: serde_json::json!({
                "shots": [{"start_s": 0.0, "end_s": 4.0, "type": "medium", "motion": "static"}]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 4.0,
                    "framing": "usable",
                    "negative_space": {"side": "top", "score": 0.7}
                }]
            }),
            ..SceneAwareShortFormInput::default()
        });

        assert!(plan.edl_fragment.contains("*** Set Parameter Animation"));
        assert!(plan.edl_fragment.contains("*** Trim Clip"));
        let parsed = crate::edl::parse(&plan.edl_fragment).expect("scene-aware EDL should parse");
        assert!(
            parsed
                .ops
                .iter()
                .any(|op| matches!(op, crate::edl::EdlOp::SetParameterAnimation { .. }))
        );
        assert!(
            parsed
                .ops
                .iter()
                .any(|op| matches!(op, crate::edl::EdlOp::TrimClip { .. }))
        );
    }

    #[test]
    fn generated_edl_applies_to_timeline_and_leaves_scene_aware_diff() {
        use awidat_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Timeline,
            Track, TrackChild, TrackKind,
        };

        let plan = build_scene_aware_short_form_plan(SceneAwareShortFormInput {
            asset_id: "raw/demo.mp4".into(),
            clip_id: "clip-1".into(),
            source_width: 1920,
            source_height: 1080,
            transcript: serde_json::json!({
                "segments": [{
                    "start_s": 0.9,
                    "end_s": 4.0,
                    "text": "There are three reasons this works",
                    "words": [{"start_s": 0.9, "end_s": 1.2, "text": "There"}]
                }]
            }),
            audio_energy: serde_json::json!({
                "silences": [{"start_s": 0.0, "end_s": 0.85}]
            }),
            shot: serde_json::json!({
                "shots": [{"start_s": 0.0, "end_s": 4.0, "type": "medium", "motion": "static"}]
            }),
            composition: serde_json::json!({
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 4.0,
                    "framing": "usable",
                    "negative_space": {"side": "top", "score": 0.75}
                }]
            }),
            ..SceneAwareShortFormInput::default()
        });

        let mut timeline = Timeline::empty("scene-aware-test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-1");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/demo.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        timeline.tracks.children.push(StackChild::Track(track));

        let env = crate::edl::parse(&plan.edl_fragment).expect("scene-aware EDL should parse");
        let (new_timeline, outcome) =
            crate::edl::apply(&timeline, &env, &crate::edl::AnchorContext::empty())
                .expect("scene-aware EDL should apply");

        assert!(outcome.applied.len() >= 5);
        let StackChild::Track(v1) = &new_timeline.tracks.children[0] else {
            panic!("expected V1")
        };
        let TrackChild::Clip(trimmed) = &v1.children[0] else {
            panic!("expected trimmed clip")
        };
        assert!(
            (trimmed
                .source_range
                .as_ref()
                .unwrap()
                .start_time
                .to_seconds()
                - 0.85)
                .abs()
                < 1e-9
        );
        assert!(
            new_timeline
                .tracks
                .children
                .iter()
                .any(|track| matches!(track, StackChild::Track(track) if track.name == "Titles"))
        );
    }
}
