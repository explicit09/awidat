# Auto-Indexing Performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every media-bin import enqueue scoped, staged indexing immediately, while generated B-roll receives lightweight semantic sidecars and machine-aware scheduling keeps average laptops responsive.

**Architecture:** Keep the existing `montage_index::run` dispatcher as the execution engine. Add a desktop indexing planner around it that resolves explicit imported asset IDs, selects indexer tiers, applies machine profile policy, and falls back to whole-project indexing when scoped resolution is unreliable. Generated media uses the existing `.montage/generated-media/registry.json` as source of truth and writes an `index/generated-description/<asset>.json` sidecar when a generated video reaches success.

**Tech Stack:** Rust workspace, Tauri desktop commands, `montage-index` dispatcher, `montage-config` indexer metadata, `montage-core` generated-media registry, focused Rust unit tests.

---

## File Structure

- Modify `apps/desktop/src-tauri/src/commands/index.rs`: add scoped asset resolution, `IndexMode`, tier/profile planner, and a new `index_project_assets_at_root` entrypoint that wraps the existing dispatcher.
- Modify `apps/desktop/src-tauri/src/commands/import.rs`: pass imported asset IDs into the scoped index entrypoint instead of invoking whole-project indexing after every import batch.
- Modify `crates/core/src/generated_media/registry.rs`: add generated-description sidecar writer helpers using existing registry records.
- Modify `crates/core/src/montage_mcp/tools/start_generated_media_job.rs`: write description sidecars for mock generated media that is immediately succeeded.
- Modify `crates/core/src/montage_mcp/tools/poll_generated_media_job.rs`: write description sidecars when OpenRouter polling transitions a generated media record to `Succeeded`.
- Add focused tests in the same Rust modules. Use existing in-module test style and avoid broad desktop or Python smoke tests.

---

### Task 1: Scoped Desktop Indexing Entry Point

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/index.rs`

- [ ] **Step 1: Write failing scoped asset resolution tests**

Add these tests inside `apps/desktop/src-tauri/src/commands/index.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn resolve_scoped_assets_uses_only_requested_raw_assets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("raw/nested")).unwrap();
    std::fs::write(dir.path().join("raw/a.mov"), b"a").unwrap();
    std::fs::write(dir.path().join("raw/nested/b.wav"), b"b").unwrap();
    std::fs::write(dir.path().join("raw/other.mp4"), b"c").unwrap();

    let assets = resolve_scoped_assets(
        dir.path(),
        &["raw/nested/b.wav".to_string(), "raw/a.mov".to_string()],
    )
    .unwrap();

    let ids: Vec<String> = assets.iter().map(|asset| asset.id.to_string()).collect();
    assert_eq!(ids, vec!["raw/nested/b.wav", "raw/a.mov"]);
    assert_eq!(assets[0].path, dir.path().join("raw/nested/b.wav"));
    assert_eq!(assets[1].path, dir.path().join("raw/a.mov"));
}

#[test]
fn resolve_scoped_assets_rejects_non_raw_or_missing_assets() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("raw")).unwrap();
    std::fs::write(dir.path().join("raw/a.mov"), b"a").unwrap();

    let outside = resolve_scoped_assets(dir.path(), &["renders/a.mp4".to_string()]).unwrap_err();
    assert!(outside.contains("must be under raw/"), "{outside}");

    let missing = resolve_scoped_assets(dir.path(), &["raw/missing.mov".to_string()]).unwrap_err();
    assert!(missing.contains("missing scoped asset"), "{missing}");
}

#[test]
fn resolve_assets_for_request_falls_back_to_all_raw_assets_when_scoped_input_is_bad() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("raw")).unwrap();
    std::fs::write(dir.path().join("raw/a.mov"), b"a").unwrap();
    std::fs::write(dir.path().join("raw/b.mov"), b"b").unwrap();

    let resolved = resolve_assets_for_request(
        dir.path(),
        Some(&["../escape.mov".to_string()]),
        ScopedFallback::WholeProject,
    )
    .unwrap();

    let ids: Vec<String> = resolved.assets.iter().map(|asset| asset.id.to_string()).collect();
    assert_eq!(ids, vec!["raw/a.mov", "raw/b.mov"]);
    assert!(matches!(resolved.scope, ResolvedIndexScope::FallbackAll { .. }));
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test -p montage-desktop --lib resolve_scoped_assets -- --nocapture
```

Expected: compilation fails because `resolve_scoped_assets`, `resolve_assets_for_request`, `ScopedFallback`, and `ResolvedIndexScope` do not exist.

- [ ] **Step 3: Add scoped resolution types and helpers**

In `apps/desktop/src-tauri/src/commands/index.rs`, add these items near `collect_assets`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopedFallback {
    WholeProject,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedIndexScope {
    All,
    Scoped,
    FallbackAll { reason: String },
}

#[derive(Debug)]
struct ResolvedIndexAssets {
    assets: Vec<AssetInput>,
    scope: ResolvedIndexScope,
}

fn resolve_assets_for_request(
    project_root: &Path,
    scoped_asset_ids: Option<&[String]>,
    fallback: ScopedFallback,
) -> Result<ResolvedIndexAssets, String> {
    let Some(ids) = scoped_asset_ids else {
        return collect_assets(project_root)
            .map(|assets| ResolvedIndexAssets {
                assets,
                scope: ResolvedIndexScope::All,
            })
            .map_err(|e| format!("scan raw/: {e}"));
    };
    if ids.is_empty() {
        return collect_assets(project_root)
            .map(|assets| ResolvedIndexAssets {
                assets,
                scope: ResolvedIndexScope::All,
            })
            .map_err(|e| format!("scan raw/: {e}"));
    }

    match resolve_scoped_assets(project_root, ids) {
        Ok(assets) => Ok(ResolvedIndexAssets {
            assets,
            scope: ResolvedIndexScope::Scoped,
        }),
        Err(e) if fallback == ScopedFallback::WholeProject => {
            let assets = collect_assets(project_root).map_err(|scan| {
                format!("scoped asset resolution failed ({e}); fallback scan raw/ failed: {scan}")
            })?;
            Ok(ResolvedIndexAssets {
                assets,
                scope: ResolvedIndexScope::FallbackAll { reason: e },
            })
        }
        Err(e) => Err(e),
    }
}

fn resolve_scoped_assets(project_root: &Path, asset_ids: &[String]) -> Result<Vec<AssetInput>, String> {
    let mut out = Vec::with_capacity(asset_ids.len());
    for id in asset_ids {
        let clean = id.trim().replace('\\', "/");
        if clean.is_empty() || clean.starts_with('/') || clean.contains("..") {
            return Err(format!("unsafe scoped asset id: {id}"));
        }
        if !clean.starts_with("raw/") {
            return Err(format!("scoped asset id must be under raw/: {id}"));
        }
        let path = project_root.join(&clean);
        if !path.is_file() {
            return Err(format!("missing scoped asset: {id}"));
        }
        out.push(AssetInput {
            id: montage_proto::index::AssetId::new(clean),
            path,
        });
    }
    Ok(out)
}
```

- [ ] **Step 4: Run tests and verify they pass**

Run:

```bash
cargo test -p montage-desktop --lib resolve_scoped_assets -- --nocapture
```

Expected: all scoped resolution tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/index.rs
git commit -m "feat(indexing): resolve scoped desktop index assets"
```

---

### Task 2: Post-Import Uses Scoped Asset IDs

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/import.rs`
- Modify: `apps/desktop/src-tauri/src/commands/index.rs`

- [ ] **Step 1: Write failing tests for project-relative imported IDs**

Add tests inside `apps/desktop/src-tauri/src/commands/import.rs`'s existing test module:

```rust
#[test]
fn project_relative_asset_ids_preserve_raw_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("raw/nested/clip.mov");
    std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
    std::fs::write(&asset, b"media").unwrap();

    let ids = project_relative_asset_ids(dir.path(), &[asset]).unwrap();

    assert_eq!(ids, vec!["raw/nested/clip.mov".to_string()]);
}

#[test]
fn project_relative_asset_ids_reject_paths_outside_project() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();

    let err = project_relative_asset_ids(dir.path(), &[outside.path().to_path_buf()]).unwrap_err();

    assert!(err.contains("outside project root"), "{err}");
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p montage-desktop --lib project_relative_asset_ids -- --nocapture
```

Expected: compilation fails because `project_relative_asset_ids` does not exist.

- [ ] **Step 3: Add imported asset ID helper**

In `apps/desktop/src-tauri/src/commands/import.rs`, add this helper near `run_local_import`:

```rust
fn project_relative_asset_ids(
    project_root: &std::path::Path,
    assets: &[PathBuf],
) -> Result<Vec<String>, String> {
    assets
        .iter()
        .map(|asset| {
            asset
                .strip_prefix(project_root)
                .map_err(|_| format!("imported asset outside project root: {}", asset.display()))
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        })
        .collect()
}
```

- [ ] **Step 4: Add scoped index wrapper**

In `apps/desktop/src-tauri/src/commands/index.rs`, add this public function next to `index_project_at_root`:

```rust
pub async fn index_project_assets_at_root(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    project_root: std::path::PathBuf,
    asset_ids: Vec<String>,
) -> Result<(), String> {
    index_project_at_root_with_assets(app, state, project_root, Some(asset_ids)).await
}
```

Rename the body of `index_project_at_root` into:

```rust
async fn index_project_at_root_with_assets(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    project_root: std::path::PathBuf,
    scoped_asset_ids: Option<Vec<String>>,
) -> Result<(), String> {
    // existing body, with asset discovery replaced by resolve_assets_for_request
}
```

Keep the existing public `index_project_at_root` as:

```rust
pub async fn index_project_at_root(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    project_root: std::path::PathBuf,
) -> Result<(), String> {
    index_project_at_root_with_assets(app, state, project_root, None).await
}
```

Replace the current `let assets = collect_assets...` block with:

```rust
let resolved = resolve_assets_for_request(
    &project_root,
    scoped_asset_ids.as_deref(),
    ScopedFallback::WholeProject,
)?;
let assets = resolved.assets;
```

- [ ] **Step 5: Route post-import chain to scoped indexing**

In `spawn_post_import_chain_many`, replace the final whole-project call with:

```rust
let asset_ids = match project_relative_asset_ids(&project_root, &assets) {
    Ok(ids) => ids,
    Err(e) => {
        tracing::warn!(error = %e, "unable to derive imported asset ids; falling back to full auto-index");
        Vec::new()
    }
};
let result = if asset_ids.is_empty() {
    crate::commands::index::index_project_at_root(&app, &state, project_root.clone()).await
} else {
    crate::commands::index::index_project_assets_at_root(
        &app,
        &state,
        project_root.clone(),
        asset_ids,
    )
    .await
};
if let Err(e) = result {
    tracing::warn!(error = %e, "auto-index failed");
}
```

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p montage-desktop --lib project_relative_asset_ids -- --nocapture
cargo test -p montage-desktop --lib resolve_scoped_assets -- --nocapture
```

Expected: import helper and scoped resolution tests pass.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/import.rs apps/desktop/src-tauri/src/commands/index.rs
git commit -m "feat(indexing): auto-index imported asset ids"
```

---

### Task 3: Tier Planner and Machine Profile

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/index.rs`

- [ ] **Step 1: Write failing planner tests**

Add these tests in `apps/desktop/src-tauri/src/commands/index.rs`:

```rust
#[test]
fn fast_context_keeps_agent_critical_indexers_first() {
    let mut servers = vec![
        test_server("clip"),
        test_server("topic"),
        test_server("whisper"),
        test_server("audio-energy"),
        test_server("scenedetect"),
        test_server("editorial-moments"),
        test_server("frame-quality"),
    ];

    plan_indexers_for_mode(&mut servers, IndexMode::FastContext, MachineProfile::Average);

    let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["whisper", "audio-energy", "scenedetect", "topic", "editorial-moments"]
    );
}

#[test]
fn full_context_on_powerful_machine_retains_visual_indexers_after_semantic_work() {
    let mut servers = vec![
        test_server("clip"),
        test_server("topic"),
        test_server("whisper"),
        test_server("face"),
        test_server("gaze"),
        test_server("shot"),
        test_server("audio-energy"),
    ];

    plan_indexers_for_mode(&mut servers, IndexMode::FullContext, MachineProfile::Powerful);

    let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["whisper", "audio-energy", "topic", "face", "gaze", "shot", "clip"]
    );
}

#[test]
fn machine_profile_defaults_to_average_on_low_core_count() {
    assert_eq!(profile_for_signals(4, Some(0.2)), MachineProfile::Average);
    assert_eq!(profile_for_signals(8, Some(0.2)), MachineProfile::Powerful);
    assert_eq!(profile_for_signals(8, Some(8.0)), MachineProfile::Average);
    assert_eq!(profile_for_signals(8, None), MachineProfile::Average);
}
```

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p montage-desktop --lib fast_context_keeps_agent_critical_indexers_first -- --nocapture
```

Expected: compilation fails because `IndexMode`, `MachineProfile`, `plan_indexers_for_mode`, and `profile_for_signals` do not exist.

- [ ] **Step 3: Add planner types and tier filtering**

In `apps/desktop/src-tauri/src/commands/index.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexMode {
    FastContext,
    FullContext,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MachineProfile {
    Average,
    Powerful,
}

fn current_machine_profile() -> MachineProfile {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    profile_for_signals(cores, read_load_avg_1min())
}

fn profile_for_signals(cores: usize, load_1min: Option<f64>) -> MachineProfile {
    match load_1min {
        Some(load) if cores >= 8 && load < (cores as f64) * 0.5 => MachineProfile::Powerful,
        _ => MachineProfile::Average,
    }
}

fn plan_indexers_for_mode(
    servers: &mut Vec<McpServer>,
    mode: IndexMode,
    profile: MachineProfile,
) {
    prepare_desktop_indexers(servers);
    if matches!(mode, IndexMode::FastContext) {
        servers.retain(|server| {
            matches!(
                server.name.as_str(),
                "whisper" | "audio-energy" | "beats" | "scenedetect" | "topic" | "editorial-moments"
            )
        });
    }
    if matches!((mode, profile), (IndexMode::FastContext, MachineProfile::Average)) {
        servers.retain(|server| server.name != "beats");
    }
    servers.sort_by_key(|server| planned_indexer_priority(&server.name));
}

fn planned_indexer_priority(name: &str) -> u8 {
    match name {
        "whisper" => 0,
        "audio-energy" => 1,
        "beats" => 2,
        "scenedetect" => 3,
        "topic" => 4,
        "editorial-moments" => 5,
        "frame-quality" | "color-analysis" => 6,
        "face" => 7,
        "gaze" => 8,
        "shot" => 9,
        "clip" => 10,
        _ => 11,
    }
}
```

- [ ] **Step 4: Wire fast-context mode for auto-index**

In `index_project_at_root_with_assets`, after disabled indexers are filtered:

```rust
let mode = if scoped_asset_ids.is_some() {
    IndexMode::FastContext
} else {
    IndexMode::Manual
};
plan_indexers_for_mode(&mut servers, mode, current_machine_profile());
```

- [ ] **Step 5: Run planner tests**

Run:

```bash
cargo test -p montage-desktop --lib fast_context -- --nocapture
cargo test -p montage-desktop --lib full_context -- --nocapture
cargo test -p montage-desktop --lib machine_profile -- --nocapture
```

Expected: planner tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/commands/index.rs
git commit -m "feat(indexing): plan fast context tiers"
```

---

### Task 4: Generated Media Description Sidecars

**Files:**
- Modify: `crates/core/src/generated_media/registry.rs`
- Modify: `crates/core/src/montage_mcp/tools/start_generated_media_job.rs`
- Modify: `crates/core/src/montage_mcp/tools/poll_generated_media_job.rs`

- [ ] **Step 1: Write failing generated-description sidecar test**

Add this test to `crates/core/src/generated_media/registry.rs`:

```rust
#[test]
fn generated_description_sidecar_uses_registry_record() {
    let dir = tempfile::tempdir().unwrap();
    let record = GeneratedMediaRecord::new_mock_succeeded(
        "gen-1",
        "raw/generated/mock/gen-1.mp4",
        "slow orbit around a product on a clean desk",
    )
    .unwrap();

    write_generated_description_sidecar(dir.path(), &record).unwrap();

    let path = dir
        .path()
        .join("index/generated-description/raw/generated/mock/gen-1.mp4.json");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();

    assert_eq!(value["indexer"], "generated-description");
    assert_eq!(value["asset_id"], "raw/generated/mock/gen-1.mp4");
    assert_eq!(value["data"]["job_id"], "gen-1");
    assert_eq!(value["data"]["workflow_purpose"], "broll");
    assert!(value["data"]["visual_summary"].as_str().unwrap().contains("slow orbit"));
}
```

- [ ] **Step 2: Run test and verify it fails**

Run:

```bash
cargo test -p montage-core generated_description_sidecar_uses_registry_record -- --nocapture
```

Expected: compilation fails because `write_generated_description_sidecar` does not exist.

- [ ] **Step 3: Add sidecar writer**

In `crates/core/src/generated_media/registry.rs`, add:

```rust
pub fn write_generated_description_sidecar(
    project_root: &Path,
    record: &GeneratedMediaRecord,
) -> Result<(), RegistryError> {
    let Some(asset_id) = record.output_video_path() else {
        return Ok(());
    };
    let sidecar_path = project_root
        .join("index")
        .join("generated-description")
        .join(format!("{asset_id}.json"));
    if let Some(parent) = sidecar_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let produced_at = record
        .completed_at
        .unwrap_or(record.updated_at)
        .to_rfc3339();
    let body = serde_json::json!({
        "indexer": "generated-description",
        "indexer_version": "0.1.0",
        "schema_version": "1",
        "asset_id": asset_id,
        "asset_sha256": record.prompt_hash,
        "produced_at": produced_at,
        "data": {
            "job_id": record.job_id,
            "provider": record.provider,
            "model": record.model,
            "prompt": record.prompt,
            "prompt_hash": record.prompt_hash,
            "artifact_kind": record.artifact_kind,
            "workflow_purpose": record.workflow_purpose,
            "visual_summary": record.prompt,
            "intended_use": record.workflow_purpose,
            "created_at": record.created_at,
            "completed_at": record.completed_at,
            "requires_disclosure": record.requires_disclosure,
            "uses_likeness": record.uses_likeness,
            "provenance": "generated_media_registry"
        }
    });
    fs::write(sidecar_path, serde_json::to_vec_pretty(&body)?)?;
    Ok(())
}
```

- [ ] **Step 4: Write sidecar after generated media success**

In `start_generated_media_job.rs`, after `Registry::upsert` for the mock provider, add:

```rust
crate::generated_media::registry::write_generated_description_sidecar(&ctx.project_root, &record)
    .map_err(|e| format!("start_generated_media_job: generated description sidecar: {e}"))?;
```

In `poll_generated_media_job.rs`, after `registry.upsert(...)` when `record.state` is `Succeeded`, add:

```rust
if matches!(record.state, GeneratedMediaState::Succeeded) {
    crate::generated_media::registry::write_generated_description_sidecar(
        &ctx.project_root,
        &record,
    )
    .map_err(|e| format!("poll_generated_media_job: generated description sidecar: {e}"))?;
}
```

- [ ] **Step 5: Run generated media tests**

Run:

```bash
cargo test -p montage-core generated_description_sidecar -- --nocapture
cargo test -p montage-core start_generated_media_job -- --nocapture
cargo test -p montage-core poll_generated_media_job -- --nocapture
```

Expected: generated-description sidecar test and existing generated media tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/generated_media/registry.rs crates/core/src/montage_mcp/tools/start_generated_media_job.rs crates/core/src/montage_mcp/tools/poll_generated_media_job.rs
git commit -m "feat(indexing): describe generated media for agents"
```

---

### Task 5: Verify Add-to-Timeline Is Not an Index Trigger

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/index.rs`
- Modify: `apps/desktop/src/App.tsx` only if drag/drop-to-bin import handling is added in this branch.

- [ ] **Step 1: Search for timeline insertion indexing calls**

Run:

```bash
rg -n "insert_media_on_timeline|index_project|index_project_assets|start_indexing" apps/desktop/src apps/desktop/src-tauri/src crates/core/src/montage_mcp/tools
```

Expected: `insert_media_on_timeline` paths do not invoke `index_project`, `index_project_assets_at_root`, or `start_indexing`.

- [ ] **Step 2: Add a regression note to the plan evidence section**

Append this evidence block to `docs/superpowers/plans/2026-06-06-auto-indexing-performance.md` after Task 5 when implementing:

```markdown
## Verification Evidence

- `rg -n "insert_media_on_timeline|index_project|index_project_assets|start_indexing" ...` confirmed timeline insertion does not trigger indexing; import commands own auto-indexing.
```

- [ ] **Step 3: Commit if the plan file was updated**

```bash
git add docs/superpowers/plans/2026-06-06-auto-indexing-performance.md
git commit -m "docs(indexing): record timeline trigger verification"
```

## Verification Evidence

- `rg -n "insert_media_on_timeline|index_project|index_project_assets|start_indexing" apps/desktop/src apps/desktop/src-tauri/src crates/core/src/montage_mcp/tools` confirmed timeline insertion does not trigger indexing; import commands own auto-indexing.

---

### Task 6: Final Focused Verification

**Files:**
- No new files unless a preceding task left verification notes uncommitted.

- [ ] **Step 1: Run formatting**

Run:

```bash
cargo fmt --all -- --check
```

Expected: exits 0. Stable Rust may print `imports_granularity` warnings; those are acceptable if the exit code is 0.

- [ ] **Step 2: Run focused Rust tests**

Run:

```bash
cargo test -p montage-index
cargo test -p montage-core generated_description_sidecar -- --nocapture
cargo test -p montage-core generated_media -- --nocapture
cargo test -p montage-desktop --lib resolve_scoped_assets -- --nocapture
cargo test -p montage-desktop --lib project_relative_asset_ids -- --nocapture
cargo test -p montage-desktop --lib fast_context -- --nocapture
cargo test -p montage-desktop --lib machine_profile -- --nocapture
```

Expected: all selected tests pass. The ignored `montage-index` Python end-to-end test remains ignored unless the Python workspace is synced.

- [ ] **Step 3: Inspect final diff**

Run:

```bash
git status --short
git log --oneline -8
```

Expected: no uncommitted files except explicitly accepted follow-up notes. Recent commits should show the scoped indexing, post-import routing, tier planner, generated sidecar, and verification commits.

- [ ] **Step 4: Report remaining follow-up**

Report that shared sparse-frame/audio cache measurement remains the next performance project after this branch, because this implementation preserves existing indexer internals and focuses on trigger/scope/tier orchestration.
