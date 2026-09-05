//! Configure direct or native-search MCP tool exposure (R31).
//!
//! The refreshed codex (upstream 315195492c80) defers **every** MCP tool
//! behind a tool-search mechanism whenever the active model reports
//! `supports_search_tool: true` — see
//! `vendor/codex-rs/core/src/mcp_tool_exposure.rs` (`ToolExposure::Deferred`)
//! and `vendor/codex-rs/core/src/tools/spec_plan.rs::search_tool_enabled`.
//! Every model in the bundled catalog (`models.json`) has that flag set,
//! and the former `tool_search` / `tool_search_always_defer_mcp_tools`
//! feature flags are now removed no-ops
//! (`vendor/codex-rs/features/src/lib.rs:163-166`) — which is why the
//! `features.tool_search_always_defer_mcp_tools=true` override the bridge
//! used to pass has silently stopped meaning anything.
//!
//! The only remaining lever is `model_info.supports_search_tool`, and the
//! only config surface that reaches it is a full model-catalog override
//! (`model_catalog_json`, `vendor/codex-rs/core/src/config/mod.rs:360`).
//! For Montage this deferral was a regression: on organically
//! phrased editing prompts ("do a quick cleanup pass") the model never
//! issues a tool-search, never sees the 116-tool Montage surface, and
//! shell-scripts edits to `project.otio.json` directly — bypassing the
//! validate/diff/proposal flow the whole product is built around
//! (confirmed in the live A/B, 2026-07-20).
//!
//! Direct mode derives a patched catalog from the vendored `models.json`
//! at runtime — flipping `supports_search_tool` to `false` on every model
//! — and writes it under `codex_home` so the override is a stable on-disk
//! path. The transform is a pure JSON edit (one boolean per model) over
//! `serde_json::Value`, so it does **not** pull the vendored model-catalog
//! type graph into the default (non-`in-process-codex`) bridge build that
//! the external app-server path compiles into. Deriving from the vendored
//! source (rather than a hand-frozen copy) means the patch auto-tracks
//! upstream catalog changes on each fork refresh: new models, renamed
//! slugs, and reasoning edits all flow through untouched except for the
//! one flag we clear. The MCP server advertises a stable catalog and checks
//! the bootstrap tools plus active skill allowlist at invocation time.
//! `MONTAGE_CODEX_TOOL_EXPOSURE=native_search`
//! opts into Codex's native deferred discovery for evaluation; direct remains
//! the safe default until the checked-in organic-prompt gate passes.

use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::warn;

/// The vendored codex model catalog, embedded at compile time. Same file
/// codex's own `codex_models_manager::bundled_models_response()` reads, so
/// our patched copy stays a faithful superset-minus-one-flag of what the
/// spawned app-server would otherwise use.
const BUNDLED_MODELS_JSON: &str =
    include_str!("../../../vendor/codex-rs/models-manager/models.json");

/// Filename for the patched catalog under `<codex_home>`.
const DIRECT_EXPOSURE_CATALOG_FILE: &str = "montage-direct-tools-catalog.json";
const TOOL_EXPOSURE_ENV: &str = "MONTAGE_CODEX_TOOL_EXPOSURE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolExposureMode {
    Direct,
    NativeSearch,
}

fn tool_exposure_mode(value: Option<&str>) -> ToolExposureMode {
    match value.map(str::trim) {
        Some("native_search") => ToolExposureMode::NativeSearch,
        _ => ToolExposureMode::Direct,
    }
}

pub(crate) fn configured_tool_exposure() -> &'static str {
    match tool_exposure_mode(std::env::var(TOOL_EXPOSURE_ENV).ok().as_deref()) {
        ToolExposureMode::Direct => "direct",
        ToolExposureMode::NativeSearch => "native_search",
    }
}

/// Errors deriving or writing the direct-exposure catalog. Non-fatal at
/// the call site: the bridge logs and continues without the override
/// (falling back to codex's deferred behavior) rather than failing a
/// session launch over a tool-exposure tweak.
#[derive(Debug, thiserror::Error)]
pub enum CatalogOverrideError {
    /// The embedded `models.json` didn't parse, or didn't have the
    /// expected `{ "models": [ … ] }` shape.
    #[error("parsing bundled model catalog: {0}")]
    ParseBundled(String),
    /// Serializing the patched catalog failed.
    #[error("serializing patched model catalog: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Writing the patched catalog to disk failed.
    #[error("writing patched model catalog to {path}: {source}")]
    Write {
        /// Target path.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Parse the embedded catalog and clear `supports_search_tool` on every
/// model. Pure — no I/O — so the transform is unit testable without a
/// `codex_home`. Returns the patched `Value` plus how many models had the
/// flag flipped (used by tests to guard the fix's premise).
fn direct_exposure_catalog() -> Result<(Value, usize), CatalogOverrideError> {
    let mut catalog: Value = serde_json::from_str(BUNDLED_MODELS_JSON)
        .map_err(|e| CatalogOverrideError::ParseBundled(e.to_string()))?;
    let models = catalog
        .get_mut("models")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CatalogOverrideError::ParseBundled(
                "expected a top-level `models` array in models.json".to_string(),
            )
        })?;
    let mut flipped = 0usize;
    for model in models.iter_mut() {
        if let Some(obj) = model.as_object_mut() {
            // Only count a flip when the flag was actually true — lets the
            // premise test detect a future catalog where nothing defers.
            let was_set = obj
                .get("supports_search_tool")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if was_set {
                flipped += 1;
            }
            obj.insert("supports_search_tool".to_string(), Value::Bool(false));
        }
    }
    Ok((catalog, flipped))
}

/// Write the direct-exposure catalog under `codex_home` and return its
/// path, suitable for a `model_catalog_json="<path>"` config override.
///
/// Idempotent by content: the catalog is rewritten every launch (cheap,
/// a few KB), so a fork refresh that changes `models.json` is picked up
/// on the next session without any stale-file handling.
pub fn write_direct_exposure_catalog(codex_home: &Path) -> Result<PathBuf, CatalogOverrideError> {
    let (catalog, _flipped) = direct_exposure_catalog()?;
    let bytes = serde_json::to_vec_pretty(&catalog).map_err(CatalogOverrideError::Serialize)?;
    let path = codex_home.join(DIRECT_EXPOSURE_CATALOG_FILE);
    std::fs::write(&path, &bytes).map_err(|source| CatalogOverrideError::Write {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Resolve codex_home, write the patched catalog, and return the path as
/// a string suitable for a `model_catalog_json="<path>"` config override.
/// Returns `None` (after logging) on any failure, so callers can drop the
/// override and let codex fall back to its deferred behavior rather than
/// failing a session launch over a tool-exposure tweak.
///
/// Shared by both the external app-server path (`external::app_server_args`,
/// the default desktop build) and the in-process path.
pub fn direct_exposure_catalog_override_value() -> Option<String> {
    if configured_tool_exposure() == "native_search" {
        return None;
    }
    let codex_home = match codex_utils_home_dir::find_codex_home() {
        Ok(home) => home,
        Err(e) => {
            warn!(
                "could not resolve codex_home for R31 direct-exposure catalog ({e}); \
                 Montage tools stay deferred behind tool-search"
            );
            return None;
        }
    };
    match write_direct_exposure_catalog(codex_home.as_path()) {
        Ok(path) => Some(path.display().to_string()),
        Err(e) => {
            warn!(
                "R31 direct-exposure catalog unavailable ({e}); \
                 Montage tools stay deferred behind tool-search"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_exposure_defaults_to_direct_and_allows_native_search_trials() {
        assert_eq!(tool_exposure_mode(None), ToolExposureMode::Direct);
        assert_eq!(
            tool_exposure_mode(Some("native_search")),
            ToolExposureMode::NativeSearch
        );
        assert_eq!(
            tool_exposure_mode(Some("unknown")),
            ToolExposureMode::Direct
        );
    }

    #[test]
    fn every_model_has_search_tool_cleared() {
        let (catalog, _) = direct_exposure_catalog().expect("bundled catalog derives");
        let models = catalog["models"].as_array().expect("models array");
        assert!(!models.is_empty(), "bundled catalog should carry models");
        for model in models {
            assert_eq!(
                model["supports_search_tool"].as_bool(),
                Some(false),
                "model {} still has supports_search_tool set",
                model["slug"]
            );
        }
    }

    #[test]
    fn bundled_catalog_actually_deferred_somewhere() {
        // Guards the fix's premise: if upstream ever ships a catalog where
        // nothing defers, this override is a no-op and R31 no longer
        // applies — a failing test here prompts re-evaluation rather than
        // silently shipping a pointless override.
        let (_, flipped) = direct_exposure_catalog().expect("derives");
        assert!(
            flipped > 0,
            "no bundled model deferred tools; the R31 override may be obsolete — re-check \
             vendor/codex-rs/core/src/mcp_tool_exposure.rs before removing it"
        );
    }

    #[test]
    fn write_produces_a_readable_catalog_with_flag_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_direct_exposure_catalog(dir.path()).expect("write succeeds");
        assert!(path.starts_with(dir.path()));
        let written: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("round-trips");
        let models = written["models"].as_array().expect("models array");
        assert!(!models.is_empty());
        assert!(
            models
                .iter()
                .all(|m| m["supports_search_tool"].as_bool() == Some(false))
        );
    }

    #[test]
    fn embedded_catalog_matches_the_shape_codex_expects() {
        // The spawned app-server parses this same file into ModelsResponse
        // ({ models: [ModelInfo, …] }). We can't depend on that type here
        // (default build), but we can assert the structural invariants it
        // requires so a malformed vendored file fails our test, not a
        // silent runtime rejection of the override.
        let (catalog, _) = direct_exposure_catalog().expect("derives");
        let models = catalog["models"].as_array().expect("models array");
        for model in models {
            assert!(model["slug"].is_string(), "each model needs a slug");
            assert!(
                model["supports_search_tool"].is_boolean(),
                "supports_search_tool must be a bool after patching"
            );
        }
    }
}
