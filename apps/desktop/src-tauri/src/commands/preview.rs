//! GPU transition preview command.
//!
//! Renders one blended RGBA frame at a given progress through a
//! transition window, returning a `data:image/png;base64,…` URL the
//! webview drops straight into an `<img src>`. Shares the same
//! [`montage_render_gpu::GpuTransitionRenderer`] code path as the
//! offline export composer, so what the user sees while scrubbing
//! matches what the final render produces.
//!
//! The shader expensive part — wgpu adapter/device/pipeline init —
//! runs once per shader and lives in
//! [`crate::state::MontageState::gpu_preview_renderers`]. The cheap
//! per-frame path is: pull one RGBA frame from each side with
//! [`montage_render::FrameProvider`], hand them to the cached
//! renderer, PNG-encode the result, base64-stringify.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use montage_proto::transitions::{builtin_transition_composition, resolve_composition_gpu_shader};
use montage_render::FrameProvider;
use montage_render_gpu::{
    FrameParams, GpuError, GpuTransitionRenderer, TransitionShader, encode_rgba_to_png,
};
use serde::Deserialize;
use tauri::State;

use crate::state::MontageState;

/// Arguments for [`render_transition_preview_frame`].
#[derive(Debug, Deserialize)]
pub struct RenderPreviewArgs {
    /// Outgoing clip's proxy MP4 path (the same value `<video>` uses).
    pub from_proxy_path: String,
    /// Source-time on the outgoing clip at this preview frame.
    pub from_source_s: f64,
    /// Incoming clip's proxy MP4 path.
    pub to_proxy_path: String,
    /// Source-time on the incoming clip at this preview frame.
    pub to_source_s: f64,
    /// Stable montage transition id (e.g. `montage.match_dissolve`).
    pub transition_id: String,
    /// Transition progress in `[0.0, 1.0]`.
    pub progress: f64,
    /// Output frame width in pixels (typically the preview pane size).
    pub width: u32,
    /// Output frame height in pixels.
    pub height: u32,
}

/// Render one preview frame and return a `data:image/png;base64,…` URL.
///
/// Returns the empty string when the transition has no GPU shader
/// mapping; the frontend uses that as the signal to fall back to its
/// CSS-based overlay (or to a hard cut). Errors are stringified for
/// Tauri compatibility — the frontend logs them and falls back as if
/// the transition has no GPU shader.
#[tauri::command]
pub async fn render_transition_preview_frame(
    state: State<'_, MontageState>,
    args: RenderPreviewArgs,
) -> Result<String, String> {
    // Resolve the transition id to a GPU shader. Returns "" (empty
    // string) for transitions without a GPU mapping so the frontend
    // can drop to the CSS overlay without treating it as an error.
    let Some(shader) = resolve_shader_for_id(&args.transition_id) else {
        return Ok(String::new());
    };

    // Roughly-1-frame source windows on each side. A duration too
    // small can produce zero frames at low fps; 0.1s at 30fps is
    // safely ~3 frames, and we only consume the first.
    let from_path = PathBuf::from(&args.from_proxy_path);
    let to_path = PathBuf::from(&args.to_proxy_path);
    let preview_fps = 30u32;
    let window_s = 0.2_f64;

    let from_rgba = pull_one_rgba_frame(
        &from_path,
        args.from_source_s,
        window_s,
        args.width,
        args.height,
        preview_fps,
    )
    .await?;
    let to_rgba = pull_one_rgba_frame(
        &to_path,
        args.to_source_s,
        window_s,
        args.width,
        args.height,
        preview_fps,
    )
    .await?;

    let renderer = renderer_for(&state, shader).await?;
    let frame = renderer
        .render_frame(
            &from_rgba,
            &to_rgba,
            FrameParams {
                width: args.width,
                height: args.height,
                progress: args.progress.clamp(0.0, 1.0) as f32,
                extra_params: extra_params_for(&args.transition_id, args.progress),
            },
        )
        .map_err(|e| format!("gpu render: {e}"))?;
    let png = encode_rgba_to_png(&frame, args.width, args.height)
        .map_err(|e| format!("png encode: {e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok(format!("data:image/png;base64,{encoded}"))
}

async fn pull_one_rgba_frame(
    source: &std::path::Path,
    source_start_s: f64,
    window_s: f64,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<Vec<u8>, String> {
    let mut provider = FrameProvider::open(
        source,
        source_start_s.max(0.0),
        window_s,
        width,
        height,
        fps,
    )
    .await
    .map_err(|e| format!("frame provider open: {e}"))?;
    let frame = match provider.next_frame().await {
        Ok(Some(bytes)) => bytes.to_vec(),
        Ok(None) => return Err("frame provider yielded no frames at requested time".into()),
        Err(e) => return Err(format!("frame provider read: {e}")),
    };
    // `provider.close()` kills the FFmpeg subprocess; we only needed
    // the first frame, so the early-close path is the expected one.
    let _ = provider.close().await;
    Ok(frame)
}

async fn renderer_for(
    state: &State<'_, MontageState>,
    shader: TransitionShader,
) -> Result<Arc<GpuTransitionRenderer>, String> {
    let mut cache = state.gpu_preview_renderers.lock().await;
    if let Some(existing) = cache.get(&shader) {
        return Ok(existing.clone());
    }
    // Drop the lock before the (potentially slow) wgpu init? It's
    // already held but `GpuTransitionRenderer::new` is blocking
    // (pollster-wrapped), so other callers waiting on the cache lock
    // would be queued either way. Holding the lock here also
    // guarantees only one wgpu init per shader — concurrent first
    // calls don't race to spawn two devices.
    let renderer = GpuTransitionRenderer::new(shader).map_err(|e| match e {
        GpuError::NoAdapter(msg) => format!("no GPU adapter: {msg}"),
        GpuError::NoDevice(msg) => format!("GPU device init failed: {msg}"),
        other => format!("gpu init: {other}"),
    })?;
    let arc = Arc::new(renderer);
    cache.insert(shader, arc.clone());
    Ok(arc)
}

/// Map an montage transition id to its GPU shader if the composition
/// resolves to one. Mirrors what the planner does at render time, so
/// preview and export agree.
fn resolve_shader_for_id(id: &str) -> Option<TransitionShader> {
    let composition = builtin_transition_composition(id)?;
    let shader_id = resolve_composition_gpu_shader(&composition)?;
    TransitionShader::from_proto_id(shader_id)
}

/// Map an montage transition id to its primary `extra_params[0]` value
/// at `progress`. Built-in compositions like `montage.match_dissolve`
/// embed a `ParamCurve` on a Blur primitive; the shader reads it from
/// the uniform. For shaders that don't consume params, this returns
/// zeros, which is harmless.
fn extra_params_for(id: &str, progress: f64) -> [f32; 4] {
    use montage_proto::transitions::{TransitionPrimitiveOp, luma_mask_kind_slot};
    let mut params = [0.0f32; 4];
    let Some(composition) = builtin_transition_composition(id) else {
        return params;
    };
    for primitive in composition.primitives {
        // The primitive's start/end window shapes how we project the
        // outer progress (0..1 over the whole transition) onto the
        // primitive's own curve (0..1 over its lifetime). Outside the
        // window the primitive doesn't contribute.
        let span = primitive.end - primitive.start;
        if span <= 0.0 {
            continue;
        }
        if progress < primitive.start || progress > primitive.end {
            continue;
        }
        let local = ((progress - primitive.start) / span).clamp(0.0, 1.0);
        match primitive.op {
            TransitionPrimitiveOp::Blur { amount, .. } => {
                params[0] = amount.evaluate(local) as f32;
                return params;
            }
            TransitionPrimitiveOp::Wipe { softness, .. } => {
                params[0] = softness.evaluate(local) as f32;
                return params;
            }
            TransitionPrimitiveOp::Zoom { scale } => {
                params[0] = scale.evaluate(local) as f32;
                return params;
            }
            TransitionPrimitiveOp::LumaMask {
                mask_kind,
                softness,
            } => {
                // params.x = integer kind slot (cast back in WGSL),
                // params.y = current softness from the curve.
                params[0] = luma_mask_kind_slot(&mask_kind).unwrap_or(0) as f32;
                params[1] = softness.evaluate(local) as f32;
                return params;
            }
            TransitionPrimitiveOp::LightLeak { intensity, warmth } => {
                params[0] = intensity.evaluate(local) as f32;
                params[1] = warmth as f32;
                return params;
            }
            TransitionPrimitiveOp::SwirlVortex { strength } => {
                params[0] = strength.evaluate(local) as f32;
                return params;
            }
            TransitionPrimitiveOp::CinematicPan {
                direction,
                intensity,
            } => {
                // params.x = horizontal direction sign (-1 left,
                // +1 right). params.y = current intensity.
                params[0] = match direction.as_str() {
                    "right" => 1.0,
                    "left" => -1.0,
                    _ => 0.0,
                };
                params[1] = intensity.evaluate(local) as f32;
                return params;
            }
            _ => {}
        }
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_dissolve_resolves_to_blur_shader() {
        // The Phase 4 keyframed-Blur demonstrator is the first
        // builtin with a GPU shader mapping. Locking it in here keeps
        // the preview frontend's hardcoded `GPU_ROUTED_TRANSITION_IDS`
        // honest — if the backend resolution ever stops returning
        // Blur for match_dissolve, this test catches it.
        let shader = resolve_shader_for_id("montage.match_dissolve");
        assert_eq!(shader, Some(TransitionShader::Blur));
    }

    #[test]
    fn cross_dissolve_resolves_to_cross_dissolve_shader() {
        // cross_dissolve maps to opacity-only, which the proto layer
        // returns as the cross_dissolve shader id. Frontend currently
        // falls back to CSS for it (CSS handles plain opacity well),
        // but the resolution should still succeed.
        let shader = resolve_shader_for_id("montage.cross_dissolve");
        assert_eq!(shader, Some(TransitionShader::CrossDissolve));
    }

    #[test]
    fn unknown_transition_id_returns_none() {
        let shader = resolve_shader_for_id("montage.does_not_exist");
        assert!(shader.is_none());
    }

    #[test]
    fn match_dissolve_extra_params_evaluate_keyframed_blur() {
        // The match_dissolve composition embeds a 4-keyframe Blur
        // amount: 0 → 0.18 (at t=0.45) → 0.18 (at t=0.55) → 0. The
        // primitive's own window is [0.18, 0.82] of the transition.
        // At outer progress = 0.5 we're at primitive-local progress
        // ≈ (0.5 - 0.18) / (0.82 - 0.18) ≈ 0.5, where the curve
        // sits on its plateau at ~0.18.
        let params = extra_params_for("montage.match_dissolve", 0.5);
        assert!(
            (params[0] - 0.18).abs() < 0.02,
            "expected ~0.18 at mid-progress, got {}",
            params[0]
        );
        // At outer progress = 0 we're before the primitive's window,
        // so all zeros.
        let params = extra_params_for("montage.match_dissolve", 0.0);
        assert_eq!(params, [0.0, 0.0, 0.0, 0.0]);
    }
}
