//! Tool for preparing generated-media timeline placement.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::generated_media::registry::{
    GeneratedMediaState, Registry, validate_generated_output_path,
};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Build an EDL fragment for a completed generated-media output.
pub struct UseGeneratedMediaTool;

#[derive(Debug, Deserialize)]
struct Args {
    job_id: String,
    anchor: AnchorArg,
    duration_s: f64,
    #[serde(default)]
    position: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum AnchorArg {
    Transcript { transcript_snippet: String },
    Uuid { clip_uuid: String },
}

#[async_trait]
impl ToolHandler for UseGeneratedMediaTool {
    fn name(&self) -> &'static str {
        "use_generated_media"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["job_id", "anchor", "duration_s"],
                "properties": {
                    "job_id": { "type": "string" },
                    "anchor": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["transcript_snippet"],
                                "properties": { "transcript_snippet": { "type": "string" } }
                            },
                            {
                                "type": "object",
                                "required": ["clip_uuid"],
                                "properties": { "clip_uuid": { "type": "string" } }
                            }
                        ]
                    },
                    "duration_s": { "type": "number", "minimum": 0.5, "maximum": 30.0 },
                    "position": { "type": "string", "enum": ["overlay", "replace"] }
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
                "use_generated_media: invalid args ({e}). Required: job_id, anchor, duration_s."
            ))
        })?;
        if !(0.5..=30.0).contains(&args.duration_s) {
            return Err(FunctionCallError::RespondToModel(format!(
                "use_generated_media: duration_s={} out of range. Use 0.5-30.0.",
                args.duration_s
            )));
        }
        let position = match args.position.as_deref().unwrap_or("overlay") {
            "overlay" => "overlay",
            "replace" => "replace",
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "use_generated_media: invalid position '{other}'. Use 'overlay' or 'replace'."
                )));
            }
        };

        let registry = Registry::load_or_default(&ctx.project_root)
            .map_err(|e| FunctionCallError::RespondToModel(format!("use_generated_media: {e}")))?;
        let record = registry.get(&args.job_id).ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "use_generated_media: job '{}' not found.",
                args.job_id
            ))
        })?;
        if record.state != GeneratedMediaState::Succeeded {
            return Err(FunctionCallError::RespondToModel(format!(
                "use_generated_media: job '{}' is {:?}, not succeeded.",
                record.job_id, record.state
            )));
        }
        let asset_path = record.output_video_path().ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "use_generated_media: job '{}' has no output video path.",
                record.job_id
            ))
        })?;
        validate_generated_output_path(asset_path)
            .map_err(|e| FunctionCallError::RespondToModel(format!("use_generated_media: {e}")))?;
        let absolute_path = ctx.project_root.join(asset_path);
        let edl_fragment = build_edl_fragment(asset_path, &args.anchor, args.duration_s, position)
            .map_err(|message| {
                FunctionCallError::RespondToModel(format!("use_generated_media: {message}"))
            })?;

        Ok(ToolOutput::text(
            serde_json::json!({
                "asset_path": asset_path,
                "absolute_path": absolute_path.display().to_string(),
                "edl_fragment": edl_fragment,
                "provenance": {
                    "job_id": record.job_id,
                    "provider": record.provider,
                    "model": record.model,
                    "prompt_hash": record.prompt_hash,
                    "requires_disclosure": record.requires_disclosure,
                    "uses_likeness": record.uses_likeness
                },
                "next_step": "Hand the edl_fragment to apply_edl to place the cutaway, then run view_timeline/podcast_visual_polish and verify this generated asset still matches the resolved transcript anchor before claiming B-roll is done."
            })
            .to_string(),
        ))
    }
}

fn build_edl_fragment(
    asset_rel: &str,
    anchor: &AnchorArg,
    duration_s: f64,
    position: &str,
) -> Result<String, String> {
    let anchor_line = match anchor {
        AnchorArg::Transcript { transcript_snippet } => {
            reject_edl_control_chars(transcript_snippet, "transcript_snippet")?;
            let escaped = transcript_snippet.replace('"', "\\\"");
            format!("@@ anchor: transcript_snippet=\"{escaped}\"")
        }
        AnchorArg::Uuid { clip_uuid } => {
            reject_edl_control_chars(clip_uuid, "clip_uuid")?;
            format!("@@ anchor: clip_uuid={clip_uuid}")
        }
    };
    Ok(format!(
        "*** Begin EDL\n\
         *** Insert BRoll\n\
         {anchor_line}\n\
         + asset: {asset_rel}\n\
         + duration_s: {duration_s}\n\
         + position: {position}\n\
         *** End EDL\n"
    ))
}

fn reject_edl_control_chars(value: &str, field: &str) -> Result<(), String> {
    if value.contains('\n') || value.contains('\r') {
        return Err(format!("{field} cannot contain newlines"));
    }
    Ok(())
}

const DESCRIPTION: &str = "\
Return a ready-to-apply Insert BRoll EDL fragment for a completed generated \
media asset. This does not call apply_edl or mutate the timeline.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolHandler;

    #[tokio::test]
    async fn succeeded_generated_video_returns_insert_broll_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let record = crate::generated_media::registry::GeneratedMediaRecord::mock_succeeded(
            "generated-media-test",
            "raw/generated/mock/generated-media-test.mp4",
            "misty mountain cutaway",
        );
        crate::generated_media::registry::Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record)
            .unwrap();

        let output = UseGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "use_generated_media".into(),
                    args: serde_json::json!({
                        "job_id": "generated-media-test",
                        "anchor": { "transcript_snippet": "then we reached the pass" },
                        "duration_s": 3.0
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(
            body["asset_path"],
            "raw/generated/mock/generated-media-test.mp4"
        );
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("*** Insert BRoll")
        );
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("@@ anchor: transcript_snippet=\"then we reached the pass\"")
        );
        assert_eq!(body["provenance"]["provider"], "mock");
    }

    #[tokio::test]
    async fn transcript_anchor_rejects_newline_injection() {
        let dir = tempfile::tempdir().unwrap();
        let record = crate::generated_media::registry::GeneratedMediaRecord::mock_succeeded(
            "generated-media-test",
            "raw/generated/mock/generated-media-test.mp4",
            "office",
        );
        crate::generated_media::registry::Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record)
            .unwrap();

        let err = UseGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "use_generated_media".into(),
                    args: serde_json::json!({
                        "job_id": "generated-media-test",
                        "anchor": { "transcript_snippet": "safe\"\n*** Delete Clip" },
                        "duration_s": 3.0
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("cannot contain newlines"));
    }

    #[tokio::test]
    async fn transcript_anchor_escapes_quotes() {
        let dir = tempfile::tempdir().unwrap();
        let record = crate::generated_media::registry::GeneratedMediaRecord::mock_succeeded(
            "generated-media-test",
            "raw/generated/mock/generated-media-test.mp4",
            "office",
        );
        crate::generated_media::registry::Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record)
            .unwrap();

        let output = UseGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "use_generated_media".into(),
                    args: serde_json::json!({
                        "job_id": "generated-media-test",
                        "anchor": { "transcript_snippet": "the \"quoted\" part" },
                        "duration_s": 3.0
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert!(
            body["edl_fragment"]
                .as_str()
                .unwrap()
                .contains("transcript_snippet=\"the \\\"quoted\\\" part\"")
        );
    }

    #[tokio::test]
    async fn failed_record_is_not_usable() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = crate::generated_media::registry::GeneratedMediaRecord::mock_succeeded(
            "generated-media-test",
            "raw/generated/mock/generated-media-test.mp4",
            "office",
        );
        record.state = GeneratedMediaState::Failed;
        record.failure_message = Some("provider failed".into());
        crate::generated_media::registry::Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record)
            .unwrap();

        let err = UseGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "use_generated_media".into(),
                    args: serde_json::json!({
                        "job_id": "generated-media-test",
                        "anchor": { "transcript_snippet": "the office" },
                        "duration_s": 3.0
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("not succeeded"));
    }

    #[tokio::test]
    async fn succeeded_record_without_video_output_is_not_usable() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = crate::generated_media::registry::GeneratedMediaRecord::mock_succeeded(
            "generated-media-test",
            "raw/generated/mock/generated-media-test.mp4",
            "office",
        );
        record.output_paths.clear();
        crate::generated_media::registry::Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record)
            .unwrap();

        let err = UseGeneratedMediaTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "use_generated_media".into(),
                    args: serde_json::json!({
                        "job_id": "generated-media-test",
                        "anchor": { "transcript_snippet": "the office" },
                        "duration_s": 3.0
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("has no output video path"));
    }
}
