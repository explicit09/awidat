//! Integration test for the R4 fix: `transcode_proxy` must not hang
//! forever when the underlying ffmpeg process never exits (e.g. a
//! caller hands in a fresh, unowned `CancellationToken` that nobody
//! can fire). This lives in `tests/` (its own process) rather than
//! `src/ffmpeg.rs`'s `#[cfg(test)]` module because `ffmpeg_path()`
//! caches its resolution in a `OnceLock` for the life of the process —
//! setting `MONTAGE_FFMPEG` here is safe only because nothing in this
//! binary has resolved the real ffmpeg path yet.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Write a fake `ffmpeg` shell script at `dir/ffmpeg` that ignores all
/// arguments and sleeps far longer than any timeout this test sets,
/// simulating a stuck/runaway encode.
fn write_hanging_ffmpeg(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("ffmpeg");
    std::fs::write(
        &path,
        "#!/bin/sh\nsleep 300\n", // way past the 1-2s test timeout
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[tokio::test]
async fn transcode_proxy_times_out_and_removes_pending_file() {
    let bin_dir = tempfile::tempdir().unwrap();
    let fake_ffmpeg = write_hanging_ffmpeg(bin_dir.path());

    // SAFETY: this is the only test in this process, and it runs before
    // any call to `ffmpeg_path()`/`ffprobe_path()` (both cache in a
    // `OnceLock` on first use), so setting env here deterministically
    // wins the race.
    unsafe {
        std::env::set_var("MONTAGE_FFMPEG", &fake_ffmpeg);
        std::env::set_var("MONTAGE_PROXY_TIMEOUT_SECS", "1");
    }

    let project = tempfile::tempdir().unwrap();
    let asset_path = project.path().join("source.mov");
    // Content doesn't matter — ffmpeg is faked and never reads it. Give
    // it a `.mov` extension so the remux fast-path is considered (also
    // faked; the hang looks identical either way).
    std::fs::write(&asset_path, b"not real media").unwrap();
    let pending_path = project.path().join("proxy.mp4.pending");

    let started = std::time::Instant::now();
    let result =
        montage_render::transcode_proxy(&asset_path, &pending_path, None, CancellationToken::new())
            .await;
    let elapsed = started.elapsed();

    assert!(
        matches!(result, Err(montage_render::FfmpegError::Timeout(_))),
        "expected FfmpegError::Timeout, got {result:?}"
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("timed out"),
        "error should describe the timeout: {message}"
    );

    assert!(
        !pending_path.exists(),
        ".pending artifact must be removed on timeout so proxy_status_for \
         never reports a permanently-stuck Pending proxy (R11)",
    );

    // Bounded well under the fake ffmpeg's 300s sleep — proves we didn't
    // just get lucky waiting for the child naturally.
    assert!(
        elapsed < Duration::from_secs(30),
        "timeout should fire promptly, took {elapsed:?}",
    );
}
