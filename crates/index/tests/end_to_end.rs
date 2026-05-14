//! End-to-end integration test: real `audio-energy-mcp` Python indexer
//! launched via `uv run`, against a tiny fixture WAV.
//!
//! `#[ignore]` by default because it depends on the python workspace being
//! set up (`uv sync --all-packages` in `python/`). The CI matrix runs it
//! explicitly with `cargo test -- --ignored` after `uv sync` succeeds.
//!
//! We pick `audio-energy` because it's the lightest indexer — no model
//! downloads, no HF token, no network. Whisper / topic / scenedetect are
//! validated by hand smoke (`SMOKE.md`); their wiring shape is identical
//! to audio-energy's, so this test exercises the full dispatch path.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use awidat_config::{McpServer, McpServerKind};
use awidat_index::{AssetInput, PairOutcome, run};
use awidat_mcp::ClientInfo;
use awidat_proto::index::AssetId;
use awidat_proto::project::Project;

fn workspace_python_dir() -> PathBuf {
    // crates/index/tests/end_to_end.rs → workspace root
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // workspace
    p.push("python");
    p
}

fn fixture_wav() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("tone-2s.wav")
}

fn audio_energy_server(python_dir: &Path) -> McpServer {
    McpServer {
        name: "audio-energy".into(),
        command: which_uv(),
        args: vec![
            "run".into(),
            "--package".into(),
            "audio-energy-mcp".into(),
            "audio-energy-mcp".into(),
        ],
        env: HashMap::new(),
        cwd: Some(python_dir.to_path_buf()),
        kind: McpServerKind::Indexer,
        enabled: true,
        depends_on: vec![],
        resource_class: awidat_config::IndexerResourceClass::Light,
        indexer_group: None,
    }
}

/// Locate the `uv` binary. We don't assume it's on PATH (cargo test inside
/// some environments has a stripped PATH).
fn which_uv() -> String {
    if let Ok(path) = std::env::var("UV") {
        return path;
    }
    let home_local = format!(
        "{}/.local/bin/uv",
        std::env::var("HOME").unwrap_or_default()
    );
    if Path::new(&home_local).exists() {
        return home_local;
    }
    "uv".into()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires python workspace synced (uv sync --all-packages in python/)"]
async fn audio_energy_indexer_writes_sidecar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project_root = dir.path().to_path_buf();
    Project::init(&project_root).expect("project init");

    // Stage the fixture asset under <project>/raw/.
    let raw_dir = project_root.join("raw");
    std::fs::create_dir_all(&raw_dir).unwrap();
    let asset_path = raw_dir.join("tone-2s.wav");
    std::fs::copy(fixture_wav(), &asset_path).expect("copy fixture");

    let assets = vec![AssetInput {
        id: AssetId::new("raw/tone-2s.wav"),
        path: asset_path.clone(),
    }];
    let servers = vec![audio_energy_server(&workspace_python_dir())];
    let client_info = ClientInfo {
        name: "awidat-test".into(),
        version: "0.0.1".into(),
    };

    let report = run(&project_root, &servers, &assets, client_info, 2, None)
        .await
        .expect("dispatcher must succeed");

    // One pair, must be Wrote.
    assert_eq!(report.outcomes.len(), 1, "outcomes: {:?}", report.outcomes);
    let written_path = match &report.outcomes[0] {
        PairOutcome::Wrote { path, .. } => path.clone(),
        other => panic!("expected Wrote, got {other:?}"),
    };

    // Sidecar landed at the canonical path.
    let expected_path = project_root
        .join("index")
        .join("audio-energy")
        .join("raw")
        .join("tone-2s.wav.json");
    assert_eq!(written_path, expected_path);
    assert!(expected_path.exists(), "sidecar file must exist on disk");

    // Body parses, has the expected shape.
    let body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).expect("sidecar JSON");
    assert_eq!(body["indexer"], "audio-energy");
    assert_eq!(body["asset_id"], "raw/tone-2s.wav");
    assert!(
        body["data"]["windows"].is_array(),
        "data.windows must be an array, got: {body}"
    );
    assert!(
        body["data"]["windows"].as_array().unwrap().len() >= 15,
        "expected ≥15 windows for a 2s file at 100ms windows"
    );
    let sr = body["data"]["sample_rate"].as_u64().expect("sample_rate");
    assert_eq!(sr, 48_000);

    // Manifest was updated.
    let manifest_path = project_root.join("index").join("manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).expect("manifest JSON");
    let indexers = manifest["indexers"].as_array().expect("indexers array");
    assert_eq!(indexers.len(), 1);
    assert_eq!(indexers[0]["name"], "audio-energy");
    assert_eq!(
        indexers[0]["assets"].as_array().unwrap()[0],
        "raw/tone-2s.wav"
    );

    // Re-run is a no-op (Skipped).
    let report2 = run(
        &project_root,
        &servers,
        &assets,
        ClientInfo {
            name: "awidat-test".into(),
            version: "0.0.1".into(),
        },
        2,
        None,
    )
    .await
    .expect("second run must succeed");
    assert!(
        matches!(report2.outcomes[0], PairOutcome::Skipped { .. }),
        "second run must skip; got {:?}",
        report2.outcomes[0]
    );
}
