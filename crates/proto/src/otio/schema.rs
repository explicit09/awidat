//! OTIO schema string parsing and forward-compat rules.
//!
//! Per `PLAN.md` Concern B: parse `OTIO_SCHEMA: "Name.Major"`, accept known
//! names with unknown majors as forward-compat (warning), reject unknown
//! names hard.

use crate::ProtoError;
use crate::error::JsonPath;

/// A canonical name we recognize and the major version we last shipped.
///
/// Updating this list is the *only* way to add OTIO type support. Adding a
/// new variant in the dispatch enum without updating this list is a bug.
///
/// Listed in display order for error messages. The `expected_major` is the
/// version we map "name only" to when forward-compat triggers; bumping it
/// here is how we accept a new major.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtioSchemaName {
    /// Canonical name component (e.g. `"Clip"`).
    pub name: &'static str,
    /// Major we ship.
    pub expected_major: u32,
}

/// Names supported by the v1 engine, in display order.
///
/// Order matters: error messages enumerate this list, so put the most
/// "core" types first.
pub const SUPPORTED_SCHEMA_NAMES: &[OtioSchemaName] = &[
    OtioSchemaName {
        name: "Timeline",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "Stack",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "Track",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "Clip",
        expected_major: 2,
    },
    OtioSchemaName {
        name: "Gap",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "Transition",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "ExternalReference",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "MissingReference",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "Effect",
        expected_major: 1,
    },
    OtioSchemaName {
        name: "Marker",
        expected_major: 2,
    },
];

/// Outcome of parsing an `OTIO_SCHEMA` string.
#[derive(Debug)]
pub enum SchemaParseOutcome {
    /// Known name + matching major. The happy path.
    Exact {
        /// Canonical name component.
        name: &'static str,
        /// Major found, equal to the supported major.
        major: u32,
    },
    /// Known name, but major differs from what we ship. Caller should log
    /// the warning and continue parsing as the known major.
    ForwardCompat {
        /// Canonical name.
        name: &'static str,
        /// Major found on disk.
        file_major: u32,
        /// Major we ship.
        expected_major: u32,
    },
}

/// A non-fatal warning about a forward-compat schema mismatch.
///
/// Validation surfaces these via [`crate::validate::ValidationReport`] so
/// the CLI can print them as warnings without failing the load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaWarning {
    /// Logical file the schema came from (e.g. `"project.otio.json"`).
    pub file: String,
    /// JSON path to the node carrying the schema.
    pub path: String,
    /// Schema string as it appeared on disk.
    pub schema: String,
    /// Major version we expected.
    pub expected_major: u32,
    /// Major version found.
    pub found_major: u32,
}

/// Parse an `OTIO_SCHEMA` string (e.g. `"Clip.1"`).
///
/// Returns:
/// - [`SchemaParseOutcome::Exact`] if name and major both match a supported
///   entry.
/// - [`SchemaParseOutcome::ForwardCompat`] if the name is supported but the
///   major isn't — caller continues parsing as the supported major.
/// - [`ProtoError::UnknownOtioSchema`] if the name isn't supported.
/// - [`ProtoError::MalformedOtioSchema`] if the string isn't `Name.Major`.
pub fn parse_schema_string(
    file: &str,
    path: &JsonPath,
    schema: &str,
) -> Result<SchemaParseOutcome, ProtoError> {
    let (name_str, major_str) =
        schema
            .rsplit_once('.')
            .ok_or_else(|| ProtoError::MalformedOtioSchema {
                file: file.to_string(),
                path: path.clone(),
                schema: schema.to_string(),
            })?;

    let major: u32 = major_str
        .parse()
        .map_err(|_| ProtoError::MalformedOtioSchema {
            file: file.to_string(),
            path: path.clone(),
            schema: schema.to_string(),
        })?;

    let entry = SUPPORTED_SCHEMA_NAMES
        .iter()
        .find(|e| e.name == name_str)
        .ok_or_else(|| ProtoError::UnknownOtioSchema {
            file: file.to_string(),
            path: path.clone(),
            schema: schema.to_string(),
            supported: SUPPORTED_SCHEMA_NAMES
                .iter()
                .map(|e| format!("{}.{}", e.name, e.expected_major))
                .collect::<Vec<_>>()
                .join(", "),
        })?;

    if major == entry.expected_major {
        Ok(SchemaParseOutcome::Exact {
            name: entry.name,
            major,
        })
    } else {
        Ok(SchemaParseOutcome::ForwardCompat {
            name: entry.name,
            file_major: major,
            expected_major: entry.expected_major,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact() {
        let out = parse_schema_string("p.json", &JsonPath::root(), "Clip.2").unwrap();
        match out {
            SchemaParseOutcome::Exact { name, major } => {
                assert_eq!(name, "Clip");
                assert_eq!(major, 2);
            }
            SchemaParseOutcome::ForwardCompat { .. } => panic!("expected Exact"),
        }
    }

    #[test]
    fn parses_forward_compat_for_known_name_unknown_major() {
        // We ship Clip.2; a hypothetical Clip.7 must be accepted as
        // forward-compat (read as our supported major, with a warning).
        let out = parse_schema_string("p.json", &JsonPath::root(), "Clip.7").unwrap();
        match out {
            SchemaParseOutcome::ForwardCompat {
                name,
                file_major,
                expected_major,
            } => {
                assert_eq!(name, "Clip");
                assert_eq!(file_major, 7);
                assert_eq!(expected_major, 2);
            }
            SchemaParseOutcome::Exact { .. } => panic!("expected ForwardCompat"),
        }
    }

    #[test]
    fn rejects_unknown_name() {
        let err = parse_schema_string("p.json", &JsonPath::root(), "Foo.1").unwrap_err();
        assert!(matches!(err, ProtoError::UnknownOtioSchema { .. }));
        let msg = err.to_string();
        assert!(msg.contains("Foo.1"));
        assert!(msg.contains("Clip"));
    }

    #[test]
    fn rejects_malformed() {
        let err = parse_schema_string("p.json", &JsonPath::root(), "Clip").unwrap_err();
        assert!(matches!(err, ProtoError::MalformedOtioSchema { .. }));
    }

    #[test]
    fn rejects_non_numeric_major() {
        let err = parse_schema_string("p.json", &JsonPath::root(), "Clip.foo").unwrap_err();
        assert!(matches!(err, ProtoError::MalformedOtioSchema { .. }));
    }

    #[test]
    fn all_supported_names_unique() {
        let mut names: Vec<&str> = SUPPORTED_SCHEMA_NAMES.iter().map(|e| e.name).collect();
        names.sort_unstable();
        let n_before = names.len();
        names.dedup();
        assert_eq!(names.len(), n_before, "duplicate schema name");
    }
}
