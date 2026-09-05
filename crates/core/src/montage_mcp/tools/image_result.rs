//! Shared native-image responses for visual inspection tools.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use rmcp::model::{Annotated, CallToolResult, Content, Meta, RawContent, RawImageContent};

#[derive(Debug, Clone, Copy)]
pub(super) enum Detail {
    Preview,
    Original,
}

pub(super) fn image_tool_result(
    summary: String,
    bytes: &[u8],
    mime_type: &str,
    detail: Detail,
) -> CallToolResult {
    let meta = match detail {
        Detail::Preview => None,
        Detail::Original => {
            let mut meta = Meta::new();
            meta.insert(
                "codex/imageDetail".to_string(),
                serde_json::json!("original"),
            );
            Some(meta)
        }
    };
    let image = Annotated::new(
        RawContent::Image(RawImageContent {
            data: B64.encode(bytes),
            mime_type: mime_type.to_string(),
            meta,
        }),
        None,
    );
    CallToolResult::success(vec![Content::text(summary), image])
}

#[cfg(test)]
mod image_result_tests {
    use super::*;
    use rmcp::model::RawContent;

    #[test]
    fn frame_result_uses_native_image_content_without_base64_text() {
        let result = image_tool_result(
            "frame 1.250s of raw/take.mov (image/png, 4 bytes)".to_string(),
            &[0, 1, 2, 3],
            "image/png",
            Detail::Preview,
        );

        assert_eq!(result.content.len(), 2);
        let RawContent::Text(text) = &result.content[0].raw else {
            panic!("first block should be text provenance");
        };
        assert!(text.text.contains("frame 1.250s"));
        assert!(!text.text.contains("AAECAw=="));

        let RawContent::Image(image) = &result.content[1].raw else {
            panic!("second block should be native image content");
        };
        assert_eq!(image.data, "AAECAw==");
        assert_eq!(image.mime_type, "image/png");
        assert!(image.meta.is_none());
    }

    #[test]
    fn original_frame_requests_original_codex_image_detail() {
        let result = image_tool_result(
            "original frame".to_string(),
            &[0, 1, 2, 3],
            "image/jpeg",
            Detail::Original,
        );
        let RawContent::Image(image) = &result.content[1].raw else {
            panic!("second block should be native image content");
        };

        assert_eq!(
            image
                .meta
                .as_ref()
                .and_then(|meta| meta.get("codex/imageDetail")),
            Some(&serde_json::json!("original"))
        );
    }
}
