#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use awidat_config::Config;
use awidat_index::perf_report::{
    PerfCommand, PerfMachine, PerfMedia, build_perf_report, to_markdown,
};
use awidat_index::{AssetInput, run};
use awidat_mcp::ClientInfo;
use awidat_proto::index::AssetId;

const DEFAULT_INDEXERS: &[&str] = &[
    "audio-energy",
    "beats",
    "face",
    "clip",
    "color-analysis",
    "frame-quality",
    "scenedetect",
    "gaze",
    "shot",
];

fn main() {
    if let Err(err) = real_main() {
        eprintln!("awidat-index-perf: {err}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1).collect())?;
    fs::create_dir_all(&args.output_dir)
        .map_err(|e| format!("create {}: {e}", args.output_dir.display()))?;
    let work_dirs = WorkDirs::create(&args.work_dir)?;

    let project_root = create_project(&args.asset, &args.label, &work_dirs.projects)?;
    let asset_id = format!(
        "external/{}",
        args.asset
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| format!("asset has no file name: {}", args.asset.display()))?
    );
    let staged_asset = project_root.join(&asset_id);
    let config =
        Config::load(Some(&project_root)).map_err(|e| format!("load indexer config: {e}"))?;
    let mut servers: Vec<_> = config
        .indexers()
        .filter(|server| args.indexers.iter().any(|name| name == &server.name))
        .cloned()
        .collect();
    apply_work_env(&mut servers, &work_dirs);
    servers.sort_by_key(|server| {
        args.indexers
            .iter()
            .position(|name| name == &server.name)
            .unwrap_or(usize::MAX)
    });
    if servers.len() != args.indexers.len() {
        let configured: Vec<_> = config
            .indexers()
            .map(|server| server.name.clone())
            .collect();
        return Err(format!(
            "configured indexers did not match requested list. requested={:?} configured={configured:?}",
            args.indexers
        ));
    }

    let assets = vec![AssetInput {
        id: AssetId::new(asset_id.clone()),
        path: staged_asset,
    }];
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("build runtime: {e}"))?;
    let report = runtime
        .block_on(run(
            &project_root,
            &servers,
            &assets,
            ClientInfo {
                name: "awidat-index-perf".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            args.concurrency,
            None,
        ))
        .map_err(|e| format!("dispatch indexers: {e}"))?;

    let run_dir = args
        .output_dir
        .join(format!("index-run-{}", timestamp_nanos()));
    copy_dir(&project_root.join("index"), &run_dir)
        .map_err(|e| format!("copy index sidecars to {}: {e}", run_dir.display()))?;

    let included_indexers: Vec<String> = servers.iter().map(|server| server.name.clone()).collect();
    let perf = build_perf_report(
        args.label.clone(),
        PerfCommand {
            project_root: project_root.display().to_string(),
            output_dir: args.output_dir.display().to_string(),
            concurrency: args.concurrency,
            included_indexers,
            assets: vec![asset_id],
        },
        PerfMachine {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            parallelism: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
        },
        probe_media(&args.asset),
        &report,
        &project_root,
    );

    let json_path = args
        .output_dir
        .join(format!("{}-indexing-performance.json", args.label));
    let md_path = args
        .output_dir
        .join(format!("{}-indexing-performance.md", args.label));
    let json = serde_json::to_vec_pretty(&perf).map_err(|e| format!("serialize report: {e}"))?;
    fs::write(&json_path, json).map_err(|e| format!("write {}: {e}", json_path.display()))?;
    fs::write(&md_path, to_markdown(&perf))
        .map_err(|e| format!("write {}: {e}", md_path.display()))?;
    println!("{}", md_path.display());
    Ok(())
}

#[derive(Debug)]
struct Args {
    asset: PathBuf,
    output_dir: PathBuf,
    work_dir: PathBuf,
    label: String,
    concurrency: usize,
    indexers: Vec<String>,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, String> {
        let mut asset = None;
        let mut output_dir = PathBuf::from("artifacts/perf/indexing-optimization");
        let mut work_dir = std::env::temp_dir().join("awidat-index-perf");
        let mut label = "baseline".to_string();
        let mut concurrency = 2_usize;
        let mut indexers: Vec<String> =
            DEFAULT_INDEXERS.iter().map(|name| (*name).into()).collect();
        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--asset" => {
                    i += 1;
                    asset = raw.get(i).map(PathBuf::from);
                }
                "--output-dir" => {
                    i += 1;
                    output_dir = raw
                        .get(i)
                        .map(PathBuf::from)
                        .ok_or_else(|| "--output-dir requires a value".to_string())?;
                }
                "--work-dir" => {
                    i += 1;
                    work_dir = raw
                        .get(i)
                        .map(PathBuf::from)
                        .ok_or_else(|| "--work-dir requires a value".to_string())?;
                }
                "--label" => {
                    i += 1;
                    label = raw
                        .get(i)
                        .cloned()
                        .ok_or_else(|| "--label requires a value".to_string())?;
                }
                "--concurrency" => {
                    i += 1;
                    let value = raw
                        .get(i)
                        .ok_or_else(|| "--concurrency requires a value".to_string())?;
                    concurrency = value
                        .parse::<usize>()
                        .map_err(|e| format!("invalid --concurrency {value:?}: {e}"))?;
                }
                "--indexers" => {
                    i += 1;
                    let value = raw
                        .get(i)
                        .ok_or_else(|| "--indexers requires a comma-separated value".to_string())?;
                    indexers = value
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unknown argument {other:?}\n{}", usage())),
            }
            i += 1;
        }
        let asset = asset.ok_or_else(usage)?;
        if !asset.is_file() {
            return Err(format!("asset is not a file: {}", asset.display()));
        }
        if indexers.is_empty() {
            return Err("--indexers cannot be empty".into());
        }
        Ok(Self {
            asset,
            output_dir,
            work_dir,
            label,
            concurrency,
            indexers,
        })
    }
}

fn usage() -> String {
    "usage: awidat-index-perf --asset <video> [--output-dir <dir>] [--work-dir <dir>] [--label baseline] [--concurrency 2] [--indexers a,b,c]".into()
}

#[derive(Debug)]
struct WorkDirs {
    projects: PathBuf,
    tmp: PathBuf,
    uv_cache: PathBuf,
}

impl WorkDirs {
    fn create(root: &Path) -> Result<Self, String> {
        let dirs = Self {
            projects: root.join("projects"),
            tmp: root.join("tmp"),
            uv_cache: root.join("uv-cache"),
        };
        for dir in [&dirs.projects, &dirs.tmp, &dirs.uv_cache] {
            fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        Ok(dirs)
    }
}

fn apply_work_env(servers: &mut [awidat_config::McpServer], work_dirs: &WorkDirs) {
    for server in servers {
        server
            .env
            .entry("TMPDIR".into())
            .or_insert_with(|| work_dirs.tmp.display().to_string());
        server
            .env
            .entry("TEMP".into())
            .or_insert_with(|| work_dirs.tmp.display().to_string());
        server
            .env
            .entry("TMP".into())
            .or_insert_with(|| work_dirs.tmp.display().to_string());
        server
            .env
            .entry("UV_CACHE_DIR".into())
            .or_insert_with(|| work_dirs.uv_cache.display().to_string());
    }
}

fn create_project(asset: &Path, label: &str, projects_dir: &Path) -> Result<PathBuf, String> {
    let root = projects_dir.join(format!(
        "awidat-index-perf-{label}-{}-{}",
        std::process::id(),
        timestamp_nanos()
    ));
    fs::create_dir_all(root.join("external")).map_err(|e| format!("create temp project: {e}"))?;
    fs::create_dir_all(root.join(".awidat")).map_err(|e| format!("create .awidat: {e}"))?;
    let dest = root.join("external").join(
        asset
            .file_name()
            .ok_or_else(|| format!("asset has no file name: {}", asset.display()))?,
    );
    symlink_file(asset, &dest)
        .map_err(|e| format!("symlink {} -> {}: {e}", dest.display(), asset.display()))?;
    Ok(root)
}

#[cfg(unix)]
fn symlink_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, dest)
}

#[cfg(not(unix))]
fn symlink_file(source: &Path, dest: &Path) -> std::io::Result<()> {
    fs::copy(source, dest).map(|_| ())
}

fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn copy_dir(source: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir(&source_path, &dest_path)?;
        } else {
            fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

fn probe_media(path: &Path) -> PerfMedia {
    let mut media = PerfMedia {
        path: path.display().to_string(),
        duration_s: None,
        video_codec: None,
        audio_codec: None,
        width: None,
        height: None,
        avg_frame_rate: None,
        size_bytes: path.metadata().ok().map(|m| m.len()),
        bit_rate: None,
    };
    let Ok(output) = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,size,bit_rate",
            "-show_entries",
            "stream=codec_type,codec_name,width,height,avg_frame_rate",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
    else {
        return media;
    };
    if !output.status.success() {
        return media;
    }
    let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return media;
    };
    if let Some(format) = doc.get("format") {
        media.duration_s = string_f64(format, "duration");
        media.size_bytes = string_u64(format, "size").or(media.size_bytes);
        media.bit_rate = string_u64(format, "bit_rate");
    }
    if let Some(streams) = doc.get("streams").and_then(serde_json::Value::as_array) {
        for stream in streams {
            match stream.get("codec_type").and_then(serde_json::Value::as_str) {
                Some("video") if media.video_codec.is_none() => {
                    media.video_codec = string_field(stream, "codec_name");
                    media.width = stream.get("width").and_then(serde_json::Value::as_u64);
                    media.height = stream.get("height").and_then(serde_json::Value::as_u64);
                    media.avg_frame_rate = string_field(stream, "avg_frame_rate");
                }
                Some("audio") if media.audio_codec.is_none() => {
                    media.audio_codec = string_field(stream, "codec_name");
                }
                _ => {}
            }
        }
    }
    media
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn string_u64(value: &serde_json::Value, key: &str) -> Option<u64> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<u64>().ok())
}

fn string_f64(value: &serde_json::Value, key: &str) -> Option<f64> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<f64>().ok())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use awidat_config::{IndexerResourceClass, McpServer, McpServerKind};

    use super::*;

    #[test]
    fn parses_explicit_work_dir() {
        let asset = tiny_asset("parse-work-dir");

        let args = Args::parse(vec![
            "--asset".into(),
            asset.display().to_string(),
            "--work-dir".into(),
            "/bench/work".into(),
            "--output-dir".into(),
            "/bench/out".into(),
            "--label".into(),
            "fresh".into(),
            "--concurrency".into(),
            "4".into(),
            "--indexers".into(),
            "clip,face".into(),
        ])
        .expect("args parse");

        assert_eq!(args.asset, asset);
        assert_eq!(args.work_dir, PathBuf::from("/bench/work"));
        assert_eq!(args.output_dir, PathBuf::from("/bench/out"));
        assert_eq!(args.label, "fresh");
        assert_eq!(args.concurrency, 4);
        assert_eq!(args.indexers, vec!["clip", "face"]);
    }

    #[test]
    fn default_indexer_order_matches_measured_pass2_order() {
        let asset = tiny_asset("default-order");

        let args =
            Args::parse(vec!["--asset".into(), asset.display().to_string()]).expect("args parse");

        assert_eq!(
            args.indexers,
            vec![
                "audio-energy",
                "beats",
                "face",
                "clip",
                "color-analysis",
                "frame-quality",
                "scenedetect",
                "gaze",
                "shot",
            ]
        );
    }

    #[test]
    fn applies_work_env_without_overwriting_configured_values() {
        let work_dirs = WorkDirs {
            projects: PathBuf::from("/bench/work/projects"),
            tmp: PathBuf::from("/bench/work/tmp"),
            uv_cache: PathBuf::from("/bench/work/uv-cache"),
        };
        let mut servers = vec![McpServer {
            name: "clip".into(),
            command: "uv".into(),
            args: vec![],
            env: HashMap::from([("TMPDIR".into(), "/custom/tmp".into())]),
            cwd: None,
            kind: McpServerKind::Indexer,
            enabled: true,
            depends_on: vec![],
            resource_class: IndexerResourceClass::Light,
            indexer_group: None,
        }];

        apply_work_env(&mut servers, &work_dirs);

        assert_eq!(servers[0].env.get("TMPDIR"), Some(&"/custom/tmp".into()));
        assert_eq!(servers[0].env.get("TEMP"), Some(&"/bench/work/tmp".into()));
        assert_eq!(servers[0].env.get("TMP"), Some(&"/bench/work/tmp".into()));
        assert_eq!(
            servers[0].env.get("UV_CACHE_DIR"),
            Some(&"/bench/work/uv-cache".into())
        );
    }

    fn tiny_asset(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "awidat-index-perf-{label}-{}-{}.mp4",
            std::process::id(),
            timestamp_nanos()
        ));
        fs::write(&path, b"not a real video").expect("write tiny asset");
        path
    }
}
