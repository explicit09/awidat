# Caption frame-pixel scorer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace metadata/sidecar-derived caption rendered-output evidence in `verify_render` with a real frame-pixel scorer that decodes the rendered output and measures safe-area + occlusion per caption event, keeping libass-sidecar derivation as a named fallback.

**Architecture:** A new `caption_rendered_output_scorer` module in `awidat-core` parses Dialogue lines from ASS sidecars, computes per-event bboxes against PlayRes + style margins + safe-area profiles, and asks a `CaptionFrameSampler` trait for a grayscale frame at the event midpoint. The production sampler shells out to a new `extract_frame_raw_gray` helper in `awidat_render::ffmpeg` (raw `gray` rawvideo, no PNG roundtrip, no new image-decode dep). `verify_render_output` runs the scorer before its sync gate-builder and injects measured `caption_rendered_output_*` metadata; the existing `add_caption_rendered_output_gate` consumes the injected fields unchanged.

**Tech Stack:** Rust 2024, tokio async, existing `awidat_render::ffmpeg::Command` infra, existing ASS sidecar pipeline, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

Spec: `docs/superpowers/specs/2026-05-22-caption-frame-pixel-scorer-design.md`.

---

## File Structure

| File | Responsibility | Status |
|---|---|---|
| `crates/render/src/manifest.rs` | Add `libass_layout_sidecar_paths` to ASS sidecar metadata | Modify |
| `crates/render/src/ffmpeg.rs` | Add `extract_frame_raw_gray` helper | Modify |
| `crates/render/src/lib.rs` | Re-export `extract_frame_raw_gray` | Modify |
| `crates/core/src/caption_rendered_output_scorer.rs` | Scorer module: trait, types, parser, geometry, occlusion | Create |
| `crates/core/src/lib.rs` | Module declaration + public re-exports | Modify |
| `crates/core/src/tools/verify_render.rs` | Run scorer before sync gate code; inject metadata; new reason strings | Modify |
| `crates/core/src/capabilities.rs` | Update caption verification note language | Modify |
| `crates/core/src/capability_metadata.rs` | Mirror capabilities note language | Modify |
| `crates/core/tests/capability_manifest.rs` | Update expected note language | Modify |

---

## Task 1: Surface ASS sidecar paths in the render manifest

**Files:**
- Modify: `crates/render/src/manifest.rs:465-523` (`AssSidecarLayoutSummary`)
- Test: `crates/render/tests/ass_captions.rs` (extend existing module — no new file)

- [ ] **Step 1: Write the failing test**

Append to `crates/render/tests/ass_captions.rs`:

```rust
#[test]
fn ass_sidecar_layout_metadata_records_paths_in_argv_order() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("captions_a.ass");
    let b = dir.path().join("captions_b.ass");
    let body = "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,80,80,40,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,hello world\n";
    fs::write(&a, body).unwrap();
    fs::write(&b, body).unwrap();
    let argv = vec![
        "-vf".into(),
        format!("subtitles={}", a.display()),
        "-vf".into(),
        format!("subtitles={}", b.display()),
    ];
    let metadata = awidat_render::ass_sidecar_layout_metadata(&argv).unwrap();
    let joined = metadata.get("libass_layout_sidecar_paths").expect("paths key present");
    assert_eq!(joined, &format!("{},{}", a.display(), b.display()));
}
```

- [ ] **Step 2: Run test to verify it fails**

```
cargo test -p awidat-render --test ass_captions ass_sidecar_layout_metadata_records_paths_in_argv_order
```

Expected: FAIL — `libass_layout_sidecar_paths` key absent.

- [ ] **Step 3: Implement the metadata extension**

Modify `crates/render/src/manifest.rs`:

In `AssSidecarLayoutSummary` (line 465):
```rust
#[derive(Debug, Default)]
struct AssSidecarLayoutSummary {
    sidecar_count: usize,
    playres_values: std::collections::BTreeSet<String>,
    wrapped_sidecar_count: usize,
    safe_area_sidecar_count: usize,
    karaoke_sidecar_count: usize,
    sidecar_paths: Vec<String>,
}
```

In `ass_sidecar_layout_metadata` (line 450), change the loop to also record paths:
```rust
pub fn ass_sidecar_layout_metadata(
    argv: &[String],
) -> Result<std::collections::BTreeMap<String, String>, RenderManifestError> {
    let mut summary = AssSidecarLayoutSummary::default();
    for path in ffmpeg_subtitle_filter_paths(argv) {
        let contents =
            std::fs::read_to_string(&path).map_err(|source| RenderManifestError::Io {
                path: path.to_string_lossy().into_owned(),
                source,
            })?;
        summary.add_document(&contents);
        summary.sidecar_paths.push(path.to_string_lossy().into_owned());
    }
    Ok(summary.into_metadata())
}
```

In `into_metadata` (line 493), append the paths key after the existing inserts and before the final `metadata`:
```rust
metadata.insert(
    "libass_layout_sidecar_paths".into(),
    self.sidecar_paths.join(","),
);
metadata
```

- [ ] **Step 4: Run test to verify it passes**

```
cargo test -p awidat-render --test ass_captions ass_sidecar_layout_metadata_records_paths_in_argv_order
```

Expected: PASS.

- [ ] **Step 5: Re-run the existing render manifest tests to confirm no regression**

```
cargo test -p awidat-render --test ass_captions
cargo test -p awidat-render manifest
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/render/src/manifest.rs crates/render/tests/ass_captions.rs
git commit -m "Record ASS sidecar paths in render manifest metadata"
```

---

## Task 2: Add `extract_frame_raw_gray` to `awidat_render::ffmpeg`

**Files:**
- Modify: `crates/render/src/ffmpeg.rs` (append after the existing `extract_frame_*` helpers)
- Modify: `crates/render/src/lib.rs` (re-export)

No focused unit test — this helper invokes real ffmpeg and is covered indirectly by the scorer integration test in Task 5.

- [ ] **Step 1: Add the helper**

Append to `crates/render/src/ffmpeg.rs` (after `extract_frame_filtered`):

```rust
/// Extract a single grayscale frame at time `t_s` from `asset_path`,
/// returning `(width, height, luma_bytes)` where `luma_bytes.len() == width * height`.
///
/// Implementation:
/// `ffmpeg -ss <t_s> -i <asset> -frames:v 1 -f rawvideo -pix_fmt gray -`
///
/// Designed for downstream luma-plane analysis (caption rendered-output scorer).
pub async fn extract_frame_raw_gray(
    asset_path: &Path,
    t_s: f64,
) -> Result<(u32, u32, Vec<u8>), FfmpegError> {
    if !t_s.is_finite() || t_s < 0.0 {
        return Err(FfmpegError::BadTimestamp(t_s));
    }
    let bin = ffmpeg_path()?;

    // Probe dimensions first via ffprobe-equivalent: ask ffmpeg to print stream info.
    // We use `-vf showinfo` to a single frame and parse `s:WxH`. Simpler: ask for
    // dimensions via a separate `-f null` probe.
    let mut probe = Command::new(&bin);
    probe.arg("-hide_banner")
        .arg("-loglevel").arg("error")
        .arg("-i").arg(asset_path)
        .arg("-frames:v").arg("1")
        .arg("-vf").arg("showinfo")
        .arg("-f").arg("null").arg("-");
    probe.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    let probe_output = probe.output().await.map_err(|e| FfmpegError::Spawn { path: bin.clone(), source: e })?;
    let stderr = String::from_utf8_lossy(&probe_output.stderr);
    let (width, height) = parse_showinfo_dimensions(&stderr).ok_or_else(|| {
        FfmpegError::Io(std::io::Error::other("ffmpeg showinfo did not report dimensions"))
    })?;

    let mut cmd = Command::new(&bin);
    cmd.arg("-loglevel").arg("error")
        .arg("-y")
        .arg("-ss").arg(format!("{t_s}"))
        .arg("-i").arg(asset_path)
        .arg("-frames:v").arg("1")
        .arg("-f").arg("rawvideo")
        .arg("-pix_fmt").arg("gray")
        .arg("-");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = cmd.output().await.map_err(|e| FfmpegError::Spawn { path: bin.clone(), source: e })?;
    if !output.status.success() {
        let stderr_tail = tail_string(&output.stderr, STDERR_TAIL_BYTES);
        return Err(FfmpegError::NonZero { code: output.status.code().unwrap_or(-1), stderr_tail });
    }
    let expected = (width as usize) * (height as usize);
    if output.stdout.len() < expected {
        return Err(FfmpegError::Io(std::io::Error::other(format!(
            "ffmpeg rawvideo returned {} bytes, expected at least {}",
            output.stdout.len(),
            expected
        ))));
    }
    let mut luma = output.stdout;
    luma.truncate(expected);
    Ok((width, height, luma))
}

fn parse_showinfo_dimensions(stderr: &str) -> Option<(u32, u32)> {
    for line in stderr.lines() {
        if let Some(idx) = line.find("s:") {
            let rest = &line[idx + 2..];
            let end = rest.find(|c: char| !(c.is_ascii_digit() || c == 'x')).unwrap_or(rest.len());
            let spec = &rest[..end];
            if let Some((w, h)) = spec.split_once('x') {
                if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                    return Some((w, h));
                }
            }
        }
    }
    None
}
```

- [ ] **Step 2: Re-export from `crates/render/src/lib.rs`**

Add `extract_frame_raw_gray` to the existing re-export list on line ~57 (already exports `extract_frame`, `extract_frame_complex`, `extract_frame_filtered`).

- [ ] **Step 3: Verify the crate still builds + clippy clean**

```
cargo build -p awidat-render
cargo clippy -p awidat-render --all-targets -- -D warnings
```

Expected: both pass.

- [ ] **Step 4: Commit**

```bash
git add crates/render/src/ffmpeg.rs crates/render/src/lib.rs
git commit -m "Add raw grayscale single-frame extraction helper"
```

---

## Task 3: Scorer module — types, parser, geometry, occlusion (in-memory sampler)

**Files:**
- Create: `crates/core/src/caption_rendered_output_scorer.rs`
- Modify: `crates/core/src/lib.rs` (declare module + re-exports)

- [ ] **Step 1: Add module declaration**

In `crates/core/src/lib.rs`, with the other `pub mod` declarations:
```rust
pub mod caption_rendered_output_scorer;
```

- [ ] **Step 2: Write the failing tests first (all five focused unit tests in one batch)**

Create `crates/core/src/caption_rendered_output_scorer.rs` with only the test module + stubs:

```rust
//! Frame-pixel caption rendered-output scorer.
//!
//! Parses Dialogue lines from ASS sidecars, computes per-event bounding boxes
//! against PlayRes + style margins + a safe-area profile, asks a
//! [`CaptionFrameSampler`] for a grayscale frame at the event midpoint, then
//! decides safe-area and occlusion outcomes. Production wiring lives in
//! [`crate::tools::verify_render`].

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ScorerError {
    #[error("frame sampler unavailable: {0}")]
    SamplerUnavailable(&'static str),
    #[error("render output missing or empty")]
    RenderOutputMissing,
    #[error("all sidecars failed to parse")]
    SidecarParseFailed,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub luma: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionEventFinding {
    pub start_ms: u64,
    pub end_ms: u64,
    pub probe_ms: u64,
    pub bbox: (u32, u32, u32, u32),
    pub safe_area_pass: bool,
    pub occlusion_fail: bool,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptionRenderedOutputEvidence {
    pub probe_count: usize,
    pub safe_area_pass_count: usize,
    pub occlusion_fail_count: usize,
    pub per_event_findings: Vec<CaptionEventFinding>,
    pub fallback_reason: Option<&'static str>,
}

#[async_trait::async_trait]
pub trait CaptionFrameSampler: Send + Sync {
    async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError>;
}

pub async fn score_caption_rendered_output(
    render_output: &Path,
    layout_sidecars: &[PathBuf],
    video_dims: (u32, u32),
    safe_area_profile: &str,
    sampler: &dyn CaptionFrameSampler,
) -> Result<CaptionRenderedOutputEvidence, ScorerError> {
    let _ = (render_output, layout_sidecars, video_dims, safe_area_profile, sampler);
    todo!("implemented in step 3")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct InMemoryFrameSampler {
        frames: Mutex<HashMap<u64, DecodedFrame>>, // key = t_ms / 10 (10ms buckets)
    }

    impl InMemoryFrameSampler {
        fn new() -> Self {
            Self { frames: Mutex::new(HashMap::new()) }
        }
        fn insert(&self, t_s: f64, frame: DecodedFrame) {
            let key = (t_s * 100.0).round() as u64;
            self.frames.lock().unwrap().insert(key, frame);
        }
    }

    #[async_trait::async_trait]
    impl CaptionFrameSampler for InMemoryFrameSampler {
        async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError> {
            let key = (t_s * 100.0).round() as u64;
            let guard = self.frames.lock().unwrap();
            for candidate in [key, key.saturating_sub(1), key + 1, key.saturating_sub(2), key + 2] {
                if let Some(frame) = guard.get(&candidate) {
                    return Ok(frame.clone());
                }
            }
            Err(ScorerError::SamplerUnavailable("no_frame_for_timestamp"))
        }
    }

    fn checkerboard_frame(width: u32, height: u32) -> DecodedFrame {
        let mut luma = vec![0u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                luma[(y * width + x) as usize] = if (x / 4 + y / 4) % 2 == 0 { 240 } else { 16 };
            }
        }
        DecodedFrame { width, height, luma }
    }

    fn flat_frame(width: u32, height: u32, value: u8) -> DecodedFrame {
        DecodedFrame { width, height, luma: vec![value; (width * height) as usize] }
    }

    fn sidecar_with_event(start_s: f64, end_s: f64, alignment: u8, margin_v: u32) -> String {
        format!(
            "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,{alignment},80,80,{margin_v},1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,{a},{b},Default,,0,0,0,,hello world\n",
            a = ass_time(start_s),
            b = ass_time(end_s),
        )
    }

    fn ass_time(t: f64) -> String {
        let h = (t / 3600.0).floor() as u32;
        let m = ((t / 60.0) % 60.0).floor() as u32;
        let s = t % 60.0;
        format!("{h}:{m:02}:{s:05.2}")
    }

    fn write_tmp_sidecar(body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("captions.ass");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn scorer_passes_when_event_within_safe_area_and_variance_high() {
        let (_tmp, sidecar) = write_tmp_sidecar(&sidecar_with_event(0.0, 2.0, 2, 80));
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, checkerboard_frame(1920, 1080));
        let output_dir = tempfile::tempdir().unwrap();
        let render_output = output_dir.path().join("out.mp4");
        std::fs::write(&render_output, b"not actually mp4").unwrap();
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        ).await.unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.safe_area_pass_count, 1);
        assert_eq!(result.occlusion_fail_count, 0);
        assert!(result.fallback_reason.is_none());
    }

    #[tokio::test]
    async fn scorer_fails_safe_area_when_event_outside_margin() {
        // margin_v=0 puts the caption flush against the bottom edge, outside the default 5% inset.
        let (_tmp, sidecar) = write_tmp_sidecar(&sidecar_with_event(0.0, 2.0, 2, 0));
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, checkerboard_frame(1920, 1080));
        let output_dir = tempfile::tempdir().unwrap();
        let render_output = output_dir.path().join("out.mp4");
        std::fs::write(&render_output, b"x").unwrap();
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        ).await.unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.safe_area_pass_count, 0);
    }

    #[tokio::test]
    async fn scorer_fails_occlusion_when_inside_variance_low() {
        let (_tmp, sidecar) = write_tmp_sidecar(&sidecar_with_event(0.0, 2.0, 2, 80));
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, flat_frame(1920, 1080, 128));
        let output_dir = tempfile::tempdir().unwrap();
        let render_output = output_dir.path().join("out.mp4");
        std::fs::write(&render_output, b"x").unwrap();
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        ).await.unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.occlusion_fail_count, 1);
    }

    #[tokio::test]
    async fn scorer_returns_empty_evidence_when_no_events() {
        let body = "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name\nStyle: Default,Arial\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";
        let (_tmp, sidecar) = write_tmp_sidecar(body);
        let sampler = InMemoryFrameSampler::new();
        let output_dir = tempfile::tempdir().unwrap();
        let render_output = output_dir.path().join("out.mp4");
        std::fs::write(&render_output, b"x").unwrap();
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        ).await.unwrap();
        assert_eq!(result.probe_count, 0);
        assert_eq!(result.fallback_reason, Some("no_caption_events"));
    }

    #[tokio::test]
    async fn scorer_reports_partial_when_some_events_unparseable() {
        let body = format!(
            "[Script Info]\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,80,80,40,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,0:00:00.00,0:00:02.00,Default,,0,0,0,,hello world\n\
Dialogue: not-a-valid-line\n",
        );
        let (_tmp, sidecar) = write_tmp_sidecar(&body);
        let sampler = InMemoryFrameSampler::new();
        sampler.insert(1.0, checkerboard_frame(1920, 1080));
        let output_dir = tempfile::tempdir().unwrap();
        let render_output = output_dir.path().join("out.mp4");
        std::fs::write(&render_output, b"x").unwrap();
        let result = score_caption_rendered_output(
            &render_output,
            &[sidecar],
            (1920, 1080),
            "default",
            &sampler,
        ).await.unwrap();
        assert_eq!(result.probe_count, 1);
        assert_eq!(result.fallback_reason, Some("partial_scorer_evidence"));
    }
}
```

Add dependencies to `crates/core/Cargo.toml` if not already present:
- `async-trait` (check first — `grep '^async-trait' crates/core/Cargo.toml` — only add if missing)
- `tempfile` under `[dev-dependencies]` (check first)
- `tokio` with `macros` and `rt-multi-thread` features for `#[tokio::test]` (already a dep — verify)

- [ ] **Step 3: Run tests to verify they fail**

```
cargo test -p awidat-core --lib caption_rendered_output_scorer
```

Expected: FAIL — `score_caption_rendered_output` is `todo!()`.

- [ ] **Step 4: Implement the scorer**

Replace the `score_caption_rendered_output` stub with the real implementation, plus private helpers. The complete implementation:

```rust
pub async fn score_caption_rendered_output(
    render_output: &Path,
    layout_sidecars: &[PathBuf],
    video_dims: (u32, u32),
    safe_area_profile: &str,
    sampler: &dyn CaptionFrameSampler,
) -> Result<CaptionRenderedOutputEvidence, ScorerError> {
    let render_meta = std::fs::metadata(render_output)
        .map_err(|_| ScorerError::RenderOutputMissing)?;
    if render_meta.len() == 0 {
        return Err(ScorerError::RenderOutputMissing);
    }

    let safe_area = safe_area_inset(safe_area_profile, video_dims);

    let mut parsed_any = false;
    let mut had_parse_failures = false;
    let mut events: Vec<DialogueEvent> = Vec::new();
    for path in layout_sidecars {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => {
                had_parse_failures = true;
                continue;
            }
        };
        let (file_events, file_partial, file_ok) = parse_sidecar_events(&contents, video_dims);
        if file_ok {
            parsed_any = true;
        }
        if file_partial {
            had_parse_failures = true;
        }
        events.extend(file_events);
    }
    if !parsed_any && !events.is_empty() {
        // events came back but no sidecar reported a clean parse — impossible by construction, kept defensive
    }
    if !parsed_any && layout_sidecars.iter().any(|p| std::fs::metadata(p).is_ok()) && events.is_empty() && !had_parse_failures {
        // sidecars present but no Dialogue lines anywhere
        return Ok(CaptionRenderedOutputEvidence {
            probe_count: 0,
            safe_area_pass_count: 0,
            occlusion_fail_count: 0,
            per_event_findings: vec![],
            fallback_reason: Some("no_caption_events"),
        });
    }
    if !parsed_any {
        return Err(ScorerError::SidecarParseFailed);
    }
    if events.is_empty() {
        return Ok(CaptionRenderedOutputEvidence {
            probe_count: 0,
            safe_area_pass_count: 0,
            occlusion_fail_count: 0,
            per_event_findings: vec![],
            fallback_reason: Some("no_caption_events"),
        });
    }

    let mut findings = Vec::with_capacity(events.len());
    let mut probe_count = 0usize;
    let mut safe_area_pass_count = 0usize;
    let mut occlusion_fail_count = 0usize;

    for event in &events {
        let probe_ms = (event.start_ms + event.end_ms) / 2;
        let t_s = (probe_ms as f64) / 1000.0;
        let frame = match sampler.sample(t_s).await {
            Ok(f) => f,
            Err(_) => {
                findings.push(CaptionEventFinding {
                    start_ms: event.start_ms,
                    end_ms: event.end_ms,
                    probe_ms,
                    bbox: event.bbox,
                    safe_area_pass: false,
                    occlusion_fail: false,
                    reason: "sample_failed",
                });
                continue;
            }
        };

        let (x, y, w, h) = event.bbox;
        let safe_pass = bbox_within_inset(event.bbox, video_dims, safe_area);
        let inside_var = luma_variance_in_rect(&frame, (x, y, w, h));
        let halo_var = luma_variance_in_halo(&frame, (x, y, w, h), 8);
        let occlusion_fail = inside_var < halo_var + 4.0;

        probe_count += 1;
        if safe_pass {
            safe_area_pass_count += 1;
        }
        if occlusion_fail {
            occlusion_fail_count += 1;
        }

        findings.push(CaptionEventFinding {
            start_ms: event.start_ms,
            end_ms: event.end_ms,
            probe_ms,
            bbox: event.bbox,
            safe_area_pass: safe_pass,
            occlusion_fail,
            reason: "scored",
        });
    }

    let fallback_reason = if had_parse_failures {
        Some("partial_scorer_evidence")
    } else {
        None
    };

    Ok(CaptionRenderedOutputEvidence {
        probe_count,
        safe_area_pass_count,
        occlusion_fail_count,
        per_event_findings: findings,
        fallback_reason,
    })
}

struct DialogueEvent {
    start_ms: u64,
    end_ms: u64,
    bbox: (u32, u32, u32, u32),
}

#[derive(Debug, Default, Clone)]
struct StyleRecord {
    fontsize: u32,
    alignment: u8,
    margin_l: u32,
    margin_r: u32,
    margin_v: u32,
}

fn safe_area_inset(profile: &str, dims: (u32, u32)) -> (u32, u32, u32, u32) {
    // returns (left, top, right, bottom) insets in output pixels
    let (w, h) = dims;
    match profile {
        "mobile" => (w * 5 / 100, h * 10 / 100, w * 5 / 100, h * 10 / 100),
        _ => (w * 5 / 100, h * 5 / 100, w * 5 / 100, h * 5 / 100),
    }
}

fn bbox_within_inset(
    bbox: (u32, u32, u32, u32),
    dims: (u32, u32),
    inset: (u32, u32, u32, u32),
) -> bool {
    let (x, y, w, h) = bbox;
    let (l, t, r, b) = inset;
    let (fw, fh) = dims;
    x >= l && y >= t && x + w <= fw.saturating_sub(r) && y + h <= fh.saturating_sub(b)
}

fn luma_variance_in_rect(frame: &DecodedFrame, rect: (u32, u32, u32, u32)) -> f64 {
    let (x, y, w, h) = clamp_rect(rect, (frame.width, frame.height));
    if w == 0 || h == 0 { return 0.0; }
    let mut sum = 0u64;
    let mut sum_sq = 0u64;
    let mut count = 0u64;
    for yy in y..y + h {
        for xx in x..x + w {
            let v = frame.luma[(yy * frame.width + xx) as usize] as u64;
            sum += v;
            sum_sq += v * v;
            count += 1;
        }
    }
    if count == 0 { return 0.0; }
    let mean = sum as f64 / count as f64;
    (sum_sq as f64 / count as f64) - mean * mean
}

fn luma_variance_in_halo(
    frame: &DecodedFrame,
    rect: (u32, u32, u32, u32),
    halo: u32,
) -> f64 {
    let (x, y, w, h) = rect;
    let dims = (frame.width, frame.height);
    let outer = (
        x.saturating_sub(halo),
        y.saturating_sub(halo),
        w + 2 * halo,
        h + 2 * halo,
    );
    let (ox, oy, ow, oh) = clamp_rect(outer, dims);
    if ow == 0 || oh == 0 { return 0.0; }
    let mut sum = 0u64;
    let mut sum_sq = 0u64;
    let mut count = 0u64;
    for yy in oy..oy + oh {
        for xx in ox..ox + ow {
            let inside = xx >= x && xx < x + w && yy >= y && yy < y + h;
            if inside { continue; }
            let v = frame.luma[(yy * frame.width + xx) as usize] as u64;
            sum += v;
            sum_sq += v * v;
            count += 1;
        }
    }
    if count == 0 { return 0.0; }
    let mean = sum as f64 / count as f64;
    (sum_sq as f64 / count as f64) - mean * mean
}

fn clamp_rect(rect: (u32, u32, u32, u32), dims: (u32, u32)) -> (u32, u32, u32, u32) {
    let (x, y, w, h) = rect;
    let (fw, fh) = dims;
    let x = x.min(fw);
    let y = y.min(fh);
    let w = w.min(fw.saturating_sub(x));
    let h = h.min(fh.saturating_sub(y));
    (x, y, w, h)
}

fn parse_sidecar_events(contents: &str, video_dims: (u32, u32)) -> (Vec<DialogueEvent>, bool, bool) {
    let playres = parse_playres(contents).unwrap_or(video_dims);
    let styles = parse_styles(contents);
    let mut events: Vec<DialogueEvent> = Vec::new();
    let mut any_bad = false;
    let mut any_good = false;
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("Dialogue:") { continue; }
        let rest = trimmed.trim_start_matches("Dialogue:").trim_start();
        let fields: Vec<&str> = rest.splitn(10, ',').collect();
        if fields.len() < 10 {
            any_bad = true;
            continue;
        }
        let (start_ms, end_ms) = match (parse_ass_time_ms(fields[1].trim()), parse_ass_time_ms(fields[2].trim())) {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => { any_bad = true; continue; }
        };
        let style_name = fields[3].trim();
        let style = styles.get(style_name).cloned().unwrap_or_default();
        let margin_l_override: u32 = fields[5].trim().parse().unwrap_or(0);
        let margin_r_override: u32 = fields[6].trim().parse().unwrap_or(0);
        let margin_v_override: u32 = fields[7].trim().parse().unwrap_or(0);
        let margin_l = if margin_l_override > 0 { margin_l_override } else { style.margin_l };
        let margin_r = if margin_r_override > 0 { margin_r_override } else { style.margin_r };
        let margin_v = if margin_v_override > 0 { margin_v_override } else { style.margin_v };
        let text = fields[9];
        let line_count = 1 + text.matches("\\N").count() as u32;

        let bbox = compute_bbox(playres, video_dims, &style, margin_l, margin_r, margin_v, line_count);
        events.push(DialogueEvent { start_ms, end_ms, bbox });
        any_good = true;
    }
    (events, any_bad && any_good, any_good)
}

fn compute_bbox(
    playres: (u32, u32),
    video_dims: (u32, u32),
    style: &StyleRecord,
    margin_l: u32,
    margin_r: u32,
    margin_v: u32,
    line_count: u32,
) -> (u32, u32, u32, u32) {
    let (pw, ph) = playres;
    let (fw, fh) = video_dims;
    let sx = fw as f64 / pw as f64;
    let sy = fh as f64 / ph as f64;
    let line_height = ((style.fontsize as f64) * 1.2 * sy).round() as u32;
    let height = line_height.saturating_mul(line_count);
    let width = fw.saturating_sub(((margin_l as f64) * sx).round() as u32)
        .saturating_sub(((margin_r as f64) * sx).round() as u32);
    let x = ((margin_l as f64) * sx).round() as u32;
    let margin_v_px = ((margin_v as f64) * sy).round() as u32;
    // ASS alignment numpad: 1/2/3 = bottom, 4/5/6 = mid, 7/8/9 = top
    let y = match style.alignment {
        7 | 8 | 9 => margin_v_px,
        4 | 5 | 6 => (fh.saturating_sub(height)) / 2,
        _ => fh.saturating_sub(height).saturating_sub(margin_v_px),
    };
    (x, y, width, height)
}

fn parse_playres(contents: &str) -> Option<(u32, u32)> {
    let mut x = None;
    let mut y = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("PlayResX:") { x = rest.trim().parse().ok(); }
        if let Some(rest) = line.strip_prefix("PlayResY:") { y = rest.trim().parse().ok(); }
    }
    match (x, y) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

fn parse_styles(contents: &str) -> std::collections::HashMap<String, StyleRecord> {
    let mut out = std::collections::HashMap::new();
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Style:") {
            let fields: Vec<&str> = rest.split(',').map(str::trim).collect();
            if fields.len() < 23 { continue; }
            let name = fields[0].to_string();
            let fontsize = fields[2].parse().unwrap_or(0);
            let alignment = fields[18].parse().unwrap_or(2);
            let margin_l = fields[19].parse().unwrap_or(0);
            let margin_r = fields[20].parse().unwrap_or(0);
            let margin_v = fields[21].parse().unwrap_or(0);
            out.insert(name, StyleRecord { fontsize, alignment, margin_l, margin_r, margin_v });
        }
    }
    out
}

fn parse_ass_time_ms(s: &str) -> Option<u64> {
    // h:mm:ss.cs
    let mut parts = s.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec = parts.next()?;
    let mut sec_parts = sec.split('.');
    let s_whole: u64 = sec_parts.next()?.parse().ok()?;
    let cs: u64 = sec_parts.next().unwrap_or("0").parse().ok()?;
    Some(((h * 3600 + m * 60 + s_whole) * 1000) + cs * 10)
}
```

- [ ] **Step 5: Run tests to verify they pass**

```
cargo test -p awidat-core --lib caption_rendered_output_scorer
```

Expected: PASS, 5 tests.

- [ ] **Step 6: Format + clippy**

```
cargo fmt --all
cargo clippy -p awidat-core --tests -- -D warnings
```

Expected: both clean.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/caption_rendered_output_scorer.rs crates/core/src/lib.rs crates/core/Cargo.toml
git commit -m "Add caption frame-pixel scorer with in-memory sampler tests"
```

---

## Task 4: Production `FfmpegFrameSampler`

**Files:**
- Modify: `crates/core/src/caption_rendered_output_scorer.rs` (append)

- [ ] **Step 1: Append the production sampler**

```rust
/// Production [`CaptionFrameSampler`] backed by ffmpeg raw-grayscale frame extraction.
pub struct FfmpegFrameSampler {
    pub render_output: PathBuf,
}

impl FfmpegFrameSampler {
    pub fn new(render_output: PathBuf) -> Self {
        Self { render_output }
    }
}

#[async_trait::async_trait]
impl CaptionFrameSampler for FfmpegFrameSampler {
    async fn sample(&self, t_s: f64) -> Result<DecodedFrame, ScorerError> {
        let (width, height, luma) = awidat_render::extract_frame_raw_gray(&self.render_output, t_s)
            .await
            .map_err(|_| ScorerError::SamplerUnavailable("ffmpeg_extract_failed"))?;
        Ok(DecodedFrame { width, height, luma })
    }
}
```

If `awidat-render` is not yet a dep of `awidat-core`, check `crates/core/Cargo.toml`:
```
grep '^awidat-render\b\|awidat_render' crates/core/Cargo.toml
```
If absent, add under `[dependencies]`:
```toml
awidat-render = { path = "../render" }
```

- [ ] **Step 2: Build + clippy**

```
cargo build -p awidat-core
cargo clippy -p awidat-core --tests -- -D warnings
```

Expected: both clean.

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/caption_rendered_output_scorer.rs crates/core/Cargo.toml
git commit -m "Add ffmpeg-backed CaptionFrameSampler"
```

---

## Task 5: Wire scorer into `verify_render_output`

**Files:**
- Modify: `crates/core/src/tools/verify_render.rs`

- [ ] **Step 1: Write the three integration tests first**

Append to the existing `#[cfg(test)] mod tests` block in `crates/core/src/tools/verify_render.rs`. Reuse the `InMemoryFrameSampler` pattern from Task 3 by importing it via `crate::caption_rendered_output_scorer::tests`. If the in-memory sampler is gated behind `#[cfg(test)]` and not accessible cross-module, lift the `InMemoryFrameSampler` to a public-in-crate module under `caption_rendered_output_scorer::test_support` gated by `#[cfg(test)]`. Implement that lift in this step.

In `crates/core/src/caption_rendered_output_scorer.rs`, replace the test-private `InMemoryFrameSampler` with a `#[cfg(test)] pub(crate) mod test_support { ... }` module containing the same struct + impl. Update the existing tests in Task 3 to import from `super::test_support::InMemoryFrameSampler`.

Then add the three integration tests:

```rust
#[tokio::test]
async fn verify_render_uses_frame_pixel_scorer_when_render_output_present() {
    // build a synthetic project + render manifest with caption summary + sidecar paths,
    // override the scorer to use InMemoryFrameSampler, run verify_render_output,
    // assert the caption_rendered_output_readable gate passes with reason = frame_pixel_scorer_passed.
    // Use the existing helpers in this test module (search for verify_render_reports_synthetic_render_gates).
    // The InMemoryFrameSampler is injected via a new `with_caption_frame_sampler` option on VerifyRenderOptions.
    todo!("filled in step 2 alongside the wiring change")
}

#[tokio::test]
async fn verify_render_falls_back_to_libass_layout_when_scorer_unavailable() {
    todo!("filled in step 2")
}

#[tokio::test]
async fn verify_render_reports_frame_pixel_scorer_failed_on_failed_evidence() {
    todo!("filled in step 2")
}
```

- [ ] **Step 2: Implement the wiring + flesh out the tests**

In `crates/core/src/tools/verify_render.rs`:

1. Add an injection hook on `VerifyRenderOptions`:
   ```rust
   pub caption_frame_sampler_override: Option<std::sync::Arc<dyn crate::caption_rendered_output_scorer::CaptionFrameSampler>>,
   ```
   Default to `None`.

2. In `verify_render_output`, after `let render_manifest = collect_render_manifest_evidence(...)` and `let caption_summary = ...`, *before* the sync gate-builder calls, add:

   ```rust
   maybe_run_caption_scorer(
       &project,
       &mut render_manifest,
       &caption_summary,
       options.caption_frame_sampler_override.as_deref(),
   ).await;
   ```

   (Take `render_manifest` as `&mut Option<RenderManifestEvidence>` so we can mutate the metadata view.)

3. Implement `maybe_run_caption_scorer`:

   ```rust
   async fn maybe_run_caption_scorer(
       _project: &awidat_proto::Project,
       render_manifest: &mut Option<crate::tools::verify_render::RenderManifestEvidence>,
       caption_summary: &crate::captions::CaptionSummary,
       sampler_override: Option<&dyn crate::caption_rendered_output_scorer::CaptionFrameSampler>,
   ) {
       if !caption_summary.has_exportable_captions { return; }
       let Some(manifest) = render_manifest.as_mut() else { return; };
       let Some(output) = manifest.output_path.as_ref() else { return; };
       let Some(sidecar_csv) = manifest.metadata.get("libass_layout_sidecar_paths").cloned() else { return; };
       let sidecars: Vec<std::path::PathBuf> = sidecar_csv
           .split(',')
           .filter(|s| !s.is_empty())
           .map(std::path::PathBuf::from)
           .collect();
       if sidecars.is_empty() { return; }
       let safe_area_profile = caption_summary.selected_safe_area_profile.as_deref().unwrap_or("default");
       let video_dims: (u32, u32) = (
           manifest.metadata.get("render_resolution_width").and_then(|v| v.parse().ok()).unwrap_or(1920),
           manifest.metadata.get("render_resolution_height").and_then(|v| v.parse().ok()).unwrap_or(1080),
       );

       let owned_sampler;
       let sampler: &dyn crate::caption_rendered_output_scorer::CaptionFrameSampler = if let Some(s) = sampler_override {
           s
       } else {
           owned_sampler = crate::caption_rendered_output_scorer::FfmpegFrameSampler::new(std::path::PathBuf::from(output));
           &owned_sampler
       };

       match crate::caption_rendered_output_scorer::score_caption_rendered_output(
           std::path::Path::new(output),
           &sidecars,
           video_dims,
           safe_area_profile,
           sampler,
       ).await {
           Ok(evidence) => {
               manifest.metadata.insert("caption_rendered_output_source".into(), "frame_pixel_scorer".into());
               let status = if evidence.probe_count == 0 {
                   "skipped"
               } else if evidence.safe_area_pass_count == evidence.probe_count && evidence.occlusion_fail_count == 0 {
                   "passed"
               } else {
                   "failed"
               };
               manifest.metadata.insert("caption_rendered_output_status".into(), status.into());
               manifest.metadata.insert("caption_rendered_output_probe_count".into(), evidence.probe_count.to_string());
               manifest.metadata.insert("caption_rendered_output_safe_area_pass_count".into(), evidence.safe_area_pass_count.to_string());
               manifest.metadata.insert("caption_rendered_output_occlusion_fail_count".into(), evidence.occlusion_fail_count.to_string());
               if let Some(reason) = evidence.fallback_reason {
                   manifest.metadata.insert("caption_rendered_output_fallback_reason".into(), reason.into());
               }
           }
           Err(err) => {
               let reason = match err {
                   crate::caption_rendered_output_scorer::ScorerError::SamplerUnavailable(_) => "ffmpeg_unavailable",
                   crate::caption_rendered_output_scorer::ScorerError::RenderOutputMissing => "render_output_missing",
                   crate::caption_rendered_output_scorer::ScorerError::SidecarParseFailed => "sidecar_parse_failed",
                   crate::caption_rendered_output_scorer::ScorerError::Io(_) => "io_error",
               };
               manifest.metadata.insert("caption_rendered_output_fallback_reason".into(), reason.into());
           }
       }
   }
   ```

4. In `add_caption_rendered_output_gate`, extend the `reason` selection so:
   - When `caption_rendered_output_source == "frame_pixel_scorer"` and `passed` → `reason = "frame_pixel_scorer_passed"`.
   - When `caption_rendered_output_source == "frame_pixel_scorer"` and `!passed && has_evidence` → `reason = "frame_pixel_scorer_failed"`.
   - When `caption_rendered_output_fallback_reason` is set and the libass-layout fallback ultimately passes → `reason = "frame_pixel_scorer_unavailable_fell_back_to_libass_layout"`.
   - Existing reasons (`derived_from_libass_layout_evidence`, `missing_caption_rendered_output_evidence`, `passed`, `caption_rendered_output_evidence_failed`) preserved otherwise.

5. Flesh out the three integration tests:

```rust
#[tokio::test]
async fn verify_render_uses_frame_pixel_scorer_when_render_output_present() {
    use crate::caption_rendered_output_scorer::test_support::InMemoryFrameSampler;
    use std::sync::Arc;

    let setup = synthetic_caption_render_setup_with_sidecars();
    let sampler = Arc::new(InMemoryFrameSampler::new());
    sampler.insert(1.0, checkerboard_frame_for_tests(1920, 1080));
    let options = VerifyRenderOptions {
        caption_frame_sampler_override: Some(sampler.clone()),
        ..VerifyRenderOptions::default()
    };
    let report = verify_render_output(&setup.project_root, &setup.output_path, options).await.unwrap();
    let gate = report.gates.iter().find(|g| g.name == "caption_rendered_output_readable").unwrap();
    assert!(gate.passed, "gate details: {:?}", gate.details);
    assert_eq!(gate.details["reason"], "frame_pixel_scorer_passed");
}

#[tokio::test]
async fn verify_render_falls_back_to_libass_layout_when_scorer_unavailable() {
    use crate::caption_rendered_output_scorer::test_support::AlwaysUnavailableFrameSampler;
    use std::sync::Arc;
    let setup = synthetic_caption_render_setup_with_sidecars();
    let sampler: Arc<dyn crate::caption_rendered_output_scorer::CaptionFrameSampler> = Arc::new(AlwaysUnavailableFrameSampler);
    let options = VerifyRenderOptions {
        caption_frame_sampler_override: Some(sampler),
        ..VerifyRenderOptions::default()
    };
    let report = verify_render_output(&setup.project_root, &setup.output_path, options).await.unwrap();
    let gate = report.gates.iter().find(|g| g.name == "caption_rendered_output_readable").unwrap();
    assert!(gate.passed);
    assert_eq!(gate.details["reason"], "frame_pixel_scorer_unavailable_fell_back_to_libass_layout");
}

#[tokio::test]
async fn verify_render_reports_frame_pixel_scorer_failed_on_failed_evidence() {
    use crate::caption_rendered_output_scorer::test_support::InMemoryFrameSampler;
    use std::sync::Arc;
    let setup = synthetic_caption_render_setup_with_sidecars();
    let sampler = Arc::new(InMemoryFrameSampler::new());
    sampler.insert(1.0, flat_frame_for_tests(1920, 1080, 128));
    let options = VerifyRenderOptions {
        caption_frame_sampler_override: Some(sampler),
        ..VerifyRenderOptions::default()
    };
    let report = verify_render_output(&setup.project_root, &setup.output_path, options).await.unwrap();
    let gate = report.gates.iter().find(|g| g.name == "caption_rendered_output_readable").unwrap();
    assert!(!gate.passed);
    assert_eq!(gate.details["reason"], "frame_pixel_scorer_failed");
}
```

Add helper `synthetic_caption_render_setup_with_sidecars` to the test module by extending the existing helper used in `verify_render_reports_synthetic_render_gates`. Reuse its fixture; add a written `.ass` sidecar on disk and an `libass_layout_sidecar_paths` metadata key referencing it.

Add `AlwaysUnavailableFrameSampler` to `caption_rendered_output_scorer::test_support`:

```rust
pub struct AlwaysUnavailableFrameSampler;
#[async_trait::async_trait]
impl super::CaptionFrameSampler for AlwaysUnavailableFrameSampler {
    async fn sample(&self, _t_s: f64) -> Result<super::DecodedFrame, super::ScorerError> {
        Err(super::ScorerError::SamplerUnavailable("test_unavailable"))
    }
}
```

Define `checkerboard_frame_for_tests` and `flat_frame_for_tests` either inside the verify_render test module (copying the bodies from Task 3's tests) or by re-exporting helpers from `test_support`.

- [ ] **Step 3: Run the integration tests**

```
cargo test -p awidat-core --lib verify_render_uses_frame_pixel_scorer_when_render_output_present
cargo test -p awidat-core --lib verify_render_falls_back_to_libass_layout_when_scorer_unavailable
cargo test -p awidat-core --lib verify_render_reports_frame_pixel_scorer_failed_on_failed_evidence
```

Expected: all PASS.

- [ ] **Step 4: Re-run all caption tests + verify gate tests for no regression**

```
cargo test -p awidat-core --lib caption
cargo test -p awidat-core caption_rendered_output_gate
cargo test -p awidat-core verify_render_reports_synthetic_render_gates
```

Expected: PASS.

- [ ] **Step 5: Format + clippy**

```
cargo fmt --all
cargo clippy -p awidat-core --tests -- -D warnings
```

Expected: both clean.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/tools/verify_render.rs crates/core/src/caption_rendered_output_scorer.rs
git commit -m "Run frame-pixel caption scorer before verify_render gates"
```

---

## Task 6: Update capability metadata notes

**Files:**
- Modify: `crates/core/src/capabilities.rs:395-403` (caption_safe_area_verification block)
- Modify: `crates/core/src/capability_metadata.rs` (mirror note)
- Modify: `crates/core/tests/capability_manifest.rs` (expected string)

- [ ] **Step 1: Update the note in `capabilities.rs`**

Replace the current note text:
```
"currently validates safe-area metadata for caption overlays; editable subtitle tracks and sidecar captions are summarized separately"
```
with:
```
"safe-area and occlusion are now measured per caption event from rendered output via the frame-pixel scorer when ffmpeg and a render output are available; libass-layout sidecar derivation remains a named fallback path"
```

- [ ] **Step 2: Mirror the same note text in `capability_metadata.rs`** wherever the `caption_safe_area_verification` entry duplicates the note. (Search: `grep -n "caption_safe_area_verification" crates/core/src/capability_metadata.rs`.)

- [ ] **Step 3: Update the expected string in the capability manifest test**

In `crates/core/tests/capability_manifest.rs`, find the assertion checking the caption_safe_area_verification note and replace the expected string with the new one. (Search: `grep -n "currently validates safe-area metadata" crates/core/tests/capability_manifest.rs`.)

- [ ] **Step 4: Run the capability test**

```
cargo test -p awidat-core --test capability_manifest
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/capabilities.rs crates/core/src/capability_metadata.rs crates/core/tests/capability_manifest.rs
git commit -m "Update caption verification capability note for frame-pixel scorer"
```

---

## Task 7: Broad workspace verification

- [ ] **Step 1: Format check**

```
cargo fmt --all -- --check
```

Expected: PASS (no output).

- [ ] **Step 2: Workspace clippy**

```
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: exit 0 (only pre-existing `ts-rs failed to parse this attribute` notes from desktop generated exports — not errors).

- [ ] **Step 3: Workspace tests**

```
cargo test --workspace
```

Expected: 0 failures across all binaries.

- [ ] **Step 4: Update the living plan**

Edit `docs/awidat-improvement-living-plan.md`:
- Move the "Frame-pixel caption occlusion scoring" line out of "Not yet started or not yet production-complete".
- Add a new "Completed Slice" entry with the verification commands run + result.
- Update the "Last updated" header and the Running Log.
- Update the "Next Slice" to point at the resumable preview worker pool slice.

- [ ] **Step 5: Final commit**

```bash
git add docs/awidat-improvement-living-plan.md
git commit -m "Mark frame-pixel caption scorer slice complete in living plan"
```

---

## Self-Review

**Spec coverage:** Every architecture point in the spec (scorer module, trait, prod sampler, in-memory sampler, sidecar path surfacing, verify_render wiring, capability metadata, build sequence) maps to a task above.

**Placeholder scan:** No TBD/TODO bodies. Every code step shows the actual code to write. The two `todo!()` macros in Task 5 Step 1 are deliberately the failing-test stage — they are replaced with concrete code in Step 2 of the same task.

**Type consistency:** `CaptionFrameSampler`, `DecodedFrame`, `CaptionRenderedOutputEvidence`, `CaptionEventFinding`, `ScorerError`, `FfmpegFrameSampler`, `InMemoryFrameSampler`, `AlwaysUnavailableFrameSampler`, and `score_caption_rendered_output` are used with consistent signatures across tasks. The new metadata keys (`libass_layout_sidecar_paths`, `caption_rendered_output_source`, `caption_rendered_output_status`, `caption_rendered_output_probe_count`, `caption_rendered_output_safe_area_pass_count`, `caption_rendered_output_occlusion_fail_count`, `caption_rendered_output_fallback_reason`) match what the existing gate already reads.
