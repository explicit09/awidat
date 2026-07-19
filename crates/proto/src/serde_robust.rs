//! JSON deserialization helpers that survive `serde_json`'s
//! `arbitrary_precision` feature.
//!
//! Cargo feature unification means ANY crate in the build graph can switch
//! `serde_json` into arbitrary-precision mode for every other crate (today:
//! `starlark` 0.14 via vendored `codex-execpolicy`; see fork patch #9 in
//! `vendor/codex-rs/SOURCE`). In that mode, `from_str`/`from_slice` emit
//! every number as an opaque token map when serde buffers content — which
//! internally-tagged and untagged enums (e.g. `EdlOp`, `tag = "op"`) always
//! do — so `f64` fields fail to deserialize with "invalid type: map".
//!
//! Deserializing to `serde_json::Value` first and then `from_value` is
//! immune: `Value`'s deserializer replays representable numbers as plain
//! numeric visits. Route every typed JSON parse through these helpers
//! instead of calling `serde_json::from_str`/`from_slice` directly.
//!
//! Lives in `montage-proto` (the lowest-level first-party crate) so every
//! other Montage crate can reach it without a dependency cycle;
//! `montage_core::serde_robust` re-exports this module for callers that
//! already reference it there.

use serde::de::DeserializeOwned;

/// `serde_json::from_str`, robust to `arbitrary_precision` in the graph.
pub fn from_json_str<T: DeserializeOwned>(s: &str) -> serde_json::Result<T> {
    let value: serde_json::Value = serde_json::from_str(s)?;
    serde_json::from_value(value)
}

/// `serde_json::from_slice`, robust to `arbitrary_precision` in the graph.
pub fn from_json_slice<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    serde_json::from_value(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_hop_roundtrips_floats() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        #[serde(tag = "kind")]
        enum Tagged {
            A { x: f64 },
        }
        let json = serde_json::to_string(&Tagged::A { x: 1.5 }).unwrap();
        let back: Tagged = from_json_str(&json).unwrap();
        assert_eq!(back, Tagged::A { x: 1.5 });
    }
}
