use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use montage_config::{Config, McpServer};
use montage_index::AssetInput;
use montage_index::perf_report::PerformanceReport;
use montage_mcp::ClientInfo;
use montage_proto::index::AssetId;
use serde::Serialize;

use crate::index_cmd;

pub struct IndexPerfArgs {
    pub project_root: PathBuf,
    pub assets: Vec<PathBuf>,
    pub indexers: Vec<String>,
    pub exclude_indexers: Vec<String>,
    pub include_whisper: bool,
    pub output: Option<PathBuf>,
    pub concurrency: usize,
}

#[derive(Debug, Serialize)]
struct IndexPerfEnvelope {
    command: CommandMetadata,
    machine: MachineMetadata,
    report: PerformanceReport,
}

#[derive(Debug, Serialize)]
struct CommandMetadata {
    project_root: String,
    output_dir: String,
    concurrency: usize,
    requested_indexers: Vec<String>,
    excluded_indexers: Vec<String>,
    included_indexers: Vec<String>,
    assets: Vec<String>,
}

#[derive(Debug, Serialize)]
struct MachineMetadata {
    profile: String,
    os: String,
    arch: String,
    parallelism: usize,
}

pub fn run(args: IndexPerfArgs) -> Result<()> {
    if !args.project_root.is_dir() {
        return Err(anyhow!(
            "project root '{}' is not a directory",
            args.project_root.display()
        ));
    }

    let config = Config::load(Some(&args.project_root))
        .with_context(|| "failed to load montage config (global and/or project)")?;
    let configured_servers = config.indexers().cloned().collect::<Vec<_>>();
    let servers = select_indexers(
        configured_servers.clone(),
        &args.indexers,
        &args.exclude_indexers,
        args.include_whisper,
    )?;
    let assets = collect_perf_assets(&args.project_root, &args.assets)?;
    if assets.is_empty() {
        return Err(anyhow!(
            "no assets to index. Drop source files under '{}' or pass --asset.",
            args.project_root.join("raw").display()
        ));
    }

    let output_dir = args
        .output
        .clone()
        .unwrap_or_else(|| args.project_root.join("reports/indexing-performance"));
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("create output dir '{}'", output_dir.display()))?;
    let measurement_index_dir = measurement_index_dir(&output_dir);

    let client_info = ClientInfo {
        name: "montage-index-perf".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let index_report = runtime
        .block_on(montage_index::run_with_index_dir(
            &args.project_root,
            &measurement_index_dir,
            &servers,
            &assets,
            client_info,
            args.concurrency,
            None,
        ))
        .context("indexer dispatcher failed")?;
    let perf_report = PerformanceReport::from_index_report(&index_report);
    let envelope = IndexPerfEnvelope {
        command: CommandMetadata {
            project_root: args.project_root.display().to_string(),
            output_dir: output_dir.display().to_string(),
            concurrency: args.concurrency,
            requested_indexers: args.indexers.clone(),
            excluded_indexers: expanded_exclusions(
                &configured_servers,
                &default_exclusions(&args.exclude_indexers, args.include_whisper),
            ),
            included_indexers: servers.iter().map(|server| server.name.clone()).collect(),
            assets: assets.iter().map(|asset| asset.id.to_string()).collect(),
        },
        machine: machine_metadata(),
        report: perf_report,
    };

    let json_path = output_dir.join("indexing-performance.json");
    let md_path = output_dir.join("indexing-performance.md");
    std::fs::write(&json_path, serde_json::to_vec_pretty(&envelope)?)
        .with_context(|| format!("write '{}'", json_path.display()))?;
    std::fs::write(&md_path, render_markdown(&envelope))
        .with_context(|| format!("write '{}'", md_path.display()))?;

    println!("wrote {}", json_path.display());
    println!("wrote {}", md_path.display());
    if index_report.has_failures() {
        Err(anyhow!(
            "{} indexer pair(s) failed; report written for review",
            index_report.failures().count()
        ))
    } else {
        Ok(())
    }
}

fn select_indexers(
    mut servers: Vec<McpServer>,
    requested: &[String],
    excluded: &[String],
    include_whisper: bool,
) -> Result<Vec<McpServer>> {
    if servers.is_empty() {
        return Err(anyhow!(
            "no indexers configured. Add `[[mcp.servers]]` entries with kind = \"indexer\" \
             to your `<project>/.montage/config.toml` or `~/.config/montage/config.toml`."
        ));
    }
    if !requested.is_empty() {
        servers.retain(|server| requested.iter().any(|name| name == &server.name));
    }
    let exclusions = expanded_exclusions(&servers, &default_exclusions(excluded, include_whisper));
    servers.retain(|server| !exclusions.iter().any(|name| name == &server.name));
    if servers.is_empty() {
        return Err(anyhow!(
            "index-perf selected no indexers after filters. requested={requested:?} excluded={exclusions:?}"
        ));
    }
    Ok(servers)
}

fn collect_perf_assets(
    project_root: &std::path::Path,
    explicit: &[PathBuf],
) -> Result<Vec<AssetInput>> {
    if explicit.is_empty() {
        return index_cmd::collect_assets(project_root, explicit);
    }

    explicit
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let abs = if path.is_absolute() {
                path.clone()
            } else {
                project_root.join(path)
            };
            let id = match abs.strip_prefix(project_root) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => format!("external/{:04}-{}", index + 1, safe_file_name(&abs)),
            };
            Ok(AssetInput {
                id: AssetId::new(id),
                path: abs,
            })
        })
        .collect()
}

fn safe_file_name(path: &std::path::Path) -> String {
    let raw = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let safe = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "asset".into()
    } else {
        safe
    }
}

fn default_exclusions(excluded: &[String], include_whisper: bool) -> Vec<String> {
    let mut out = excluded.to_vec();
    if !include_whisper && !out.iter().any(|name| name == "whisper") {
        out.push("whisper".into());
    }
    out.sort();
    out.dedup();
    out
}

fn expanded_exclusions(servers: &[McpServer], base: &[String]) -> Vec<String> {
    let mut excluded = base.to_vec();
    loop {
        let mut changed = false;
        for server in servers {
            if excluded.iter().any(|name| name == &server.name) {
                continue;
            }
            if server
                .depends_on
                .iter()
                .any(|dep| excluded.iter().any(|name| name == dep))
            {
                excluded.push(server.name.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    excluded.sort();
    excluded.dedup();
    excluded
}

static MEASUREMENT_INDEX_COUNTER: AtomicU64 = AtomicU64::new(0);

fn measurement_index_dir(output_dir: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = MEASUREMENT_INDEX_COUNTER.fetch_add(1, Ordering::Relaxed);
    output_dir.join(format!("index-run-{ts}-{pid}-{seq}"))
}

fn machine_metadata() -> MachineMetadata {
    let parallelism = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1);
    let profile = if parallelism >= 8 {
        "powerful"
    } else {
        "average"
    };
    MachineMetadata {
        profile: profile.into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        parallelism,
    }
}

fn render_markdown(envelope: &IndexPerfEnvelope) -> String {
    let summary = &envelope.report.summary;
    let mut out = String::new();
    out.push_str("# Indexing Performance Report\n\n");
    out.push_str(&format!(
        "- Project: `{}`\n- Machine: {} ({} {}, {} worker threads)\n- Concurrency: {}\n- Assets: {}\n- Indexers: {}\n\n",
        envelope.command.project_root,
        envelope.machine.profile,
        envelope.machine.os,
        envelope.machine.arch,
        envelope.machine.parallelism,
        envelope.command.concurrency,
        envelope.command.assets.join(", "),
        envelope.command.included_indexers.join(", "),
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Pairs: {}\n- Wrote: {}\n- Skipped: {}\n- Failed: {}\n- Blocked by dependency: {}\n- Budget violations: {}\n- Slowest pair: {} ms\n\n",
        summary.pair_count,
        summary.wrote,
        summary.skipped,
        summary.failed,
        summary.dep_skipped,
        summary.budget_violations,
        summary.max_total_ms,
    ));
    out.push_str("## Pair Timings\n\n");
    out.push_str("| Indexer | Asset | Outcome | Total ms | Tool ms | Launch ms | Queue ms | Write ms | Budget |\n");
    out.push_str("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for row in &envelope.report.pairs {
        let budget = if row.status.all_ok() {
            "pass"
        } else {
            "review"
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            row.indexer,
            row.asset_id,
            row.outcome,
            row.measured.total_ms,
            row.measured.tool_ms,
            row.measured.launch_init_ms,
            row.measured.queued_ms,
            row.measured.write_ms,
            budget,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use montage_config::{IndexerGroup, IndexerResourceClass, McpServerKind};

    use super::*;

    fn server(name: &str) -> McpServer {
        server_with_deps(name, &[])
    }

    fn server_with_deps(name: &str, depends_on: &[&str]) -> McpServer {
        McpServer {
            name: name.into(),
            command: "false".into(),
            args: Vec::new(),
            env: Default::default(),
            cwd: None,
            kind: McpServerKind::Indexer,
            enabled: true,
            depends_on: depends_on.iter().map(|name| (*name).into()).collect(),
            resource_class: IndexerResourceClass::Light,
            indexer_group: Some(IndexerGroup::Navigation),
        }
    }

    #[test]
    fn index_perf_excludes_whisper_by_default() {
        let selected = select_indexers(
            vec![
                server("whisper"),
                server_with_deps("topic", &["whisper"]),
                server_with_deps("editorial-moments", &["whisper", "topic"]),
                server("audio-energy"),
                server("scenedetect"),
            ],
            &[],
            &[],
            false,
        )
        .unwrap();

        let names = selected
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["audio-energy", "scenedetect"]);
    }

    #[test]
    fn explicit_whisper_exclusion_removes_transitive_dependents() {
        let selected = select_indexers(
            vec![
                server("whisper"),
                server_with_deps("topic", &["whisper"]),
                server_with_deps("editorial-moments", &["whisper", "topic"]),
                server("scenedetect"),
            ],
            &[],
            &["whisper".into()],
            true,
        )
        .unwrap();

        let names = selected
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["scenedetect"]);
    }

    #[test]
    fn index_perf_can_include_whisper_explicitly() {
        let selected = select_indexers(vec![server("whisper")], &[], &[], true).unwrap();

        assert_eq!(selected[0].name, "whisper");
    }

    #[test]
    fn markdown_includes_millisecond_columns_and_budget_status() {
        let report = PerformanceReport::from_index_report(&montage_index::IndexReport {
            outcomes: vec![montage_index::PairOutcome::Skipped {
                indexer: "topic".into(),
                asset: montage_proto::index::AssetId::new("raw/a.mp4"),
                telemetry: montage_index::PairTelemetry {
                    queued: std::time::Duration::from_millis(1),
                    launch_init: std::time::Duration::from_millis(2),
                    tool: std::time::Duration::from_millis(16_000),
                    write: std::time::Duration::from_millis(3),
                    total: std::time::Duration::from_millis(29_000),
                    peak_rss_bytes: None,
                },
            }],
        });
        let envelope = IndexPerfEnvelope {
            command: CommandMetadata {
                project_root: "/tmp/project".into(),
                output_dir: "/tmp/project/reports/indexing-performance".into(),
                concurrency: 2,
                requested_indexers: Vec::new(),
                excluded_indexers: vec!["whisper".into()],
                included_indexers: vec!["topic".into()],
                assets: vec!["raw/a.mp4".into()],
            },
            machine: MachineMetadata {
                profile: "average".into(),
                os: "macos".into(),
                arch: "aarch64".into(),
                parallelism: 4,
            },
            report,
        };

        let md = render_markdown(&envelope);

        assert!(md.contains("Total ms"));
        assert!(
            md.contains("| topic | raw/a.mp4 | skipped | 29000 | 16000 | 2 | 1 | 3 | review |")
        );
    }

    #[test]
    fn explicit_external_assets_get_safe_project_relative_ids() {
        let dir = tempfile::tempdir().unwrap();
        let assets = collect_perf_assets(
            dir.path(),
            &[PathBuf::from("/Volumes/Media/My Clip (Final).mp4")],
        )
        .unwrap();

        assert_eq!(assets[0].id.as_str(), "external/0001-My_Clip__Final_.mp4");
        assert!(assets[0].id.sidecar_relative_path().is_some());
    }

    #[test]
    fn measurement_index_dir_is_unique_and_report_scoped() {
        let project = tempfile::tempdir().unwrap();
        let output = project.path().join("reports/indexing-performance");

        let first = measurement_index_dir(&output);
        let second = measurement_index_dir(&output);

        assert!(first.starts_with(&output));
        assert!(second.starts_with(&output));
        assert_ne!(first, project.path().join("index"));
        assert_ne!(first, second);
    }
}
