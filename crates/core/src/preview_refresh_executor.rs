//! Production [`PreviewRefreshExecutor`] backed by ffmpeg.
//!
//! Dispatches each `PreviewCacheRefreshTask` to the matching
//! `montage_render` helper by `artifact_kind`:
//! - `proxy`     → `montage_render::transcode_proxy`
//! - `thumbnails`→ `montage_render::generate_thumbnails`
//! - `waveform`  → `montage_render::generate_waveform` + JSON sidecar write
//!
//! The waveform sidecar shape (`{ "buckets": [f32, ...], "duration_s": f64 }`)
//! matches the desktop's `WaveformSidecar` schema so existing readers do not
//! change (`duration_s` is optional/defaulted for back-compat).

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::preview_cache::{PreviewCacheRefreshTask, PreviewRefreshError, PreviewRefreshExecutor};

/// Default waveform bucket count, mirroring the desktop's
/// `commands/waveform.rs::DEFAULT_BUCKET_COUNT`. ~4 KB JSON per asset.
pub const DEFAULT_WAVEFORM_BUCKET_COUNT: usize = 2048;

/// Production executor that resolves source media via `project_root + asset_id`
/// and dispatches to the appropriate `montage_render` helper for the task's
/// `artifact_kind`. Unknown kinds surface as an executor error so callers
/// see the misconfiguration immediately instead of silently skipping work.
pub struct FfmpegPreviewRefreshExecutor {
    project_root: PathBuf,
    waveform_bucket_count: usize,
    cancel: CancellationToken,
}

impl FfmpegPreviewRefreshExecutor {
    /// Build an executor rooted at `project_root` with the default waveform
    /// bucket count and a fresh cancellation token.
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            waveform_bucket_count: DEFAULT_WAVEFORM_BUCKET_COUNT,
            cancel: CancellationToken::new(),
        }
    }

    /// Override the waveform bucket count (e.g. for smaller test fixtures).
    pub fn with_waveform_bucket_count(mut self, bucket_count: usize) -> Self {
        self.waveform_bucket_count = bucket_count;
        self
    }

    /// Supply a cancellation token shared with the caller's job-control flow.
    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    fn source_path(&self, asset_id: &str) -> PathBuf {
        self.project_root.join(asset_id)
    }
}

#[derive(Debug, Serialize)]
struct WaveformSidecar<'a> {
    buckets: &'a [f32],
    /// Audio duration the buckets span, in seconds. Lets the timeline
    /// slice the buckets correctly for trimmed clips.
    duration_s: f64,
}

#[async_trait::async_trait]
impl PreviewRefreshExecutor for FfmpegPreviewRefreshExecutor {
    async fn execute(&self, task: &PreviewCacheRefreshTask) -> Result<(), PreviewRefreshError> {
        let source = self.source_path(&task.asset_id);
        let artifact = Path::new(&task.artifact_path);
        match task.artifact_kind.as_str() {
            "proxy" => {
                montage_render::transcode_proxy(&source, artifact, None, self.cancel.clone())
                    .await
                    .map_err(|e| PreviewRefreshError::Executor {
                        task_id: task.task_id.clone(),
                        message: format!("transcode_proxy: {e}"),
                    })
            }
            "thumbnails" => {
                montage_render::generate_thumbnails(&source, artifact, self.cancel.clone())
                    .await
                    .map_err(|e| PreviewRefreshError::Executor {
                        task_id: task.task_id.clone(),
                        message: format!("generate_thumbnails: {e}"),
                    })
            }
            "waveform" => {
                let wave = montage_render::generate_waveform(
                    &source,
                    self.waveform_bucket_count,
                    self.cancel.clone(),
                )
                .await
                .map_err(|e| PreviewRefreshError::Executor {
                    task_id: task.task_id.clone(),
                    message: format!("generate_waveform: {e}"),
                })?;
                if let Some(parent) = artifact.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let body = serde_json::to_vec(&WaveformSidecar {
                    buckets: &wave.buckets,
                    duration_s: wave.duration_s,
                })
                .map_err(|e| PreviewRefreshError::Executor {
                    task_id: task.task_id.clone(),
                    message: format!("serialize waveform sidecar: {e}"),
                })?;
                tokio::fs::write(artifact, body).await?;
                Ok(())
            }
            other => Err(PreviewRefreshError::Executor {
                task_id: task.task_id.clone(),
                message: format!("unknown artifact_kind: {other}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_cache::PreviewArtifactStatus;

    fn task_with_kind(kind: &str) -> PreviewCacheRefreshTask {
        PreviewCacheRefreshTask {
            task_id: format!("{kind}:test"),
            asset_id: "raw/missing.mp4".into(),
            artifact_path: "/tmp/never-written".into(),
            artifact_kind: kind.into(),
            status: PreviewArtifactStatus::Missing,
            estimated_weight: 1,
            reason: "test".into(),
        }
    }

    #[tokio::test]
    async fn execute_unknown_kind_surfaces_executor_error() {
        let dir = tempfile::tempdir().unwrap();
        let executor = FfmpegPreviewRefreshExecutor::new(dir.path().to_path_buf());
        let err = executor
            .execute(&task_with_kind("nonsense"))
            .await
            .expect_err("unknown kind should fail");
        match err {
            PreviewRefreshError::Executor { task_id, message } => {
                assert_eq!(task_id, "nonsense:test");
                assert!(message.contains("unknown artifact_kind"));
            }
            other => panic!("expected Executor error, got {other:?}"),
        }
    }
}
