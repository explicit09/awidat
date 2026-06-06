//! `plan_delivery_export` - read-only export/render delivery planner.
//!
//! Resolves a human delivery intent to an existing Awidat render/export path.
//! It does not mutate the project or start work; it returns the concrete tools
//! an agent should call next.

use awidat_proto::professional::{ExportPreset, HardwareAccelerationPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `plan_delivery_export`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanDeliveryExportArgs {
    /// High-level delivery request, e.g. YouTube upload, client review,
    /// archive master, podcast audio, or stream-copy/remux.
    pub intent: String,
    /// Optional explicit destination: youtube, shorts, social, review,
    /// archive, podcast, turnover, or remux.
    #[serde(default)]
    pub destination: Option<String>,
    /// Whether the caller needs a delivery package instead of only a render.
    #[serde(default)]
    pub needs_package: Option<bool>,
    /// Prefer stream-copy/remux when the source is already approved and no
    /// re-encode is needed.
    #[serde(default)]
    pub prefer_remux: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryKind {
    Youtube,
    Shorts,
    Review,
    Archive,
    Podcast,
    Turnover,
    Remux,
}

/// Run `plan_delivery_export`. The project root is unused because this is a
/// read-only intent planner; render/package tools do project inspection.
pub fn run(args: PlanDeliveryExportArgs, _ctx: McpToolCtx) -> Result<String, String> {
    if args.prefer_remux == Some(true) {
        return serialize(remux_plan());
    }

    let Some(kind) = classify_delivery(&args) else {
        return serialize(review_response());
    };

    let needs_package = args.needs_package.unwrap_or_else(|| default_package(kind));
    let body = match kind {
        DeliveryKind::Youtube => package_plan("youtube", needs_package),
        DeliveryKind::Shorts => package_plan("shorts", needs_package),
        DeliveryKind::Podcast => package_plan("podcast", needs_package),
        DeliveryKind::Turnover => package_plan("turnover", true),
        DeliveryKind::Archive => archive_plan(needs_package),
        DeliveryKind::Review => review_render_plan(needs_package),
        DeliveryKind::Remux => remux_plan(),
    };
    serialize(body)
}

fn classify_delivery(args: &PlanDeliveryExportArgs) -> Option<DeliveryKind> {
    let text = format!(
        "{} {}",
        args.destination.as_deref().unwrap_or_default(),
        args.intent
    )
    .to_ascii_lowercase()
    .replace(['-', '_'], " ");

    if contains_any(
        &text,
        &["remux", "rewrap", "stream copy", "without re encoding"],
    ) {
        Some(DeliveryKind::Remux)
    } else if contains_any(&text, &["turnover", "aaf", "xml", "edl handoff", "conform"]) {
        Some(DeliveryKind::Turnover)
    } else if contains_any(
        &text,
        &["archive", "master", "future recut", "prores", "dnx"],
    ) {
        Some(DeliveryKind::Archive)
    } else if contains_any(
        &text,
        &["shorts", "short form", "tiktok", "reels", "vertical"],
    ) {
        Some(DeliveryKind::Shorts)
    } else if contains_any(&text, &["youtube", "upload"]) {
        Some(DeliveryKind::Youtube)
    } else if contains_any(&text, &["podcast", "audio only", "m4a", "wav"]) {
        Some(DeliveryKind::Podcast)
    } else if contains_any(&text, &["review", "client", "approval", "screening"]) {
        Some(DeliveryKind::Review)
    } else {
        None
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn default_package(kind: DeliveryKind) -> bool {
    matches!(
        kind,
        DeliveryKind::Youtube
            | DeliveryKind::Shorts
            | DeliveryKind::Podcast
            | DeliveryKind::Turnover
    )
}

fn package_plan(format: &str, needs_package: bool) -> serde_json::Value {
    let preset = package_preset(format);
    let mut tools = vec![render_preflight_tool()];
    if needs_package {
        tools.push(serde_json::json!({
            "name": "export_package",
            "args": {
                "format": format,
                "hardware_acceleration": "off"
            },
            "reason": "Render the timeline with the selected delivery preset and write sidecars, preflight, metadata, and package artifacts."
        }));
        tools.push(serde_json::json!({
            "name": "poll_render",
            "args": {
                "job_id": "<job_id from export_package>"
            }
        }));
        tools.push(serde_json::json!({
            "name": "verify_render",
            "args": {
                "output_path": "<artifacts.mp4 from export_package>"
            }
        }));
    } else {
        tools.push(start_timeline_render_tool(None));
        tools.push(verify_render_tool("<output_path from start_render>"));
    }

    serde_json::json!({
        "status": "ready",
        "summary_for_agent": format!("Use the {format} package/export path, then verify the produced artifact."),
        "recommended": {
            "delivery_kind": delivery_kind_for_format(format),
            "render_strategy": if needs_package { "export_package" } else { "timeline_render" },
            "export_preset_id": preset.id,
            "profile_id": preset.profile.id,
            "package_format": format,
            "container": preset.output.container,
            "video_codec": preset.video.as_ref().map(|video| video.codec.as_str()),
            "audio_codec": preset.audio.as_ref().map(|audio| audio.codec.as_str()),
            "width": preset.profile.width,
            "height": preset.profile.height,
            "aspect_ratio": preset.profile.aspect_ratio,
            "loudness_lufs": preset.profile.loudness_lufs,
            "preflight_checks": preflight_checks(&preset),
        },
        "follow_up_tools": tools,
        "verification_requirements": verification_requirements(format),
    })
}

fn archive_plan(needs_package: bool) -> serde_json::Value {
    let preset = ExportPreset::archival_prores();
    let mut tools = vec![render_preflight_tool()];
    tools.push(start_timeline_render_tool(Some("prores")));
    tools.push(verify_render_tool("<output_path from start_render>"));
    if needs_package {
        tools.push(serde_json::json!({
            "name": "local_review_package",
            "args": {
                "render_path": "<output_path from start_render>",
                "tags": ["archive", "master"]
            }
        }));
    }

    serde_json::json!({
        "status": "ready",
        "summary_for_agent": "Render an intraframe archive master, verify it, then package the receipt/artifact if requested.",
        "recommended": {
            "delivery_kind": "archive",
            "render_strategy": "timeline_render",
            "export_preset_id": preset.id,
            "profile_id": preset.profile.id,
            "container": preset.output.container,
            "video_codec": preset.video.as_ref().map(|video| video.codec.as_str()),
            "audio_codec": preset.audio.as_ref().map(|audio| audio.codec.as_str()),
            "width": preset.profile.width,
            "height": preset.profile.height,
            "aspect_ratio": preset.profile.aspect_ratio,
            "loudness_lufs": preset.profile.loudness_lufs,
            "preflight_checks": preflight_checks(&preset),
        },
        "follow_up_tools": tools,
        "verification_requirements": [
            "codec/container match the archive preset",
            "audio is present and unclipped",
            "no black frames or missing media",
            "verification receipt is kept with the package"
        ],
    })
}

fn review_render_plan(needs_package: bool) -> serde_json::Value {
    let mut tools = vec![render_preflight_tool(), start_timeline_render_tool(None)];
    tools.push(verify_render_tool("<output_path from start_render>"));
    if needs_package {
        tools.push(serde_json::json!({
            "name": "local_review_package",
            "args": {
                "render_path": "<output_path from start_render>",
                "tags": ["review"]
            }
        }));
    }

    serde_json::json!({
        "status": "ready",
        "summary_for_agent": "Render a compatibility review file, verify it, then create a local review package if requested.",
        "recommended": {
            "delivery_kind": "review",
            "render_strategy": "timeline_render",
            "export_preset_id": "timeline_default_h264",
            "profile_id": "review_default",
            "package_format": if needs_package { "local_review" } else { "none" },
            "container": "mp4",
            "video_codec": "libx264",
            "audio_codec": "aac",
            "preflight_checks": ["timeline_renderability", "missing_media", "captions", "audio_presence"],
        },
        "follow_up_tools": tools,
        "verification_requirements": verification_requirements("review"),
    })
}

fn remux_plan() -> serde_json::Value {
    serde_json::json!({
        "status": "ready",
        "summary_for_agent": "Use stream_remux only when the source streams are already approved and no re-encode is required.",
        "recommended": {
            "delivery_kind": "remux",
            "render_strategy": "stream_remux",
            "export_preset_id": "stream_copy_remux",
            "container": "mp4",
            "preflight_checks": ["source_streams_known", "codec_compatible_with_container", "output_path_safe"],
        },
        "follow_up_tools": [
            {
                "name": "stream_remux",
                "args": {
                    "input": "<project-relative approved media>",
                    "output": "renders/remux/final-review.mp4",
                    "container": "mp4",
                    "streams": [
                        {"id": "video", "kind": "video", "source_index": 0, "mode": "copy"},
                        {"id": "audio", "kind": "audio", "source_index": 1, "mode": "copy"}
                    ],
                    "metadata": {}
                }
            },
            {
                "name": "poll_render",
                "args": {
                    "job_id": "<job_id from stream_remux>"
                }
            },
            {
                "name": "verify_render",
                "args": {
                    "output_path": "<output_path from stream_remux>"
                }
            }
        ],
        "verification_requirements": [
            "verify_render reports stream_remux evidence",
            "codec/container are accepted by the destination",
            "duration, audio streams, and captions/subtitles are still present"
        ],
    })
}

fn review_response() -> serde_json::Value {
    serde_json::json!({
        "status": "needs_review",
        "summary_for_agent": "Delivery intent is too vague to choose a preset safely.",
        "review_questions": [
            "What destination is this for: YouTube, Shorts/Reels/TikTok, client review, archive, podcast/audio-only, turnover, or remux?",
            "Does the delivery need captions/subtitle sidecars, a thumbnail, metadata, or a package folder?",
            "Should this re-encode from the timeline, or stream-copy/remux an already approved media file?"
        ],
    })
}

fn package_preset(format: &str) -> ExportPreset {
    match format {
        "shorts" => ExportPreset::vertical_short_form(),
        "podcast" => ExportPreset::podcast_audio(),
        _ => {
            let profile = awidat_proto::professional::DeliveryProfile::youtube_1080p();
            let bitrate_kbps = profile.video_bitrate_kbps.unwrap_or(12_000);
            ExportPreset {
                id: format!("package_{}", profile.id),
                name: format!("{} Package Export", profile.name),
                mode: awidat_proto::professional::ExportMode::AudioVideo,
                profile,
                output: awidat_proto::professional::ExportOutputSettings {
                    hardware_acceleration: HardwareAccelerationPolicy::Off,
                    ..awidat_proto::professional::ExportOutputSettings::mp4()
                },
                video: Some(awidat_proto::professional::VideoExportSettings::h264(
                    bitrate_kbps,
                )),
                audio: Some(awidat_proto::professional::AudioExportSettings::aac(192)),
                range: Default::default(),
            }
        }
    }
}

fn preflight_checks(preset: &ExportPreset) -> Vec<String> {
    preset
        .profile
        .preflight_checks
        .iter()
        .map(|check| format!("{check:?}").to_ascii_lowercase())
        .collect()
}

fn render_preflight_tool() -> serde_json::Value {
    serde_json::json!({
        "name": "render_preflight",
        "args": {
            "scope": "timeline"
        },
        "reason": "Check renderability before starting an export job."
    })
}

fn start_timeline_render_tool(preset: Option<&str>) -> serde_json::Value {
    let mut args = serde_json::json!({
        "scope": "timeline"
    });
    if let Some(preset) = preset {
        args["preset"] = serde_json::json!(preset);
    }
    serde_json::json!({
        "name": "start_render",
        "args": args
    })
}

fn verify_render_tool(output_path: &str) -> serde_json::Value {
    serde_json::json!({
        "name": "verify_render",
        "args": {
            "output_path": output_path
        }
    })
}

fn delivery_kind_for_format(format: &str) -> &str {
    match format {
        "shorts" => "shorts",
        "podcast" => "podcast",
        "turnover" => "turnover",
        _ => "youtube",
    }
}

fn verification_requirements(format: &str) -> Vec<&'static str> {
    match format {
        "shorts" => vec![
            "1080x1920 vertical geometry",
            "audio is present and unclipped",
            "captions/sidecars are present if requested",
            "no black frames or missing media",
        ],
        "podcast" => vec![
            "audio stream is present",
            "loudness is near the podcast target",
            "no long unexpected silence",
            "metadata sidecars are present if packaged",
        ],
        "turnover" => vec![
            "turnover manifest exists",
            "cut lists and department handoffs exist",
            "source assets are listed with status/checksum where available",
        ],
        _ => vec![
            "codec/container match the selected preset",
            "dimensions, FPS, and duration are expected",
            "audio is present and unclipped",
            "captions, thumbnail, metadata, and sidecars match the request",
            "no black frames or missing media",
        ],
    }
}

fn serialize(value: serde_json::Value) -> Result<String, String> {
    serde_json::to_string(&value).map_err(|e| format!("plan_delivery_export serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Read-only delivery/export planner. Pass a human delivery intent plus optional \
destination. The tool selects an existing Awidat export/package path, returns \
the intended ExportPreset/profile, preflight checks, ordered follow-up tools \
(`render_preflight`, `start_render` or `export_package`/`stream_remux`, \
`poll_render`, `verify_render`, optional packaging), and verification \
requirements. It never starts a render or writes files.\
";
