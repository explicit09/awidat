//! Indexer drill-down readers (Wave 3 C1). Each command takes a
//! `(project_path, stem)` and returns the parsed evidence rows for one
//! indexer sidecar. The Brief surface's evidence chips open per-kind
//! drill-down panels that call these.
//!
//! Sidecar conventions:
//!   - `index/scenedetect/<asset_id>.json`     → AssetId-keyed
//!   - `index/audio-energy/<asset_id>.json`    → AssetId-keyed
//!   - `index/face/<asset_id>.json`            → AssetId-keyed
//!   - `index/color-analysis/<asset_id>.json`  → AssetId-keyed
//!   - `.awidat/silences/<stem>-<hash>.json`   → proxy-stem + FNV hash
//!   - `.awidat/motion/<stem>-<hash>.json`     → proxy-stem + FNV hash
//!
//! Path resolution mirrors `commands::transcript::read_transcript`:
//! walk `<project>/raw/` and recompute each candidate's proxy filename
//! until one matches the incoming `stem`. From there we derive both the
//! AssetId (project-relative path) and the proxy-stem-hash sidecar path.
//!
//! Missing-file policy: a sidecar that doesn't exist is `Ok(vec![])`,
//! not an error. The frontend distinguishes "indexer hasn't run yet"
//! from "indexer ran and found nothing" via `useEvidenceAvailability()`;
//! both drill-downs render an empty state, but the chip availability
//! map only flips true once at least one row exists.

use std::path::{Path, PathBuf};

use awidat_proto::index::AssetId;
use serde::{Deserialize, Serialize};

use crate::commands::media::{motion_path_for, proxy_path_for, silences_path_for};

// --- Output shapes (mirror `evidenceAccessors.ts` Evidence* interfaces) ---

/// One scene boundary from the scenedetect indexer. Mirrors
/// `EvidenceScene` in the frontend accessor module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub start_s: f64,
    pub end_s: f64,
    /// Optional filmstrip thumbnail — never produced by scenedetect today,
    /// reserved for a future thumbnail-grid pipeline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_path: Option<String>,
}

/// One silence range from the silence indexer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Silence {
    pub start_s: f64,
    pub end_s: f64,
    pub duration_s: f64,
}

/// One audio-energy sample (one 100ms window in source-time).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AudioSample {
    pub at_s: f64,
    /// Loudness magnitude, normalised to `[0.0, 1.0]`. The on-disk
    /// sidecar emits dBFS (`rms_db` in the python `audio-energy-mcp`);
    /// we map the typical `[-80, 0]` dB band to `[0.0, 1.0]` so the
    /// frontend can render a sparkline without knowing the audio
    /// scale convention.
    pub rms: f64,
}

/// One face-detection record. `at_s` is the centre time of the frame
/// the face was seen on; `bbox` is the detection box in detector
/// coordinates (the python sidecar records the detect size as
/// `detect_width`/`detect_height`, exposed to the consumer via
/// `frame_rate_sampled`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceDetection {
    pub id: String,
    pub at_s: f64,
    pub bbox: BoundingBox,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One motion-energy region. The motion sidecar carries a flat
/// magnitude-per-second array — we expand each sample into a one-
/// second region the drill-down's heat strip can render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionRegion {
    pub id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub intensity: f64,
}

/// One per-shot color statistic from the color-analysis indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColorStat {
    pub id: String,
    pub start_s: f64,
    pub end_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub luma_mean: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saturation_mean: Option<f64>,
}

// --- Resolver: stem → (asset_path, asset_id) -----------------------

/// Walk `<project>/raw/` and find the asset whose proxy filename stem
/// matches `stem`. Falls back to raw-filename match (with or without
/// extension) so we still resolve transcripts whose whisper sidecar
/// landed before the proxy did. Returns `(asset_abs_path, asset_id)`
/// where `asset_id` is the project-relative posix-form path used as
/// the AssetId-keyed sidecar suffix.
fn resolve_asset(project_root: &Path, stem: &str) -> Option<(PathBuf, String)> {
    let raw_dir = project_root.join("raw");
    if !raw_dir.is_dir() {
        return None;
    }
    let proxies_dir = project_root.join(".awidat").join("proxies");
    let target_proxy_name = format!("{stem}.mp4");
    let abs = walk_for_match(&raw_dir, &proxies_dir, &target_proxy_name)?;
    let rel = abs
        .strip_prefix(project_root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    Some((abs, rel))
}

fn walk_for_match(dir: &Path, proxies_dir: &Path, target_proxy_name: &str) -> Option<PathBuf> {
    let raw_stem_target = target_proxy_name
        .strip_suffix(".mp4")
        .unwrap_or(target_proxy_name);
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = walk_for_match(&path, proxies_dir, target_proxy_name) {
                return Some(found);
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let proxy_path = proxy_path_for(proxies_dir, &path);
        if proxy_path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n == target_proxy_name)
        {
            return Some(path);
        }
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if file_name == raw_stem_target || file_stem == raw_stem_target {
            return Some(path);
        }
    }
    None
}

// --- Sidecar readers ----------------------------------------------

fn read_index_sidecar(
    project_root: &Path,
    indexer: &str,
    asset_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    let asset = AssetId::new(asset_id);
    match awidat_index::read_sidecar(project_root, indexer, &asset) {
        Ok(v) => Ok(Some(v)),
        Err(awidat_index::SidecarError::NotFound { .. }) => Ok(None),
        Err(e) => Err(format!("read {indexer} sidecar: {e}")),
    }
}

// --- Tauri commands -----------------------------------------------

/// Read scene boundaries from `index/scenedetect/<asset_id>.json`.
/// Returns `[]` when no sidecar exists or `raw/` has no asset matching
/// `stem`. Each scene's `id` is `scene-<idx>` so drill-down rows are
/// stable across renders.
#[tauri::command]
pub async fn read_scenes(project_path: String, stem: String) -> Result<Vec<Scene>, String> {
    let project_root = PathBuf::from(project_path);
    tokio::task::spawn_blocking(move || -> Result<Vec<Scene>, String> {
        let Some((_, asset_id)) = resolve_asset(&project_root, &stem) else {
            return Ok(Vec::new());
        };
        let Some(sidecar) = read_index_sidecar(&project_root, "scenedetect", &asset_id)? else {
            return Ok(Vec::new());
        };
        let shots = sidecar
            .get("data")
            .and_then(|d| d.get("shots"))
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        let scenes = shots
            .into_iter()
            .enumerate()
            .filter_map(|(idx, shot)| {
                let start_s = shot.get("start_s").and_then(serde_json::Value::as_f64)?;
                let end_s = shot.get("end_s").and_then(serde_json::Value::as_f64)?;
                Some(Scene {
                    id: format!("scene-{idx}"),
                    start_s,
                    end_s,
                    thumbnail_path: None,
                })
            })
            .collect();
        Ok(scenes)
    })
    .await
    .map_err(|e| format!("read_scenes join: {e}"))?
}

/// Read silence ranges from `.awidat/silences/<stem>-<hash>.json`. The
/// silence sidecar uses the renderer's `SilenceSidecar` shape
/// (`{ ranges, threshold_db, min_duration_s }`).
#[tauri::command]
pub async fn read_silences(project_path: String, stem: String) -> Result<Vec<Silence>, String> {
    let project_root = PathBuf::from(project_path);
    tokio::task::spawn_blocking(move || -> Result<Vec<Silence>, String> {
        let Some((asset_abs, _)) = resolve_asset(&project_root, &stem) else {
            return Ok(Vec::new());
        };
        let sidecar = silences_path_for(&project_root, &asset_abs);
        if !sidecar.exists() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&sidecar).map_err(|e| format!("read silence sidecar: {e}"))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse silence sidecar: {e}"))?;
        let ranges = value
            .get("ranges")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(ranges
            .into_iter()
            .filter_map(|r| {
                let start_s = r.get("start_s").and_then(serde_json::Value::as_f64)?;
                let end_s = r.get("end_s").and_then(serde_json::Value::as_f64)?;
                Some(Silence {
                    start_s,
                    end_s,
                    duration_s: (end_s - start_s).max(0.0),
                })
            })
            .collect())
    })
    .await
    .map_err(|e| format!("read_silences join: {e}"))?
}

/// Read RMS-over-time samples from `index/audio-energy/<asset_id>.json`.
/// The python `audio-energy-mcp` emits `windows[].rms_db` at 100ms
/// granularity; we map dBFS into a `[0.0, 1.0]` magnitude so the
/// sparkline renderer doesn't need to know the audio scale.
#[tauri::command]
pub async fn read_audio_samples(
    project_path: String,
    stem: String,
) -> Result<Vec<AudioSample>, String> {
    let project_root = PathBuf::from(project_path);
    tokio::task::spawn_blocking(move || -> Result<Vec<AudioSample>, String> {
        let Some((_, asset_id)) = resolve_asset(&project_root, &stem) else {
            return Ok(Vec::new());
        };
        let Some(sidecar) = read_index_sidecar(&project_root, "audio-energy", &asset_id)? else {
            return Ok(Vec::new());
        };
        let windows = sidecar
            .get("data")
            .and_then(|d| d.get("windows"))
            .and_then(|w| w.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(windows
            .into_iter()
            .filter_map(|w| {
                let at_s = w.get("start_s").and_then(serde_json::Value::as_f64)?;
                let rms_db = w.get("rms_db").and_then(serde_json::Value::as_f64)?;
                // dBFS `[-80, 0]` → magnitude `[0, 1]`. Clamp + linear
                // map. Anything below -80 reads as silence.
                let magnitude = ((rms_db + 80.0) / 80.0).clamp(0.0, 1.0);
                Some(AudioSample {
                    at_s,
                    rms: magnitude,
                })
            })
            .collect())
    })
    .await
    .map_err(|e| format!("read_audio_samples join: {e}"))?
}

/// Read face detections from `index/face/<asset_id>.json`. The python
/// `face-mcp` emits `per_frame[].faces[]` with `box: [x, y, w, h]`
/// pairs and a clustered `face_id`. We flatten into per-detection rows
/// keyed by frame time so the drill-down can group by identity.
#[tauri::command]
pub async fn read_faces(project_path: String, stem: String) -> Result<Vec<FaceDetection>, String> {
    let project_root = PathBuf::from(project_path);
    tokio::task::spawn_blocking(move || -> Result<Vec<FaceDetection>, String> {
        let Some((_, asset_id)) = resolve_asset(&project_root, &stem) else {
            return Ok(Vec::new());
        };
        let Some(sidecar) = read_index_sidecar(&project_root, "face", &asset_id)? else {
            return Ok(Vec::new());
        };
        let per_frame = sidecar
            .get("data")
            .and_then(|d| d.get("per_frame"))
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = Vec::new();
        for (frame_idx, frame) in per_frame.iter().enumerate() {
            let at_s = frame
                .get("t_s")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            let faces = frame.get("faces").and_then(|f| f.as_array());
            let Some(faces) = faces else { continue };
            for (face_idx, face) in faces.iter().enumerate() {
                let box_arr = face.get("box").and_then(|b| b.as_array());
                let bbox = box_arr
                    .and_then(|arr| {
                        if arr.len() >= 4 {
                            Some(BoundingBox {
                                x: arr[0].as_f64().unwrap_or(0.0),
                                y: arr[1].as_f64().unwrap_or(0.0),
                                w: arr[2].as_f64().unwrap_or(0.0),
                                h: arr[3].as_f64().unwrap_or(0.0),
                            })
                        } else {
                            None
                        }
                    })
                    .unwrap_or(BoundingBox {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    });
                let identity_id = face
                    .get("face_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                out.push(FaceDetection {
                    id: format!("face-{frame_idx}-{face_idx}"),
                    at_s,
                    bbox,
                    identity_id,
                });
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("read_faces join: {e}"))?
}

/// Read motion-energy regions from `.awidat/motion/<stem>-<hash>.json`.
/// The motion sidecar is a flat `magnitudes[]` at
/// `samples_per_second`; we expand each sample into a one-window
/// region so the heat strip can render each cell directly.
#[tauri::command]
pub async fn read_motion_regions(
    project_path: String,
    stem: String,
) -> Result<Vec<MotionRegion>, String> {
    let project_root = PathBuf::from(project_path);
    tokio::task::spawn_blocking(move || -> Result<Vec<MotionRegion>, String> {
        let Some((asset_abs, _)) = resolve_asset(&project_root, &stem) else {
            return Ok(Vec::new());
        };
        let sidecar = motion_path_for(&project_root, &asset_abs);
        if !sidecar.exists() {
            return Ok(Vec::new());
        }
        let bytes = std::fs::read(&sidecar).map_err(|e| format!("read motion sidecar: {e}"))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse motion sidecar: {e}"))?;
        let samples_per_second = value
            .get("samples_per_second")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1)
            .max(1) as f64;
        let window_s = 1.0 / samples_per_second;
        let magnitudes = value
            .get("magnitudes")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(magnitudes
            .into_iter()
            .enumerate()
            .filter_map(|(idx, m)| {
                let intensity = m.as_f64()?.clamp(0.0, 1.0);
                let start_s = idx as f64 * window_s;
                Some(MotionRegion {
                    id: format!("motion-{idx}"),
                    start_s,
                    end_s: start_s + window_s,
                    intensity,
                })
            })
            .collect())
    })
    .await
    .map_err(|e| format!("read_motion_regions join: {e}"))?
}

/// Read per-shot color statistics from
/// `index/color-analysis/<asset_id>.json`. The python `color-analysis-mcp`
/// emits a `scenes[]` array; each scene carries aggregated `brightness`
/// (0..255), `saturation_mean` (0..255 in HSV-S), plus the shot range.
/// We normalise both to `[0.0, 1.0]` for the drill-down's swatch row.
#[tauri::command]
pub async fn read_color_stats(
    project_path: String,
    stem: String,
) -> Result<Vec<ColorStat>, String> {
    let project_root = PathBuf::from(project_path);
    tokio::task::spawn_blocking(move || -> Result<Vec<ColorStat>, String> {
        let Some((_, asset_id)) = resolve_asset(&project_root, &stem) else {
            return Ok(Vec::new());
        };
        let Some(sidecar) = read_index_sidecar(&project_root, "color-analysis", &asset_id)? else {
            return Ok(Vec::new());
        };
        let scenes = sidecar
            .get("data")
            .and_then(|d| d.get("scenes"))
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(scenes
            .into_iter()
            .enumerate()
            .filter_map(|(idx, scene)| {
                let start_s = scene.get("start_s").and_then(serde_json::Value::as_f64)?;
                let end_s = scene.get("end_s").and_then(serde_json::Value::as_f64)?;
                let scene_id = scene
                    .get("scene_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("color-{idx}"));
                // The python sidecar uses 0..255 for brightness / sat;
                // map to 0..1. Falls back to `None` so a future sidecar
                // shipping normalized values doesn't double-divide.
                let luma_mean = scene
                    .get("brightness")
                    .and_then(serde_json::Value::as_f64)
                    .map(|v| if v > 1.0 { v / 255.0 } else { v }.clamp(0.0, 1.0));
                let saturation_mean = scene
                    .get("saturation_mean")
                    .and_then(serde_json::Value::as_f64)
                    .map(|v| if v > 1.0 { v / 255.0 } else { v }.clamp(0.0, 1.0));
                Some(ColorStat {
                    id: scene_id,
                    start_s,
                    end_s,
                    luma_mean,
                    saturation_mean,
                })
            })
            .collect())
    })
    .await
    .map_err(|e| format!("read_color_stats join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sidecar(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    #[tokio::test]
    async fn read_scenes_returns_empty_when_raw_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let scenes = read_scenes(dir.path().to_string_lossy().into_owned(), "missing".into())
            .await
            .unwrap();
        assert!(scenes.is_empty());
    }

    #[tokio::test]
    async fn read_scenes_returns_empty_when_sidecar_missing() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("clip.mov"), b"x").unwrap();
        let scenes = read_scenes(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert!(scenes.is_empty());
    }

    #[tokio::test]
    async fn read_scenes_projects_shot_rows() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let asset = raw.join("clip.mov");
        std::fs::write(&asset, b"x").unwrap();
        let sidecar = dir.path().join("index/scenedetect/raw/clip.mov.json");
        write_sidecar(
            &sidecar,
            r#"{
                "indexer": "scenedetect",
                "data": {
                    "shots": [
                        {"index": 0, "start_s": 0.0, "end_s": 1.5},
                        {"index": 1, "start_s": 1.5, "end_s": 4.0}
                    ]
                }
            }"#,
        );
        let scenes = read_scenes(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert_eq!(scenes.len(), 2);
        assert_eq!(scenes[0].id, "scene-0");
        assert!((scenes[0].end_s - 1.5).abs() < 1e-6);
        assert_eq!(scenes[1].id, "scene-1");
    }

    #[tokio::test]
    async fn read_audio_samples_normalises_db() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("clip.mov"), b"x").unwrap();
        let sidecar = dir.path().join("index/audio-energy/raw/clip.mov.json");
        write_sidecar(
            &sidecar,
            r#"{
                "indexer": "audio-energy",
                "data": {
                    "windows": [
                        {"start_s": 0.0, "rms_db": -80.0},
                        {"start_s": 0.1, "rms_db": -40.0},
                        {"start_s": 0.2, "rms_db": 0.0}
                    ]
                }
            }"#,
        );
        let samples = read_audio_samples(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert_eq!(samples.len(), 3);
        assert!((samples[0].rms - 0.0).abs() < 1e-6);
        assert!((samples[1].rms - 0.5).abs() < 1e-6);
        assert!((samples[2].rms - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn read_faces_flattens_per_frame_detections() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("clip.mov"), b"x").unwrap();
        let sidecar = dir.path().join("index/face/raw/clip.mov.json");
        write_sidecar(
            &sidecar,
            r#"{
                "indexer": "face",
                "data": {
                    "per_frame": [
                        {"t_s": 0.0, "faces": [
                            {"face_id": "f_0", "box": [10, 20, 30, 40]},
                            {"face_id": "f_1", "box": [50, 60, 30, 40]}
                        ]},
                        {"t_s": 0.2, "faces": [
                            {"face_id": "f_0", "box": [12, 22, 30, 40]}
                        ]}
                    ]
                }
            }"#,
        );
        let faces = read_faces(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert_eq!(faces.len(), 3);
        assert_eq!(faces[0].identity_id.as_deref(), Some("f_0"));
        assert!((faces[0].bbox.x - 10.0).abs() < 1e-6);
        assert_eq!(faces[2].identity_id.as_deref(), Some("f_0"));
    }

    #[tokio::test]
    async fn read_motion_regions_expands_magnitudes_by_window() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let asset = raw.join("clip.mov");
        std::fs::write(&asset, b"x").unwrap();
        let sidecar = motion_path_for(dir.path(), &asset);
        write_sidecar(
            &sidecar,
            r#"{"samples_per_second": 1, "magnitudes": [0.1, 0.5, 0.9]}"#,
        );
        let regions = read_motion_regions(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert_eq!(regions.len(), 3);
        assert!((regions[0].intensity - 0.1).abs() < 1e-6);
        assert!((regions[1].start_s - 1.0).abs() < 1e-6);
        assert!((regions[2].end_s - 3.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn read_color_stats_normalises_brightness() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("clip.mov"), b"x").unwrap();
        let sidecar = dir.path().join("index/color-analysis/raw/clip.mov.json");
        write_sidecar(
            &sidecar,
            r#"{
                "indexer": "color-analysis",
                "data": {
                    "scenes": [
                        {"scene_id": "color_scene_1", "start_s": 0.0, "end_s": 2.0, "brightness": 127.5, "saturation_mean": 51.0}
                    ]
                }
            }"#,
        );
        let stats = read_color_stats(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].id, "color_scene_1");
        assert!((stats[0].luma_mean.unwrap() - 0.5).abs() < 1e-3);
        assert!((stats[0].saturation_mean.unwrap() - 0.2).abs() < 1e-3);
    }

    #[tokio::test]
    async fn read_silences_returns_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let asset = raw.join("clip.mov");
        std::fs::write(&asset, b"x").unwrap();
        let sidecar = silences_path_for(dir.path(), &asset);
        write_sidecar(
            &sidecar,
            r#"{
                "ranges": [
                    {"start_s": 1.0, "end_s": 2.5, "db_floor": -45.0},
                    {"start_s": 5.0, "end_s": 6.0, "db_floor": -50.0}
                ],
                "threshold_db": -40.0,
                "min_duration_s": 0.6
            }"#,
        );
        let silences = read_silences(dir.path().to_string_lossy().into_owned(), "clip".into())
            .await
            .unwrap();
        assert_eq!(silences.len(), 2);
        assert!((silences[0].duration_s - 1.5).abs() < 1e-6);
        assert!((silences[1].start_s - 5.0).abs() < 1e-6);
    }
}
