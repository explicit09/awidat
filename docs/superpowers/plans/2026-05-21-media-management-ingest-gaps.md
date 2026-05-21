# Media Management Ingest Gaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the Media Management / Ingest gaps documented in `.reference-research/pro-editing-gap-analysis/01-media-management.md` without touching the user's current checkout.

**Architecture:** Keep durable media organization state in `Timeline.metadata.awidat`, where `asset_catalog`, `selects`, and `stringouts` already live. Add small agent tools in `crates/core/src/tools/` that mutate the existing project model through `Project::read` and `Project::write`, and move duplicated media scanning/proxy path logic into reusable library helpers. Keep desktop commands intact and share lower-level behavior where possible.

**Tech Stack:** Rust 2024 workspace, `awidat-proto` for project schema, `awidat-core` for tools, `awidat-index` for media walking, `awidat-render` for ffmpeg/proxy generation, existing `ToolHandler` approval gating.

---

## Baseline

- Worktree: `/Users/explicit/.config/superpowers/worktrees/awidat/media-management-ingest-gaps`
- Branch: `codex/media-management-ingest-gaps`
- Baseline command attempted: `cargo test --workspace`
- Baseline result: blocked by disk space before tests ran: `No space left on device (os error 28)`.
- Required before completion: free disk space, then run focused tests, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` where feasible.

## File Map

- Modify: `crates/proto/src/awidat_meta.rs`
  - Add focused helpers for `ensure_awidat_metadata`, catalog lookup, and validating bin parent references if needed.
- Modify: `crates/proto/src/professional.rs`
  - Extend validation for bin tree integrity: duplicate bin ids, self-parent, missing parent, and cycles.
- Create: `crates/index/src/media_files.rs`
  - Shared project media walker with extension filtering and ignored directory rules.
- Modify: `crates/index/src/lib.rs`
  - Export `media_files`.
- Modify: `crates/core/src/tools/list_assets.rs`
  - Use shared walker and optionally include bin metadata when catalog entries exist.
- Modify: `crates/core/src/tools/start_indexing.rs`
  - Replace local `raw/` walker with shared helper.
- Modify: `crates/core/src/tools/diagnose_project_media.rs`
  - Replace local project media walker with shared helper.
- Create: `crates/core/src/media_catalog_mutation.rs`
  - Shared project mutation helpers for catalog creation, asset upsert, bin operations, metadata changes, select creation, and timeline relink traversal.
- Create: `crates/core/src/proxy.rs`
  - Shared proxy path, status, manifest, and generation helpers callable from agent tools.
- Create: `crates/core/src/tools/import_media.rs`
  - Agent-callable `import_local` and `import_url`.
- Create: `crates/core/src/tools/proxy_media.rs`
  - Agent-callable `proxy_status` and `generate_proxy`.
- Create: `crates/core/src/tools/relink_media.rs`
  - Agent-callable `relink_media`.
- Create: `crates/core/src/tools/manage_assets.rs`
  - Agent-callable `create_bin`, `move_to_bin`, `rename_asset`, `tag_asset`, `rate_asset`, and `mark_select`.
- Create: `crates/core/src/tools/transcript_search.rs`
  - First-class transcript search over whisper sidecars.
- Modify: `crates/core/src/tools/mod.rs`
  - Export new tool modules.
- Modify: `apps/desktop/src-tauri/src/session.rs`
  - Register new tools in the desktop session registry.
- Modify: any CLI/TUI registry file that mirrors the desktop set.
  - Confirm with `rg "register\\(Arc::new" crates apps`.

## Task 1: Shared Media File Walker

**Files:**
- Create: `crates/index/src/media_files.rs`
- Modify: `crates/index/src/lib.rs`
- Modify: `crates/core/src/tools/list_assets.rs`
- Modify: `crates/core/src/tools/start_indexing.rs`
- Modify: `crates/core/src/tools/diagnose_project_media.rs`

- [ ] **Step 1: Write failing tests for the shared walker**

Add tests in `crates/index/src/media_files.rs` covering:

```rust
#[test]
fn collect_project_media_skips_generated_dirs() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path().join("raw/a.mp4"));
    write(dir.path().join("renders/out.mp4"));
    write(dir.path().join(".awidat/proxies/a.mp4"));
    write(dir.path().join("index/whisper/a.json"));

    let found = collect_project_media_files(dir.path(), MediaScanOptions::default()).unwrap();

    assert_eq!(relative_paths(dir.path(), &found), vec!["raw/a.mp4"]);
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo test -p awidat-index collect_project_media_skips_generated_dirs
```

Expected: fail because `media_files` does not exist yet.

- [ ] **Step 3: Implement `media_files`**

Implement `MediaScanOptions`, `MediaFile`, `collect_project_media_files`, `collect_raw_media_inputs`, `is_media_path`, and `is_ignored_scan_dir`. Keep extension rules centralized and include the extensions already used by `diagnose_project_media`.

- [ ] **Step 4: Replace duplicate walkers**

Use the shared walker in `list_assets`, `start_indexing`, and `diagnose_project_media`. Preserve existing pagination, scope labels, caps, and error behavior.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p awidat-index media_files
cargo test -p awidat-core list_assets start_indexing diagnose_project_media
```

## Task 2: Durable Catalog Mutation Helpers

**Files:**
- Modify: `crates/proto/src/professional.rs`
- Modify: `crates/proto/src/awidat_meta.rs`
- Create: `crates/core/src/media_catalog_mutation.rs`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Write validation tests**

Add tests that reject duplicate bin ids, missing parent ids, self-parent, and cycles.

- [ ] **Step 2: Implement validation**

Extend `AssetCatalog::validate()` to validate `AssetBin.parent_id`. Keep diagnostics warnings/errors consistent with existing `ProfessionalDiagnostic` patterns.

- [ ] **Step 3: Add mutation helper tests**

Test that helpers create `metadata.awidat` when absent, create `asset_catalog` when absent, preserve existing assets, upsert imported assets, and update `source_assets`.

- [ ] **Step 4: Implement helpers**

Expose focused functions:

```rust
pub fn ensure_awidat_metadata(timeline: &mut Timeline) -> &mut AwidatTimelineMetadata;
pub fn ensure_asset_catalog(meta: &mut AwidatTimelineMetadata) -> &mut AssetCatalog;
pub fn upsert_asset(meta: &mut AwidatTimelineMetadata, record: AssetRecord);
pub fn create_bin(meta: &mut AwidatTimelineMetadata, id: String, name: String, parent_id: Option<String>) -> Result<(), CatalogMutationError>;
pub fn move_asset_to_bin(meta: &mut AwidatTimelineMetadata, asset_id: &str, bin_id: Option<String>) -> Result<(), CatalogMutationError>;
```

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p awidat-proto professional awidat_meta
cargo test -p awidat-core media_catalog_mutation
```

## Task 3: Agent Import Tools

**Files:**
- Create: `crates/core/src/tools/import_media.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Modify: registries that construct `ToolRegistry`

- [ ] **Step 1: Write failing tool tests**

Cover local copy, local symlink, source path safety, URL import unavailable when `yt-dlp` is missing, and catalog/source asset updates.

- [ ] **Step 2: Implement `import_local`**

Inputs:

```json
{
  "source_path": "/absolute/path/to/media.mp4",
  "destination_name": "optional-name.mp4",
  "link": false
}
```

Behavior: validate source exists and is a file, create `raw/`, copy or symlink into `raw/`, fail loudly on name collision unless an identical existing path is intentionally reused, update `metadata.awidat.source_assets` and `asset_catalog`, then return structured JSON with project-relative path and size.

- [ ] **Step 3: Implement `import_url`**

Inputs:

```json
{
  "url": "https://example.com/video",
  "destination_name": "optional-name.mp4"
}
```

Behavior: use the same `yt-dlp` command strategy as desktop import where practical, write into `raw/`, update project metadata, and return structured JSON.

- [ ] **Step 4: Mark mutating and approval-scoped**

`is_mutating` returns true. `approval_keys` should bucket by source path or URL, not by tool name alone.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p awidat-core import_media
```

## Task 4: Agent Proxy Status and Generation

**Files:**
- Create: `crates/core/src/proxy.rs`
- Create: `crates/core/src/tools/proxy_media.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Modify: registries that construct `ToolRegistry`

- [ ] **Step 1: Write failing proxy helper tests**

Cover fresh, stale, missing, orphan, pending, stable proxy path, and cleanup planning if included.

- [ ] **Step 2: Implement reusable proxy helpers**

Move the stable proxy-path/status logic out of desktop-only command code or mirror it exactly in `awidat-core` without creating a dependency on Tauri.

- [ ] **Step 3: Implement tools**

`proxy_status(asset_id?)` returns status for one asset or all catalog/raw assets. `generate_proxy(asset_id, force?)` validates the asset path and calls `awidat_render::transcode_proxy`.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p awidat-core proxy_media proxy
```

## Task 5: Relink Apply Tool

**Files:**
- Create: `crates/core/src/tools/relink_media.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Modify: registries that construct `ToolRegistry`

- [ ] **Step 1: Write failing tests**

Cover replacing all matching missing `target_url` references, rejecting unsafe `../` or absolute targets, rejecting non-existent replacements, and preserving unrelated references.

- [ ] **Step 2: Implement relink traversal**

Traverse `Timeline.tracks` recursively through `StackChild` and `TrackChild`, update `MediaReference::External.target_url` when `old_target_url` or `clip_id` matches.

- [ ] **Step 3: Persist and report**

Write the project and return count, old target, new target, and affected clip names/ids.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p awidat-core relink_media diagnose_project_media
```

## Task 6: Bin, Asset Metadata, and Select Mutator Tools

**Files:**
- Create: `crates/core/src/tools/manage_assets.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Modify: registries that construct `ToolRegistry`

- [ ] **Step 1: Write failing tests**

Cover `create_bin`, `move_to_bin`, `rename_asset`, `tag_asset`, `rate_asset`, and `mark_select`.

- [ ] **Step 2: Implement bin tools**

`create_bin(id, name, parent_id?)` creates a durable bin. `move_to_bin(asset_id, bin_id?)` changes `AssetRecord.bin_id`.

- [ ] **Step 3: Implement metadata tools**

`rename_asset(asset_id, label)` updates `AssetRecord.label` without renaming files. `tag_asset(asset_id, add?, remove?)` dedupes tags. `rate_asset(asset_id, rating)` enforces `0..=5`.

- [ ] **Step 4: Implement `mark_select`**

Inputs include `asset_id`, `start_s`, `end_s`, `decision`, optional `reason`, optional `notes`. Generate stable ids from asset/range/decision when the caller does not provide one.

- [ ] **Step 5: Verify**

Run:

```bash
cargo test -p awidat-core manage_assets
```

## Task 7: Transcript Search Tool

**Files:**
- Create: `crates/core/src/tools/transcript_search.rs`
- Modify: `crates/core/src/tools/mod.rs`
- Modify: registries that construct `ToolRegistry`

- [ ] **Step 1: Write failing tests**

Use fixture whisper sidecars and cover query matching, optional asset filter, optional speaker filter, pagination/limit cap, and no-sidecar errors.

- [ ] **Step 2: Implement search**

Use `awidat_index::sidecar_io::walk_indexer(project_root, "whisper")`. Search segment text with case-insensitive substring scoring first; keep fuzzy/BM25 out unless existing local utilities make it trivial.

- [ ] **Step 3: Verify**

Run:

```bash
cargo test -p awidat-core transcript_search
```

## Task 8: Registry, Docs, and Final Verification

**Files:**
- Modify: `crates/core/src/tools/mod.rs`
- Modify: `apps/desktop/src-tauri/src/session.rs`
- Modify: any other registry found by `rg "build_registry|ToolRegistry::new|register\\(Arc::new"`
- Modify: `README.md` or `docs/` only if new tool usage needs discoverability.

- [ ] **Step 1: Register tools**

Ensure every new tool appears in desktop and non-desktop registries.

- [ ] **Step 2: Run focused tests**

Run all focused commands from prior tasks.

- [ ] **Step 3: Run workspace quality checks**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- [ ] **Step 4: Update the gap audit**

If implementation closes a gap, update `.reference-research/pro-editing-gap-analysis/01-media-management.md` with the new state and exact code references.

- [ ] **Step 5: Final self-review**

Confirm each original top gap has evidence:

- Agent-callable ingest exists and is registered.
- Agent-callable proxy status/generation exists and is registered.
- Relink diagnostics can be applied.
- Bins are user/agent-defined and durable.
- Tags, ratings, labels, and selects are mutable.
- Transcript search is first-class.
- Duplicate media walking is removed.
- Verification commands either pass or have a concrete environmental blocker.
