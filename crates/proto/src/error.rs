//! Typed errors for the proto crate.
//!
//! Every parse/validation error carries a [`JsonPath`] so the CLI can point
//! the user (or the agent) at the exact offending field. Error strings are
//! short, imperative, and actionable per `PLAN.md` §7.2 ("error string
//! discipline").

use std::fmt;

use thiserror::Error;

/// A pointer into a JSON document, accumulated as we descend during parsing
/// or validation.
///
/// Paths use a JSON-Pointer-ish syntax: dot-separated field names, with
/// `[N]` for array indices. Example: `tracks.children[0].source_range.duration`.
///
/// The path is a plain string internally; we append to it as we descend, and
/// emit it as part of the final error message. This is a deliberately
/// simple shape because the engine only consumes it as text.
///
/// **Discipline rule (do not delete):** this type exists *solely* to format
/// human/agent-readable error strings. Do **not** add programmatic accessors
/// like `to_json_pointer()`, `to_jq_path()`, segment iterators, or anything
/// that invites callers to depend on its internal shape. If you find
/// yourself wanting that, write a separate `JsonPointer` type with its own
/// guarantees and migrate from the string. The audit (vs. codex-apply-patch)
/// flagged this as the smallest-but-most-tempting source of future
/// over-engineering.
#[derive(Debug, Clone, Default)]
pub struct JsonPath(String);

impl JsonPath {
    /// Empty root path.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Construct a path from an existing string. The string is taken as-is;
    /// no validation.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns a child path with a field appended.
    #[must_use]
    pub fn field(&self, name: &str) -> Self {
        if self.0.is_empty() {
            Self(name.to_string())
        } else {
            Self(format!("{}.{}", self.0, name))
        }
    }

    /// Returns a child path with an array index appended.
    #[must_use]
    pub fn index(&self, idx: usize) -> Self {
        Self(format!("{}[{}]", self.0, idx))
    }

    /// Returns the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            write!(f, "<root>")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// Top-level error type for the proto crate.
///
/// Engine and CLI catch [`ProtoError`] and route it as
/// `FunctionCallError::RespondToModel` (per `PLAN.md` §7.2) — strings are
/// designed to be readable by both humans and the agent.
#[derive(Debug, Error)]
pub enum ProtoError {
    /// I/O failure reading or writing a project file.
    #[error("io error at {path}: {source}")]
    Io {
        /// Filesystem path the I/O occurred on.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// JSON parse failure (syntax error). Caught from `serde_json`.
    #[error("invalid JSON in '{file}' at line {line}, column {column}: {message}")]
    Json {
        /// File the JSON came from.
        file: String,
        /// Line number reported by `serde_json`.
        line: usize,
        /// Column reported by `serde_json`.
        column: usize,
        /// Human-readable message.
        message: String,
    },

    /// Schema validation failure. Path is the JSON pointer to the bad node.
    #[error("validation error in '{file}' at '{path}': {message}")]
    Validation {
        /// File the bad value came from.
        file: String,
        /// JSON path to the offending field.
        path: JsonPath,
        /// What was wrong.
        message: String,
    },

    /// `OTIO_SCHEMA` references a name we don't know about (not just an
    /// unknown major version of a known name). This is a hard fail per
    /// `PLAN.md` Concern B.
    #[error("unknown OTIO_SCHEMA '{schema}' in '{file}' at '{path}'; supported names: {supported}")]
    UnknownOtioSchema {
        /// File the bad schema came from.
        file: String,
        /// JSON path to the offending node.
        path: JsonPath,
        /// The schema string (e.g. `Foo.1`).
        schema: String,
        /// Comma-separated list of supported names.
        supported: String,
    },

    /// `OTIO_SCHEMA` is malformed (not `Name.Major`).
    #[error("malformed OTIO_SCHEMA '{schema}' in '{file}' at '{path}': expected 'Name.Major'")]
    MalformedOtioSchema {
        /// File the bad schema came from.
        file: String,
        /// JSON path to the node.
        path: JsonPath,
        /// The bad schema string.
        schema: String,
    },
}

impl ProtoError {
    /// Helper to construct a [`ProtoError::Validation`].
    pub fn validation(file: impl Into<String>, path: JsonPath, message: impl Into<String>) -> Self {
        Self::Validation {
            file: file.into(),
            path,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_path_root_is_empty() {
        let p = JsonPath::root();
        assert_eq!(p.as_str(), "");
        assert_eq!(format!("{p}"), "<root>");
    }

    #[test]
    fn json_path_field_appends() {
        let p = JsonPath::root().field("tracks");
        assert_eq!(p.as_str(), "tracks");
        let p = p.field("children");
        assert_eq!(p.as_str(), "tracks.children");
    }

    #[test]
    fn json_path_index_appends() {
        let p = JsonPath::root().field("tracks").field("children").index(2);
        assert_eq!(p.as_str(), "tracks.children[2]");
    }

    #[test]
    fn validation_error_displays_path_and_message() {
        let err = ProtoError::validation(
            "project.otio.json",
            JsonPath::root().field("tracks").field("rate"),
            "rate must be positive, got 0.0",
        );
        let msg = err.to_string();
        assert!(msg.contains("tracks.rate"));
        assert!(msg.contains("rate must be positive"));
    }
}
