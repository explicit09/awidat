//! Index sidecar contract — the "lingua franca" types every footage indexer
//! shares.
//!
//! This is `PLAN.md` Concern A. The engine never sees indexer-specific
//! types; it only ever sees:
//!
//! - [`Manifest`] — the registry at `index/manifest.json`.
//! - [`IndexerEntry`] — one row in the manifest.
//! - [`IndexSidecarHeader`] — the self-describing header on every sidecar.
//! - [`AssetId`], [`TimeSeconds`] — the shared coordinate model.
//!
//! Indexer-specific data lives under [`IndexSidecar::data`] as
//! `serde_json::Value` from the engine's perspective; only the indexer that
//! produced the file types its body strongly. Adding a new indexer (e.g.
//! `speaker-emotion` in v1.5) is purely additive: drop a new MCP server,
//! register it in the manifest, write to a new `index/<name>/` directory.
//! No engine code changes.
//!
//! See `INDEX_SCHEMA.md` for the full contract and a worked example.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ProtoError;
use crate::error::JsonPath;

/// A source asset's logical identifier. Convention: relative path from the
/// project root, e.g. `"raw/ep-014-cam-a.mp4"`. Engine treats this as opaque.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub String);

impl AssetId {
    /// Construct an [`AssetId`] from any string-like input.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Return the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validate that the asset id is a safe project-relative path.
    pub fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if is_safe_relative_asset_path(&self.0) {
            Ok(())
        } else {
            Err(ProtoError::validation(
                file,
                path.clone(),
                format!(
                    "asset id must be a safe relative path with no '..', '.', absolute prefix, backslash, or empty component; got '{}'",
                    self.0
                ),
            ))
        }
    }

    /// Relative sidecar path for this asset, with `.json` appended to the
    /// final filename.
    pub fn sidecar_relative_path(&self) -> Option<PathBuf> {
        if !is_safe_relative_asset_path(&self.0) {
            return None;
        }
        let mut parts = self.0.split('/').peekable();
        let mut out = PathBuf::new();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                out.push(part);
            } else {
                out.push(format!("{part}.json"));
            }
        }
        Some(out)
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A timestamp in seconds, as `f64`. The shared coordinate every indexer
/// emits.
///
/// `f64` because the OTIO spec uses it and indexers run at heterogeneous
/// rates (whisper word-level ~10 ms; scene boundaries at frame rate). A
/// single canonical seconds-based representation is the lowest friction
/// joining type.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimeSeconds(pub f64);

impl TimeSeconds {
    /// Construct.
    pub fn new(s: f64) -> Self {
        Self(s)
    }

    /// Inner value.
    pub fn value(self) -> f64 {
        self.0
    }

    /// Validate that this timestamp is finite and non-negative.
    pub fn validate(&self, file: &str, path: &JsonPath) -> Result<(), ProtoError> {
        if !self.0.is_finite() {
            return Err(ProtoError::validation(
                file,
                path.clone(),
                format!("timestamp must be finite, got {}", self.0),
            ));
        }
        if self.0 < 0.0 {
            return Err(ProtoError::validation(
                file,
                path.clone(),
                format!("timestamp must be non-negative, got {}", self.0),
            ));
        }
        Ok(())
    }
}

/// Returns true iff `name` is a valid canonical indexer id.
pub fn is_valid_indexer_id(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// True iff `s` is a safe project-relative path: non-empty, forward-slash
/// separated, and every component a plain name (no `..`/`.`, no absolute or
/// prefixed paths, no backslashes, NULs, or empty segments). The single
/// authoritative check for untrusted asset-id-shaped inputs before joining
/// them onto a project root.
pub fn is_safe_relative_asset_path(s: &str) -> bool {
    if s.is_empty() || s.contains('\\') || s.contains('\0') || s.contains("//") {
        return false;
    }
    Path::new(s)
        .components()
        .all(|component| matches!(component, Component::Normal(part) if !part.is_empty()))
}

/// Self-describing header on every sidecar.
///
/// Every indexer writes this header before its `data` blob. The engine
/// validates the header on read; per-indexer body is opaque.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSidecarHeader {
    /// Canonical indexer name. By convention a lowercase, hyphenated id —
    /// `"whisper"`, `"scenedetect"`, `"audio-energy"`, `"speaker-emotion"`,
    /// etc. The directory under `index/` matches this name.
    pub indexer: String,
    /// Software version of the indexer that produced this file.
    pub indexer_version: String,
    /// Schema version of the indexer's `data` shape. Independent of
    /// `indexer_version` so an indexer can ship a bug-fix without bumping
    /// its data schema.
    pub schema_version: String,
    /// Asset this sidecar describes.
    pub asset_id: AssetId,
    /// SHA-256 of the asset bytes at index time. Lets the engine detect
    /// stale sidecars without re-running an expensive indexer.
    pub asset_sha256: String,
    /// When the sidecar was produced (UTC).
    pub produced_at: DateTime<Utc>,
}

/// Generic sidecar with a typed body.
///
/// The engine consumes [`IndexSidecar<serde_json::Value>`] (data is opaque).
/// Each indexer in its own crate may define its own typed body and use
/// [`IndexSidecar<MyTypedData>`] internally.
///
/// Serialization is flat: `header` fields are at the top level, alongside a
/// single `data` object. This matches the "self-describing header" shape
/// from `PLAN.md` Concern A.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSidecar<T> {
    /// Sidecar header.
    #[serde(flatten)]
    pub header: IndexSidecarHeader,
    /// Per-indexer body. Engine sees this as `serde_json::Value`.
    pub data: T,
}

/// `index/manifest.json` — the registry the engine consults.
///
/// The engine treats this as data: it never hardcodes an indexer name and
/// never makes a behavioral switch on `entry.name`. New indexers slot in by
/// appending to `indexers`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest schema version. Bumped only on breaking changes to this
    /// file's shape.
    pub version: String,
    /// All indexers known to have run on this project.
    pub indexers: Vec<IndexerEntry>,
}

impl Manifest {
    /// Empty manifest at the current version.
    pub fn empty() -> Self {
        Self {
            version: crate::INDEX_MANIFEST_VERSION.to_string(),
            indexers: Vec::new(),
        }
    }
}

/// Row in the [`Manifest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerEntry {
    /// Same value as [`IndexSidecarHeader::indexer`].
    pub name: String,
    /// Software version, mirrors [`IndexSidecarHeader::indexer_version`].
    pub version: String,
    /// Body schema version, mirrors [`IndexSidecarHeader::schema_version`].
    pub schema_version: String,
    /// All assets this indexer has produced sidecars for.
    pub assets: Vec<AssetId>,
    /// Last successful run.
    pub last_run: DateTime<Utc>,
}

impl IndexerEntry {
    /// Returns true iff the entry contains the given asset id.
    pub fn covers(&self, asset: &AssetId) -> bool {
        self.assets.iter().any(|a| a == asset)
    }
}

/// Convenience: deduplicate a list of asset ids preserving order.
pub fn dedup_assets(input: impl IntoIterator<Item = AssetId>) -> Vec<AssetId> {
    let mut seen: HashSet<AssetId> = HashSet::new();
    let mut out = Vec::new();
    for id in input {
        if seen.insert(id.clone()) {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn manifest_empty_uses_current_version() {
        let m = Manifest::empty();
        assert_eq!(m.version, crate::INDEX_MANIFEST_VERSION);
        assert!(m.indexers.is_empty());
    }

    #[test]
    fn manifest_roundtrip() {
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "whisper".into(),
                version: "1.2.3".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("raw/ep.mp4")],
                last_run: now(),
            }],
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: Manifest = crate::serde_robust::from_json_str(&s).unwrap();
        assert_eq!(back.indexers.len(), 1);
        assert_eq!(back.indexers[0].name, "whisper");
    }

    #[test]
    fn sidecar_header_flatten_roundtrip() {
        let header = IndexSidecarHeader {
            indexer: "whisper".into(),
            indexer_version: "1.2.3".into(),
            schema_version: "1".into(),
            asset_id: AssetId::new("raw/ep.mp4"),
            asset_sha256: "abc123".into(),
            produced_at: now(),
        };
        let sidecar = IndexSidecar {
            header,
            data: serde_json::json!({
                "words": [{"text": "hello", "start_s": 0.0, "end_s": 0.4}],
            }),
        };
        let s = serde_json::to_string(&sidecar).unwrap();
        // Flatten: top-level should contain `indexer` and `data`, not a nested
        // `header` key.
        assert!(s.contains("\"indexer\":\"whisper\""));
        assert!(s.contains("\"data\":"));
        assert!(!s.contains("\"header\":"));
        let back: IndexSidecar<serde_json::Value> = crate::serde_robust::from_json_str(&s).unwrap();
        assert_eq!(back.header.indexer, "whisper");
    }

    #[test]
    fn engine_reads_arbitrary_indexer_data_via_json_value() {
        // The engine never knows the data shape. Confirm it can deserialize a
        // sidecar with a brand-new indexer body it's never seen before.
        let json = serde_json::json!({
            "indexer": "speaker-emotion",
            "indexer_version": "0.1.0",
            "schema_version": "1",
            "asset_id": "raw/ep.mp4",
            "asset_sha256": "def456",
            "produced_at": "2026-05-02T12:00:00Z",
            "data": {
                "windows": [
                    {"start_s": 0.0, "end_s": 1.0, "valence": 0.6, "arousal": 0.8}
                ]
            }
        });
        let s = serde_json::to_string(&json).unwrap();
        let parsed: IndexSidecar<serde_json::Value> =
            crate::serde_robust::from_json_str(&s).unwrap();
        assert_eq!(parsed.header.indexer, "speaker-emotion");
        // The engine successfully read the header without knowing the schema.
        assert!(parsed.data.is_object());
    }

    #[test]
    fn dedup_assets_preserves_order() {
        let ids = vec![
            AssetId::new("a"),
            AssetId::new("b"),
            AssetId::new("a"),
            AssetId::new("c"),
        ];
        let out = dedup_assets(ids);
        assert_eq!(
            out.iter().map(AssetId::as_str).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn indexer_entry_covers_asset() {
        let e = IndexerEntry {
            name: "whisper".into(),
            version: "1".into(),
            schema_version: "1".into(),
            assets: vec![AssetId::new("a"), AssetId::new("b")],
            last_run: now(),
        };
        assert!(e.covers(&AssetId::new("a")));
        assert!(!e.covers(&AssetId::new("c")));
    }

    #[test]
    fn asset_id_validates_safe_relative_paths() {
        let path = JsonPath::root().field("asset_id");
        AssetId::new("raw/ep.mp4")
            .validate("index.json", &path)
            .unwrap();
        assert_eq!(
            AssetId::new("raw/ep.mp4").sidecar_relative_path().unwrap(),
            PathBuf::from("raw").join("ep.mp4.json")
        );

        for bad in [
            "",
            "/tmp/ep.mp4",
            "../ep.mp4",
            "raw/../ep.mp4",
            "raw//ep.mp4",
            "raw\\ep.mp4",
        ] {
            let err = AssetId::new(bad).validate("index.json", &path).unwrap_err();
            assert!(
                err.to_string()
                    .contains("asset id must be a safe relative path")
            );
            assert!(AssetId::new(bad).sidecar_relative_path().is_none());
        }
    }

    #[test]
    fn indexer_id_validation_is_strict_and_simple() {
        for good in ["whisper", "whisperx", "audio-energy", "a1"] {
            assert!(is_valid_indexer_id(good), "{good}");
        }
        for bad in [
            "",
            "-audio",
            "audio-",
            "Audio",
            "audio_energy",
            "audio/energy",
            "audio energy",
        ] {
            assert!(!is_valid_indexer_id(bad), "{bad}");
        }
    }

    #[test]
    fn time_seconds_validates_finite_non_negative() {
        TimeSeconds::new(0.0)
            .validate("index.json", &JsonPath::root().field("start_s"))
            .unwrap();
        TimeSeconds::new(12.5)
            .validate("index.json", &JsonPath::root().field("start_s"))
            .unwrap();

        let err = TimeSeconds::new(-0.1)
            .validate("index.json", &JsonPath::root().field("start_s"))
            .unwrap_err();
        assert!(err.to_string().contains("timestamp must be non-negative"));

        let err = TimeSeconds::new(f64::NAN)
            .validate("index.json", &JsonPath::root().field("start_s"))
            .unwrap_err();
        assert!(err.to_string().contains("timestamp must be finite"));
    }
}
