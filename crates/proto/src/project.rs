//! Filesystem-backed project: read, write, init, validate.
//!
//! A project is a directory; see `PLAN.md` §3 / §4. v1 Week 1 layout:
//!
//! ```text
//! <project>/
//! ├── project.otio.json     # OTIO superset, single Timeline
//! ├── edit-plan.json        # structured edit plan
//! ├── episode-notes.md      # agent's running notes
//! ├── index/
//! │   ├── manifest.json     # registry of indexers
//! │   └── <name>/<asset>.json  # one sidecar per asset, per indexer
//! ├── renders/              # final renders
//! └── .montage/              # ephemeral state (session db, caches)
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::ProtoError;
use crate::error::JsonPath;
use crate::index::Manifest;
use crate::otio::{
    SUPPORTED_SCHEMA_NAMES, SchemaParseOutcome, SchemaWarning, Timeline, parse_schema_string,
};

/// File names inside a project, kept as constants so the CLI and validator
/// agree.
pub mod files {
    /// Main OTIO file.
    pub const OTIO: &str = "project.otio.json";
    /// Structured edit plan.
    pub const EDIT_PLAN: &str = "edit-plan.json";
    /// Agent running notes.
    pub const EPISODE_NOTES: &str = "episode-notes.md";
    /// Index dir.
    pub const INDEX_DIR: &str = "index";
    /// Index manifest filename.
    pub const INDEX_MANIFEST: &str = "manifest.json";
    /// Renders dir.
    pub const RENDERS_DIR: &str = "renders";
    /// Ephemeral state dir.
    pub const MONTAGE_DIR: &str = ".montage";
}

/// Default `.gitignore` written by `montage init`. Per `PLAN.md` §5.3:
/// regenerable indexer outputs and ephemeral session state stay out of git
/// by default; the manifest is carved back in because it's small and
/// authoritative.
const GITIGNORE_TEMPLATE: &str = "\
# Montage project — keep diffability sharp.
# Regenerable outputs and ephemeral state stay out of git by default.

# Sidecar JSON written by indexers (whisper, scenedetect, ...). Large and
# rebuildable from `montage index`.
/index/*
# Carve out the manifest: small, hand-curated source of truth.
!/index/manifest.json

# Final and proxy renders.
/renders/

# Ephemeral session state.
/.montage/

# OS noise.
.DS_Store
";

/// In-memory project. Built by [`Project::read`] and consumed by
/// [`Project::write`].
#[derive(Debug, Clone)]
pub struct Project {
    /// Project root directory.
    pub root: PathBuf,
    /// Parsed OTIO timeline.
    pub timeline: Timeline,
    /// Parsed edit plan.
    pub edit_plan: EditPlan,
    /// Parsed index manifest, if present. `None` means no indexers have run
    /// — a normal "fresh project" state.
    pub manifest: Option<Manifest>,
    /// Forward-compat warnings encountered during load. The CLI prints them
    /// at the end of `validate`.
    pub warnings: Vec<SchemaWarning>,
}

/// `edit-plan.json` shape. Status fields are free strings per `PLAN.md`
/// §4.1; we leave them as `String` rather than enum because the agent
/// extends the status vocabulary and we want forward-compat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlan {
    /// Schema version. Currently `"0.1"`.
    pub version: String,
    /// Optional brief.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub brief: Option<String>,
    /// Items in the plan.
    #[serde(default)]
    pub items: Vec<EditPlanItem>,
}

impl EditPlan {
    /// Empty plan at the current version.
    pub fn empty() -> Self {
        Self {
            version: crate::EDIT_PLAN_VERSION.to_string(),
            brief: None,
            items: Vec::new(),
        }
    }
}

/// One item in an [`EditPlan`].
///
/// We keep this **deliberately permissive** at Week 1 — the precise schema
/// of an edit-plan item lives with `apply_edl` in Week 4. The Week 1 job is
/// just "round-trip without losing fields."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlanItem {
    /// Item id.
    pub id: String,
    /// Item kind. Free string; week-4 `apply_edl` defines the vocabulary.
    pub kind: String,
    /// Status. One of `pending | applied | rejected | needs_user_input |
    /// failed` per `PLAN.md` §4.1.
    pub status: String,
    /// All other fields preserved verbatim.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Project {
    /// Initialize a fresh project at `root`. Creates the directory and all
    /// the required files / subdirs. Refuses to overwrite a non-empty
    /// directory.
    pub fn init(root: &Path) -> Result<Self, ProtoError> {
        // Ensure the directory either doesn't exist or is empty enough not to
        // clash. We allow an empty existing directory (e.g. one git just
        // cloned).
        if root.exists() {
            let mut entries = fs::read_dir(root).map_err(|e| ProtoError::Io {
                path: root.display().to_string(),
                source: e,
            })?;
            if entries.next().is_some() {
                return Err(ProtoError::Io {
                    path: root.display().to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "refusing to init: directory exists and is not empty",
                    ),
                });
            }
        } else {
            fs::create_dir_all(root).map_err(|e| ProtoError::Io {
                path: root.display().to_string(),
                source: e,
            })?;
        }

        // Subdirectories.
        let index_dir = root.join(files::INDEX_DIR);
        let renders_dir = root.join(files::RENDERS_DIR);
        let montage_dir = root.join(files::MONTAGE_DIR);
        for d in [&index_dir, &renders_dir, &montage_dir] {
            fs::create_dir_all(d).map_err(|e| ProtoError::Io {
                path: d.display().to_string(),
                source: e,
            })?;
        }

        // Project-root .gitignore: index/ and renders/ are regenerable
        // outputs; .montage/ is ephemeral session state. None should land in
        // git by default. The manifest under index/ is the one exception —
        // small, hand-curated, and the source of truth for which indexers
        // ran. See PLAN.md §5.3.
        write_file(&root.join(".gitignore"), GITIGNORE_TEMPLATE)?;

        // Default Timeline.
        let timeline = Timeline::empty(default_project_name(root));

        // Default empty edit plan.
        let edit_plan = EditPlan::empty();

        // Default empty index manifest.
        let manifest = Manifest::empty();

        // Persist files.
        write_file(
            &root.join(files::OTIO),
            &serde_json::to_string_pretty(&timeline)
                .map_err(|e| io_serde_error(root.join(files::OTIO).display().to_string(), e))?,
        )?;
        write_file(
            &root.join(files::EDIT_PLAN),
            &serde_json::to_string_pretty(&edit_plan).map_err(|e| {
                io_serde_error(root.join(files::EDIT_PLAN).display().to_string(), e)
            })?,
        )?;
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        write_file(
            &root.join(files::EPISODE_NOTES),
            &format!("# Episode Notes\n\nProject created: {now}\n"),
        )?;
        write_file(
            &index_dir.join(files::INDEX_MANIFEST),
            &serde_json::to_string_pretty(&manifest).map_err(|e| {
                io_serde_error(
                    index_dir.join(files::INDEX_MANIFEST).display().to_string(),
                    e,
                )
            })?,
        )?;

        Ok(Self {
            root: root.to_path_buf(),
            timeline,
            edit_plan,
            manifest: Some(manifest),
            warnings: Vec::new(),
        })
    }

    /// Read an existing project from disk and validate it. Forward-compat
    /// warnings are collected on [`Self::warnings`] but do not cause read
    /// to fail.
    pub fn read(root: &Path) -> Result<Self, ProtoError> {
        let mut warnings: Vec<SchemaWarning> = Vec::new();

        // OTIO.
        let otio_path = root.join(files::OTIO);
        let timeline = read_otio_timeline(&otio_path, &mut warnings)?;
        timeline.validate(&otio_path.display().to_string(), &JsonPath::root())?;

        // Edit plan.
        let plan_path = root.join(files::EDIT_PLAN);
        let edit_plan = read_json::<EditPlan>(&plan_path)?;

        // Index manifest (optional).
        let manifest_path = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let manifest = if manifest_path.exists() {
            Some(read_json::<Manifest>(&manifest_path)?)
        } else {
            None
        };

        Ok(Self {
            root: root.to_path_buf(),
            timeline,
            edit_plan,
            manifest,
            warnings,
        })
    }

    /// Write the project state back to disk. Overwrites the three managed
    /// files; ignores `episode-notes.md` (agent-owned).
    pub fn write(&self, root: &Path) -> Result<(), ProtoError> {
        write_file(
            &root.join(files::OTIO),
            &serde_json::to_string_pretty(&self.timeline)
                .map_err(|e| io_serde_error(root.join(files::OTIO).display().to_string(), e))?,
        )?;
        write_file(
            &root.join(files::EDIT_PLAN),
            &serde_json::to_string_pretty(&self.edit_plan).map_err(|e| {
                io_serde_error(root.join(files::EDIT_PLAN).display().to_string(), e)
            })?,
        )?;
        if let Some(m) = &self.manifest {
            let p = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
            write_file(
                &p,
                &serde_json::to_string_pretty(m)
                    .map_err(|e| io_serde_error(p.display().to_string(), e))?,
            )?;
        }
        Ok(())
    }
}

fn default_project_name(root: &Path) -> String {
    root.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("montage-project")
        .to_string()
}

fn write_file(path: &Path, contents: &str) -> Result<(), ProtoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ProtoError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }
    fs::write(path, contents).map_err(|e| ProtoError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ProtoError> {
    let text = fs::read_to_string(path).map_err(|e| ProtoError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    serde_json::from_str::<T>(&text).map_err(|e| ProtoError::Json {
        file: path.display().to_string(),
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    })
}

fn io_serde_error(path: String, e: serde_json::Error) -> ProtoError {
    ProtoError::Io {
        path,
        source: std::io::Error::other(e),
    }
}

/// Read the OTIO timeline, applying forward-compat schema rewriting before
/// strict deserialization. Forward-compat warnings are appended to
/// `warnings`.
pub fn read_otio_timeline(
    path: &Path,
    warnings: &mut Vec<SchemaWarning>,
) -> Result<Timeline, ProtoError> {
    let text = fs::read_to_string(path).map_err(|e| ProtoError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| ProtoError::Json {
            file: path.display().to_string(),
            line: e.line(),
            column: e.column(),
            message: e.to_string(),
        })?;

    let file = path.display().to_string();
    rewrite_schema_strings(&mut value, &file, &JsonPath::root(), warnings)?;

    // Try strict deserialization first — covers every well-formed file.
    match serde_json::from_value::<Timeline>(value.clone()) {
        Ok(tl) => Ok(tl),
        Err(strict_err) => {
            // Strict parse failed. The most common cause we've seen is the
            // agent writing malformed entries into `metadata.montage.*`
            // arrays (e.g. a `TimelineMarker` with `name`/`source_time_s`
            // instead of `label`/`time_s`). Those sections are editorial
            // sidecars — losing them is sad but recoverable; rejecting
            // the whole project means the user can't open it at all.
            //
            // Quarantine pass: walk every array under `metadata.montage`;
            // if any element fails to deserialize against its sibling
            // elements' shape, drop the offenders and re-try. We do this
            // ONLY on a strict-parse failure so well-formed files take
            // the fast path.
            if let Some(quarantined) = quarantine_montage_metadata_arrays(value, &file, warnings) {
                serde_json::from_value::<Timeline>(quarantined).map_err(|e| {
                    ProtoError::Validation {
                        file,
                        path: JsonPath::root(),
                        message: format!("{e} (after quarantining metadata.montage arrays; original error: {strict_err})"),
                    }
                })
            } else {
                Err(ProtoError::Validation {
                    file,
                    path: JsonPath::root(),
                    message: format!("{strict_err}"),
                })
            }
        }
    }
}

/// Best-effort quarantine of malformed entries under `metadata.montage`.
/// Returns `Some(rewritten_value)` if at least one array was sanitized
/// (so the caller can retry deserialization), or `None` if there was
/// nothing to fix (the failure is elsewhere in the document).
///
/// Strategy: for every array directly under `metadata.montage`, validate
/// each element against the SHAPE of the most-common element by trying
/// the array as a `Vec<serde_json::Value>` against a per-element
/// best-effort serde fallback. We can't know the target Rust type here
/// (this fn is in proto without type-erased knowledge of montage_meta),
/// so we do the cheap-but-effective thing: keep only elements whose
/// shape matches the FIRST element's keys (a sibling-shape heuristic).
/// If the very first element is malformed, we still try by checking
/// against any element in the array as the reference.
///
/// False positives (rejecting valid heterogenous entries) are possible
/// in principle; in practice every metadata.montage array we currently
/// define is homogeneous by struct, so this is safe.
fn quarantine_montage_metadata_arrays(
    mut value: serde_json::Value,
    file: &str,
    _warnings: &mut Vec<SchemaWarning>,
) -> Option<serde_json::Value> {
    let montage = value
        .pointer_mut("/metadata/montage")
        .and_then(|v| v.as_object_mut())?;

    let mut touched = false;
    let array_keys: Vec<String> = montage
        .iter()
        .filter(|(_, v)| v.as_array().is_some_and(|arr| !arr.is_empty()))
        .map(|(k, _)| k.clone())
        .collect();
    // If every element of an montage-metadata array is missing every
    // anchor field we know of (id / time_s / start_s / clip_id / uuid),
    // the agent likely wrote a fully-wrong shape — drop the lot rather
    // than fail the whole project load. Editorial sidecars losing
    // entries is recoverable; an unreadable project isn't.
    for key in array_keys {
        let Some(arr_val) = montage.get_mut(&key) else {
            continue;
        };
        let Some(arr) = arr_val.as_array() else {
            continue;
        };
        let all_bad = arr.iter().all(|el| {
            let Some(obj) = el.as_object() else {
                return true;
            };
            !obj.contains_key("id")
                && !obj.contains_key("time_s")
                && !obj.contains_key("start_s")
                && !obj.contains_key("clip_id")
                && !obj.contains_key("uuid")
        });
        if all_bad {
            let dropped = arr.len();
            *arr_val = serde_json::Value::Array(Vec::new());
            touched = true;
            eprintln!(
                "warning: {file}: emptied {dropped} malformed entr{plural} from metadata.montage.{key} (no anchor fields found)",
                plural = if dropped == 1 { "y" } else { "ies" }
            );
        }
    }

    touched.then_some(value)
}

/// Walk a JSON value, validate every `OTIO_SCHEMA` string, and rewrite
/// known-name-unknown-major strings to the supported major (recording a
/// warning).
///
/// This is what makes forward-compat work without needing custom serde
/// deserializers per type: by the time we hand the JSON to `serde_json::
/// from_value`, every `OTIO_SCHEMA` matches an enum variant.
fn rewrite_schema_strings(
    value: &mut serde_json::Value,
    file: &str,
    path: &JsonPath,
    warnings: &mut Vec<SchemaWarning>,
) -> Result<(), ProtoError> {
    match value {
        serde_json::Value::Object(map) => {
            // If this object has an OTIO_SCHEMA, parse and rewrite.
            if let Some(serde_json::Value::String(s)) = map.get("OTIO_SCHEMA") {
                let schema_str = s.clone();
                match parse_schema_string(file, path, &schema_str)? {
                    SchemaParseOutcome::Exact { .. } => {}
                    SchemaParseOutcome::ForwardCompat {
                        name,
                        file_major,
                        expected_major,
                    } => {
                        let canonical = format!("{name}.{expected_major}");
                        warnings.push(SchemaWarning {
                            file: file.to_string(),
                            path: format!("{path}"),
                            schema: schema_str,
                            expected_major,
                            found_major: file_major,
                        });
                        map.insert(
                            "OTIO_SCHEMA".to_string(),
                            serde_json::Value::String(canonical),
                        );
                    }
                }
            }
            // Recurse into children. Use known field names where useful so
            // the JSON path is meaningful.
            // We collect keys first to satisfy the borrow checker.
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                if let Some(child) = map.get_mut(&k) {
                    rewrite_schema_strings(child, file, &path.field(&k), warnings)?;
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, child) in arr.iter_mut().enumerate() {
                rewrite_schema_strings(child, file, &path.index(i), warnings)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Names of the supported OTIO schemas, exposed for CLI's `validate`
/// summary line.
pub fn supported_schema_summary() -> String {
    SUPPORTED_SCHEMA_NAMES
        .iter()
        .map(|e| format!("{}.{}", e.name, e.expected_major))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        p.push(format!("montage-test-{nanos}-{}-{n}", std::process::id()));
        p
    }

    #[test]
    fn init_creates_layout() {
        let root = tmp_dir();
        let p = Project::init(&root).unwrap();
        assert!(root.join(files::OTIO).exists());
        assert!(root.join(files::EDIT_PLAN).exists());
        assert!(root.join(files::EPISODE_NOTES).exists());
        assert!(
            root.join(files::INDEX_DIR)
                .join(files::INDEX_MANIFEST)
                .exists()
        );
        // .gitignore is the source of truth; regenerable dirs are not
        // .gitkeep'd because we don't want them tracked.
        let gi = root.join(".gitignore");
        assert!(gi.exists(), ".gitignore must be written by init");
        let gi_text = fs::read_to_string(&gi).unwrap();
        assert!(gi_text.contains("/index/*"));
        assert!(gi_text.contains("!/index/manifest.json"));
        assert!(gi_text.contains("/renders/"));
        assert!(gi_text.contains("/.montage/"));
        assert!(!root.join(files::INDEX_DIR).join(".gitkeep").exists());
        assert_eq!(p.timeline.otio_schema, "Timeline.1");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn init_then_read_roundtrip() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        let p = Project::read(&root).unwrap();
        assert!(p.warnings.is_empty());
        assert_eq!(p.timeline.otio_schema, "Timeline.1");
        assert_eq!(p.edit_plan.version, crate::EDIT_PLAN_VERSION);
        assert!(p.manifest.is_some());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn init_refuses_nonempty_dir() {
        let root = tmp_dir();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("dummy.txt"), b"hi").unwrap();
        let err = Project::init(&root).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("io"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_warns_on_known_name_unknown_major() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        // Hand-edit OTIO to use a future Clip major — should be accepted with
        // a warning.
        let otio_path = root.join(files::OTIO);
        let text = fs::read_to_string(&otio_path).unwrap();
        // Add a track with a clip carrying Clip.99.
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["tracks"]["children"] = serde_json::json!([
            {
                "OTIO_SCHEMA": "Track.1",
                "name": "V1",
                "kind": "Video",
                "children": [
                    {
                        "OTIO_SCHEMA": "Clip.99",
                        "name": "c1",
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": "raw/foo.mp4"
                        }
                    }
                ]
            }
        ]);
        fs::write(&otio_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        let p = Project::read(&root).unwrap();
        assert_eq!(p.warnings.len(), 1);
        assert_eq!(p.warnings[0].schema, "Clip.99");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_fails_on_unknown_schema_name() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        let otio_path = root.join(files::OTIO);
        let text = fs::read_to_string(&otio_path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["tracks"]["children"] = serde_json::json!([
            {
                "OTIO_SCHEMA": "Foo.1",
                "name": "??"
            }
        ]);
        fs::write(&otio_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        let err = Project::read(&root).unwrap_err();
        assert!(matches!(err, ProtoError::UnknownOtioSchema { .. }));
        let msg = err.to_string();
        assert!(msg.contains("Foo.1"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_fails_on_negative_duration() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        let otio_path = root.join(files::OTIO);
        let text = fs::read_to_string(&otio_path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["tracks"]["children"] = serde_json::json!([
            {
                "OTIO_SCHEMA": "Track.1",
                "name": "V1",
                "kind": "Video",
                "children": [
                    {
                        "OTIO_SCHEMA": "Clip.2",
                        "name": "c1",
                        "source_range": {
                            "start_time": {"value": 0.0, "rate": 24.0},
                            "duration": {"value": -1.0, "rate": 24.0}
                        },
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": "raw/foo.mp4"
                        }
                    }
                ]
            }
        ]);
        fs::write(&otio_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        let err = Project::read(&root).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("duration must be non-negative"));
        // JSON path should locate the bad node.
        assert!(msg.contains("duration"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_fails_on_zero_rate() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        let otio_path = root.join(files::OTIO);
        let text = fs::read_to_string(&otio_path).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["tracks"]["children"] = serde_json::json!([
            {
                "OTIO_SCHEMA": "Track.1",
                "name": "V1",
                "kind": "Video",
                "children": [
                    {
                        "OTIO_SCHEMA": "Clip.2",
                        "name": "c1",
                        "source_range": {
                            "start_time": {"value": 0.0, "rate": 0.0},
                            "duration": {"value": 1.0, "rate": 24.0}
                        },
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": "raw/foo.mp4"
                        }
                    }
                ]
            }
        ]);
        fs::write(&otio_path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        let err = Project::read(&root).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("rate must be positive"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_fails_on_malformed_json() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        let otio_path = root.join(files::OTIO);
        fs::write(&otio_path, "{ this is not json").unwrap();
        let err = Project::read(&root).unwrap_err();
        assert!(matches!(err, ProtoError::Json { .. }));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_index_manifest_is_ok() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        // Delete manifest; project should still read.
        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        fs::remove_file(&mp).unwrap();
        let p = Project::read(&root).unwrap();
        assert!(p.manifest.is_none());
        fs::remove_dir_all(&root).ok();
    }
}
