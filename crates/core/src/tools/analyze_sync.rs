//! `analyze_sync` tool — waveform-align external mics and cameras.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// The `analyze_sync` tool.
pub struct AnalyzeSyncTool;

#[derive(Debug, Deserialize)]
struct AnalyzeSyncArgs {
    /// Reference asset, project-relative. Defaults to the first raw asset with audio.
    #[serde(default)]
    reference_asset: Option<String>,
    /// Candidate assets to align. Defaults to every other raw asset with audio.
    #[serde(default)]
    candidate_assets: Option<Vec<String>>,
    /// Waveform bucket count. Defaults to 4096.
    #[serde(default)]
    bucket_count: Option<usize>,
}

#[async_trait]
impl ToolHandler for AnalyzeSyncTool {
    fn name(&self) -> &'static str {
        "analyze_sync"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "analyze_sync".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "reference_asset": { "type": "string", "description": "Project-relative reference asset. Defaults to the first raw audio/video asset with audio." },
                    "candidate_assets": { "type": "array", "items": { "type": "string" }, "description": "Project-relative assets to align. Defaults to every other raw asset with audio." },
                    "bucket_count": { "type": "integer", "minimum": 512, "maximum": 32768, "description": "Waveform buckets used for correlation. Default 4096." }
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
        let args: AnalyzeSyncArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "analyze_sync: invalid args ({e}). All fields are optional."
            ))
        })?;
        let bucket_count = args.bucket_count.unwrap_or(4096).clamp(512, 32768);
        let all_assets = raw_assets(&ctx.project_root);
        if all_assets.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "analyze_sync: no files found under raw/".into(),
            ));
        }
        let reference_asset = args
            .reference_asset
            .or_else(|| all_assets.first().cloned())
            .ok_or_else(|| {
                FunctionCallError::RespondToModel("analyze_sync: no reference asset".into())
            })?;
        validate_rel("reference_asset", &reference_asset)?;
        let reference_path = ctx.project_root.join(&reference_asset);
        let reference_probe = awidat_render::probe_media(&reference_path)
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "analyze_sync: ffprobe reference failed: {e}"
                ))
            })?;
        if !reference_probe.has_audio {
            return Err(FunctionCallError::RespondToModel(format!(
                "analyze_sync: reference asset {reference_asset:?} has no audio stream"
            )));
        }
        let reference_wave = awidat_render::generate_waveform(
            &reference_path,
            bucket_count,
            CancellationToken::new(),
        )
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "analyze_sync: waveform reference failed: {e}"
            ))
        })?;
        let reference_duration_s = reference_probe.duration_s.unwrap_or(bucket_count as f64);

        let candidates = args.candidate_assets.unwrap_or_else(|| {
            all_assets
                .into_iter()
                .filter(|a| a != &reference_asset)
                .collect()
        });
        let mut proposals = Vec::new();
        let mut blockers = Vec::new();
        for candidate in candidates {
            if let Err(e) = validate_rel("candidate_asset", &candidate) {
                blockers.push(serde_json::json!({ "asset": candidate, "reason": e.to_string() }));
                continue;
            }
            let candidate_path = ctx.project_root.join(&candidate);
            let probe = match awidat_render::probe_media(&candidate_path).await {
                Ok(p) => p,
                Err(e) => {
                    blockers.push(serde_json::json!({ "asset": candidate, "reason": format!("ffprobe failed: {e}") }));
                    continue;
                }
            };
            if !probe.has_audio {
                blockers
                    .push(serde_json::json!({ "asset": candidate, "reason": "no audio stream" }));
                continue;
            }
            let wave = match awidat_render::generate_waveform(
                &candidate_path,
                bucket_count,
                CancellationToken::new(),
            )
            .await
            {
                Ok(w) if !w.is_empty() => w,
                Ok(_) => {
                    blockers.push(
                        serde_json::json!({ "asset": candidate, "reason": "empty audio waveform" }),
                    );
                    continue;
                }
                Err(e) => {
                    blockers.push(serde_json::json!({ "asset": candidate, "reason": format!("waveform failed: {e}") }));
                    continue;
                }
            };
            let offset = best_offset(&reference_wave, &wave, reference_duration_s);
            let drift = drift_estimate(&reference_wave, &wave, reference_duration_s);
            let low_confidence = offset.confidence < 0.35;
            let large_drift = drift.abs() > 0.50;
            let speed_factor = if !large_drift && drift.abs() > 0.02 {
                probe
                    .duration_s
                    .filter(|d| *d > 1.0)
                    .map(|d| (d / (d + drift)).clamp(0.995, 1.005))
            } else {
                None
            };
            let sync_group_id = format!("sync-{}", short_slug(&reference_asset, &candidate));
            proposals.push(serde_json::json!({
                "reference_asset": reference_asset,
                "candidate_asset": candidate,
                "offset_s": offset.offset_s,
                "confidence": offset.confidence,
                "sync_group_id": sync_group_id,
                "speed_factor": speed_factor,
                "manual_offset_required": low_confidence,
                "blocker": if large_drift { Some(format!("detected unstable drift of {drift:.3}s; split or manually sync this source")) } else { None },
                "edl": format!(
                    "*** Begin EDL\n*** Set Sync Group\n@@ anchor: clip_uuid=<candidate clip uuid>\n+ sync_group_id: {sync_group_id}\n+ offset_s: {:.6}\n{}+ confidence: {:.3}\n*** End EDL",
                    offset.offset_s,
                    speed_factor.map(|v| format!("+ speed_factor: {v:.9}\n")).unwrap_or_default(),
                    offset.confidence,
                )
            }));
        }

        let body = serde_json::json!({
            "reference_asset": reference_asset,
            "bucket_count": bucket_count,
            "proposals": proposals,
            "blockers": blockers,
            "notes": [
                "Positive offset_s means place the candidate later relative to the reference.",
                "Low confidence proposals should be reviewed manually before applying Set Sync Group."
            ]
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

#[derive(Debug)]
struct OffsetResult {
    offset_s: f64,
    confidence: f64,
}

fn best_offset(reference: &[f32], candidate: &[f32], reference_duration_s: f64) -> OffsetResult {
    let n = reference.len().min(candidate.len());
    if n < 4 {
        return OffsetResult {
            offset_s: 0.0,
            confidence: 0.0,
        };
    }
    let max_lag = (n / 3).max(1) as isize;
    let mut best_lag = 0isize;
    let mut best = f64::MIN;
    for lag in -max_lag..=max_lag {
        let mut dot = 0.0;
        let mut ar = 0.0;
        let mut br = 0.0;
        let mut count = 0usize;
        for (i, &a) in reference.iter().take(n).enumerate() {
            let j = i as isize + lag;
            if !(0..n as isize).contains(&j) {
                continue;
            }
            let b = candidate[j as usize] as f64;
            let a = a as f64;
            dot += a * b;
            ar += a * a;
            br += b * b;
            count += 1;
        }
        if count < n / 8 || ar <= 1e-12 || br <= 1e-12 {
            continue;
        }
        let score = dot / (ar.sqrt() * br.sqrt());
        if score > best {
            best = score;
            best_lag = lag;
        }
    }
    let bucket_s = reference_duration_s / n as f64;
    OffsetResult {
        offset_s: -(best_lag as f64) * bucket_s,
        confidence: best.clamp(0.0, 1.0),
    }
}

fn drift_estimate(reference: &[f32], candidate: &[f32], duration_s: f64) -> f64 {
    let n = reference.len().min(candidate.len());
    if n < 16 {
        return 0.0;
    }
    let mid = n / 2;
    let first = best_offset(&reference[..mid], &candidate[..mid], duration_s / 2.0);
    let second = best_offset(&reference[mid..n], &candidate[mid..n], duration_s / 2.0);
    second.offset_s - first.offset_s
}

fn raw_assets(project_root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk_raw(project_root, &project_root.join("raw"), &mut out);
    out.sort();
    out
}

fn walk_raw(project_root: &Path, here: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(here) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_raw(project_root, &path, out);
        } else if path.is_file()
            && let Ok(rel) = path.strip_prefix(project_root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn validate_rel(label: &str, rel: &str) -> Result<(), FunctionCallError> {
    if rel.contains("..") || PathBuf::from(rel).is_absolute() {
        return Err(FunctionCallError::RespondToModel(format!(
            "analyze_sync: {label} must be project-relative and must not contain '..'"
        )));
    }
    Ok(())
}

fn short_slug(a: &str, b: &str) -> String {
    let mut s = format!("{a}-{b}")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(18)
        .collect::<String>();
    if s.is_empty() {
        s.push_str("group");
    }
    s
}

const DESCRIPTION: &str = "\
Analyze waveform sync between a reference asset and candidate camera/mic \
assets. Uses FFmpeg waveform extraction and cross-correlation, reports \
offset_s, confidence, optional speed_factor for small stable drift, and \
an EDL Set Sync Group snippet. Low-confidence results are surfaced as \
manual offset proposals instead of silently committing.";

#[cfg(test)]
mod tests {
    use super::*;

    /// A distinctive non-periodic waveform: zeros with a single bump, so the
    /// cross-correlation peak is unambiguous.
    fn bump_wave(n: usize, at: usize) -> Vec<f32> {
        let mut w = vec![0f32; n];
        for (k, v) in [1.0, 2.0, 3.0, 2.0, 1.0].iter().enumerate() {
            w[at + k] = *v;
        }
        w
    }

    #[test]
    fn best_offset_recovers_known_lag() {
        // Candidate bump is shifted +3 buckets relative to the reference.
        // With duration_s == n, bucket_s == 1, so offset_s ≈ -3.
        let reference = bump_wave(64, 18);
        let candidate = bump_wave(64, 21);
        let r = best_offset(&reference, &candidate, 64.0);
        assert!(
            (r.offset_s - (-3.0)).abs() < 1e-6,
            "expected offset ≈ -3, got {}",
            r.offset_s
        );
        assert!(r.confidence > 0.9, "aligned bumps should correlate near 1");
    }

    #[test]
    fn best_offset_reports_zero_confidence_for_silent_candidate() {
        // A flat/silent candidate has no energy to correlate against, so every
        // lag is rejected and confidence collapses to 0 — the signal that
        // gates `manual_offset_required` in the tool.
        let reference = bump_wave(64, 18);
        let silent = vec![0f32; 64];
        let r = best_offset(&reference, &silent, 64.0);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn best_offset_handles_degenerate_short_input() {
        let r = best_offset(&[1.0, 2.0], &[1.0, 2.0], 2.0);
        assert_eq!(r.offset_s, 0.0);
        assert_eq!(r.confidence, 0.0);
    }

    #[test]
    fn drift_estimate_is_zero_for_aligned_signals() {
        // Identical halves → both half-windows align at the same offset → no
        // drift, so no spurious speed_factor is proposed.
        let mut wave = vec![0f32; 64];
        for (a, b) in [(8, 1.0), (20, 3.0), (40, 2.0), (52, 1.5)] {
            wave[a] = b;
        }
        let drift = drift_estimate(&wave, &wave, 64.0);
        assert!(drift.abs() < 1e-6, "aligned signal should not drift");
    }
}
