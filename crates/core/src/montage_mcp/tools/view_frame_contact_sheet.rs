//! Bounded source-frame contact sheet returned as one native MCP image.

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use image::{DynamicImage, Rgba, RgbaImage, imageops};
use rmcp::model::{Annotated, CallToolResult, Content, RawContent, RawImageContent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

use super::view_frame::{self, ViewFrameArgs};

const MAX_FRAMES: usize = 12;
const CELL_SIZE: u32 = 384;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ViewFrameContactSheetArgs {
    /// Project-relative asset id or an absolute path under the project.
    pub asset: String,
    /// Ranked source times to show, in tile order. Between 2 and 12 values.
    pub times_s: Vec<f64>,
    /// Grid columns. Defaults to at most four.
    #[serde(default)]
    pub columns: Option<u32>,
    /// Optional clip name when the sheet should include its color effects.
    #[serde(default)]
    pub clip: Option<String>,
}

pub async fn run(
    args: ViewFrameContactSheetArgs,
    ctx: McpToolCtx,
) -> Result<CallToolResult, String> {
    if !(2..=MAX_FRAMES).contains(&args.times_s.len()) {
        return Err(format!(
            "view_frame_contact_sheet: times_s must contain 2..={MAX_FRAMES} entries"
        ));
    }
    if args
        .times_s
        .iter()
        .any(|time| !time.is_finite() || *time < 0.0)
    {
        return Err("view_frame_contact_sheet: every time must be finite and >= 0".into());
    }
    let columns = args.columns.unwrap_or(4).clamp(1, 4);
    let mut frames = Vec::with_capacity(args.times_s.len());
    for time in &args.times_s {
        let result = view_frame::run(
            ViewFrameArgs {
                asset: args.asset.clone(),
                t_s: *time,
                detail: Some("preview".into()),
                format: Some("png".into()),
                clip: args.clip.clone(),
            },
            ctx.clone(),
        )
        .await?;
        let image = result
            .content
            .iter()
            .find_map(|content| match &content.raw {
                RawContent::Image(image) => Some(image),
                _ => None,
            })
            .ok_or_else(|| "view_frame_contact_sheet: view_frame returned no image".to_string())?;
        let bytes = B64
            .decode(&image.data)
            .map_err(|error| format!("view_frame_contact_sheet: decode frame: {error}"))?;
        frames.push(
            image::load_from_memory(&bytes)
                .map_err(|error| format!("view_frame_contact_sheet: decode image: {error}"))?,
        );
    }

    let sheet = compose_contact_sheet(&frames, columns);
    let mut png = Cursor::new(Vec::new());
    sheet
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|error| format!("view_frame_contact_sheet: encode sheet: {error}"))?;
    let key = args
        .times_s
        .iter()
        .enumerate()
        .map(|(index, time)| format!("{}={time:.3}s", index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(contact_sheet_result(
        format!(
            "source-frame contact sheet for {}; tile order: {key}; {}x{}",
            args.asset,
            sheet.width(),
            sheet.height()
        ),
        png.get_ref(),
    ))
}

fn compose_contact_sheet(frames: &[DynamicImage], columns: u32) -> DynamicImage {
    let columns = columns.clamp(1, 4);
    let rows = (frames.len() as u32).div_ceil(columns);
    let mut canvas = RgbaImage::from_pixel(
        columns * CELL_SIZE,
        rows * CELL_SIZE,
        Rgba([16, 17, 22, 255]),
    );
    for (index, frame) in frames.iter().enumerate() {
        let thumb = frame.thumbnail(CELL_SIZE, CELL_SIZE).to_rgba8();
        let col = index as u32 % columns;
        let row = index as u32 / columns;
        let x = col * CELL_SIZE + (CELL_SIZE - thumb.width()) / 2;
        let y = row * CELL_SIZE + (CELL_SIZE - thumb.height()) / 2;
        imageops::overlay(&mut canvas, &thumb, i64::from(x), i64::from(y));
    }
    DynamicImage::ImageRgba8(canvas)
}

fn contact_sheet_result(summary: String, bytes: &[u8]) -> CallToolResult {
    CallToolResult::success(vec![
        Content::text(summary),
        Annotated::new(
            RawContent::Image(RawImageContent {
                data: B64.encode(bytes),
                mime_type: "image/png".into(),
                meta: None,
            }),
            None,
        ),
    ])
}

pub const DESCRIPTION: &str = "Create a bounded grid from 2 to 12 ranked source-frame times and return it as one native image. Use the contact sheet for inexpensive visual selection, then call view_frame with detail='original' only for tiles that need closer inspection.";

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    #[test]
    fn contact_sheet_is_bounded_and_uses_native_image_content() {
        let frames = vec![
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(640, 360, Rgba([255, 0, 0, 255]))),
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(360, 640, Rgba([0, 0, 255, 255]))),
        ];
        let sheet = compose_contact_sheet(&frames, 2);
        assert_eq!(sheet.dimensions(), (768, 384));

        let result = contact_sheet_result("tiles 1-2".into(), &[0, 1, 2, 3]);
        assert!(matches!(result.content[0].raw, RawContent::Text(_)));
        let RawContent::Image(image) = &result.content[1].raw else {
            panic!("second block should be native image content");
        };
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.data, "AAECAw==");
    }
}
