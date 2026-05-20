//! `.cube` LUT parser with structured validation errors.
//!
//! Sits upstream of FFmpeg's `lut3d` filter so malformed LUTs fail at
//! `apply_edl` time with named error variants instead of mid-render
//! with opaque FFmpeg strings. Mirrors the parser surface that other
//! NLEs implement (Freecut's `cube-lut.ts`, Kdenlive's pre-validator,
//! Olive's OCIO config check). 1D vs 3D is detected from the header;
//! callers that only accept one shape can use [`parse_cube_3d`] or
//! [`parse_cube_1d`] for an explicit mismatch error.

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Smallest cube size accepted (size 1 is degenerate; size 2 is the
/// minimum meaningful LUT — eight corners of the cube).
pub const MIN_SIZE: usize = 2;

/// Largest 3D LUT edge length accepted. 65 is the largest commonly
/// shipped (DaVinci's max); 17 and 33 are typical. Anything bigger is
/// almost certainly a malformed file masquerading as a 3D cube.
pub const MAX_SIZE_3D: usize = 65;

/// Largest 1D LUT entry count accepted. 1024 / 4096 are typical;
/// 65536 is a generous upper bound that still rejects pathological
/// allocations.
pub const MAX_SIZE_1D: usize = 65_536;

/// Which kind of LUT was parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LutKind {
    /// 3D LUT declared with `LUT_3D_SIZE`.
    Three,
    /// 1D LUT declared with `LUT_1D_SIZE`.
    One,
}

/// Parsed 3D LUT.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut3d {
    /// Cube edge length. Table holds `size.pow(3)` RGB triplets.
    pub size: usize,
    /// Per-channel input lower bound (default `[0.0, 0.0, 0.0]`).
    pub domain_min: [f32; 3],
    /// Per-channel input upper bound (default `[1.0, 1.0, 1.0]`).
    pub domain_max: [f32; 3],
    /// Optional `TITLE` value with surrounding quotes stripped.
    pub title: Option<String>,
    /// Flattened RGB table laid out R-fastest, G-middle, B-slowest
    /// per the Iridas `.cube` spec. Channel `c` of grid cell
    /// `(r, g, b)` is at index `((b * size + g) * size + r) * 3 + c`.
    /// Length is `size.pow(3) * 3`.
    pub table: Vec<f32>,
}

/// Parsed 1D LUT.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut1d {
    /// Number of entries.
    pub size: usize,
    /// Per-channel input lower bound (default `[0.0, 0.0, 0.0]`).
    pub domain_min: [f32; 3],
    /// Per-channel input upper bound (default `[1.0, 1.0, 1.0]`).
    pub domain_max: [f32; 3],
    /// Optional `TITLE` value with surrounding quotes stripped.
    pub title: Option<String>,
    /// Flattened RGB table. Channel `c` of entry `i` is at index
    /// `i * 3 + c`. Length is `size * 3`.
    pub table: Vec<f32>,
}

/// Parsed LUT in either flavor.
#[derive(Debug, Clone, PartialEq)]
pub enum Lut {
    /// 3D LUT.
    Three(Lut3d),
    /// 1D LUT.
    One(Lut1d),
}

impl Lut {
    /// Which flavor this LUT is.
    pub fn kind(&self) -> LutKind {
        match self {
            Lut::Three(_) => LutKind::Three,
            Lut::One(_) => LutKind::One,
        }
    }

    /// SHA-256 hash of the parsed LUT contents — kind tag, size,
    /// domain, table values — suitable as a content-addressed cache
    /// key. Identical tables produce identical hashes regardless of
    /// the source file's formatting (whitespace, comments, BOM,
    /// quoted vs. unquoted TITLE) because the hash sees only the
    /// parsed normal form.
    pub fn content_hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        match self {
            Lut::Three(l) => {
                hasher.update(b"3D");
                hash_inner(&mut hasher, l.size, &l.domain_min, &l.domain_max, &l.table);
            }
            Lut::One(l) => {
                hasher.update(b"1D");
                hash_inner(&mut hasher, l.size, &l.domain_min, &l.domain_max, &l.table);
            }
        }
        hasher.finalize().into()
    }

    /// [`Lut::content_hash`] rendered as lowercase hex. Useful for
    /// cache filenames like `luts/cache/<hash>.cube`.
    pub fn content_hash_hex(&self) -> String {
        hex_encode(&self.content_hash())
    }
}

fn hash_inner(
    hasher: &mut Sha256,
    size: usize,
    domain_min: &[f32; 3],
    domain_max: &[f32; 3],
    table: &[f32],
) {
    hasher.update((size as u64).to_le_bytes());
    for value in domain_min {
        hasher.update(value.to_le_bytes());
    }
    for value in domain_max {
        hasher.update(value.to_le_bytes());
    }
    for value in table {
        hasher.update(value.to_le_bytes());
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // `unwrap_used` is denied in this crate, but write! on a
        // String never fails — using a manual nibble lookup avoids
        // needing the macro at all.
        out.push(NIBBLE[(byte >> 4) as usize] as char);
        out.push(NIBBLE[(byte & 0x0F) as usize] as char);
    }
    out
}

const NIBBLE: &[u8; 16] = b"0123456789abcdef";

/// Why a `.cube` file failed to parse. Distinct variants per failure
/// mode so callers (especially the agent layer) can react
/// programmatically instead of scraping a string.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum LutValidationError {
    /// File has no non-comment, non-whitespace content.
    #[error("LUT is empty or contains only whitespace/comments")]
    Empty,
    /// Neither `LUT_3D_SIZE` nor `LUT_1D_SIZE` was found.
    #[error("LUT declares no LUT_3D_SIZE or LUT_1D_SIZE")]
    MissingSize,
    /// Both size keywords appeared in the same file.
    #[error("LUT declares both LUT_3D_SIZE and LUT_1D_SIZE")]
    ConflictingSize,
    /// Declared size is outside the supported inclusive range.
    #[error("LUT_{kind} size {got} is outside supported range {min}..={max}")]
    SizeOutOfRange {
        /// `"3D"` or `"1D"`.
        kind: &'static str,
        /// Value parsed from the header.
        got: usize,
        /// Inclusive lower bound.
        min: usize,
        /// Inclusive upper bound.
        max: usize,
    },
    /// A header line (TITLE / SIZE / DOMAIN_*) was malformed.
    #[error("malformed header on line {line}: {detail}")]
    MalformedHeader {
        /// 1-based line number in the source.
        line: usize,
        /// Human-readable detail.
        detail: String,
    },
    /// Number of data rows did not match the declared size.
    #[error("expected {expected} table rows, got {got}")]
    RowCountMismatch {
        /// `size.pow(3)` for 3D, `size` for 1D.
        expected: usize,
        /// Actual non-empty, non-comment row count.
        got: usize,
    },
    /// A data row could not be parsed as three numbers.
    #[error("malformed table row on line {line}: {detail}")]
    MalformedRow {
        /// 1-based line number in the source.
        line: usize,
        /// Human-readable detail.
        detail: String,
    },
    /// A parsed number was NaN or infinite.
    #[error("non-finite value on line {line}")]
    NonFinite {
        /// 1-based line number in the source.
        line: usize,
    },
    /// `DOMAIN_MIN >= DOMAIN_MAX` for at least one channel.
    #[error("DOMAIN_MIN >= DOMAIN_MAX on channel {channel} (min={min}, max={max})")]
    DomainInverted {
        /// `R`, `G`, or `B`.
        channel: char,
        /// Lower bound for that channel.
        min: f32,
        /// Upper bound for that channel.
        max: f32,
    },
    /// File parsed as a 1D LUT but the caller asked for a 3D LUT.
    #[error("LUT is 1D but a 3D LUT was expected")]
    Was1DExpectedThree,
    /// File parsed as a 3D LUT but the caller asked for a 1D LUT.
    #[error("LUT is 3D but a 1D LUT was expected")]
    Was3DExpectedOne,
}

/// Parse a `.cube` LUT (1D or 3D) from its text contents.
///
/// Tolerates a UTF-8 BOM, CRLF/LF line endings, `#` comments (both
/// full-line and trailing), blank lines, and `TITLE` with or without
/// surrounding quotes. Headers may appear in any order but must
/// precede the first data row.
pub fn parse_cube(src: &str) -> Result<Lut, LutValidationError> {
    let stripped = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut state = ParserState::default();
    for (idx, raw) in stripped.split('\n').enumerate() {
        let line_no = idx + 1;
        let no_cr = raw.strip_suffix('\r').unwrap_or(raw);
        let no_comment = match no_cr.split_once('#') {
            Some((before, _)) => before,
            None => no_cr,
        };
        let line = no_comment.trim();
        if line.is_empty() {
            continue;
        }
        state.consume_line(line_no, line)?;
    }
    state.finish()
}

/// Parse a `.cube` and require it to be 3D.
pub fn parse_cube_3d(src: &str) -> Result<Lut3d, LutValidationError> {
    match parse_cube(src)? {
        Lut::Three(lut) => Ok(lut),
        Lut::One(_) => Err(LutValidationError::Was1DExpectedThree),
    }
}

/// Parse a `.cube` and require it to be 1D.
pub fn parse_cube_1d(src: &str) -> Result<Lut1d, LutValidationError> {
    match parse_cube(src)? {
        Lut::One(lut) => Ok(lut),
        Lut::Three(_) => Err(LutValidationError::Was3DExpectedOne),
    }
}

/// Parse a `.3dl` LUT (Lustre / Quantel / After Effects style).
///
/// The format is integer-valued and either prefixed by a mesh-point
/// header (a line of ≥4 ascending integers giving both the cube
/// edge length and the max input value) or headerless (cube edge
/// length inferred from `cbrt(row_count)`).
///
/// Each data row is three non-negative integers. Values are
/// normalized to `[0.0, 1.0]` by dividing by the mesh-header max
/// (or the largest value in the file when the header is absent).
///
/// Like [`parse_cube`], tolerates BOM, CRLF, `#` line/trailing
/// comments, and blank lines. There is no DOMAIN keyword in
/// `.3dl`; the domain is always `[0, 1]` after normalization.
pub fn parse_3dl(src: &str) -> Result<Lut3d, LutValidationError> {
    let stripped = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut mesh_header: Option<Vec<u32>> = None;
    let mut data: Vec<u32> = Vec::new();
    let mut row_count: usize = 0;
    let mut saw_first_significant = false;

    for (idx, raw) in stripped.split('\n').enumerate() {
        let line_no = idx + 1;
        let no_cr = raw.strip_suffix('\r').unwrap_or(raw);
        let no_comment = match no_cr.split_once('#') {
            Some((before, _)) => before,
            None => no_cr,
        };
        let line = no_comment.trim();
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();

        // First non-empty line is either a mesh-point header
        // (strictly ascending integers, ≥2 entries) or a data row.
        // Token count alone is insufficient because a 3-cube's
        // header and its data rows both have 3 tokens; "strictly
        // ascending" reliably tells them apart since data rows
        // either repeat (identity LUT) or zigzag.
        if !saw_first_significant {
            let parsed: Result<Vec<u32>, _> = tokens.iter().map(|t| t.parse::<u32>()).collect();
            if let Ok(mesh) = parsed {
                let strictly_ascending = mesh.len() >= 2 && mesh.windows(2).all(|w| w[0] < w[1]);
                if strictly_ascending {
                    if !(MIN_SIZE..=MAX_SIZE_3D).contains(&mesh.len()) {
                        return Err(LutValidationError::SizeOutOfRange {
                            kind: "3D",
                            got: mesh.len(),
                            min: MIN_SIZE,
                            max: MAX_SIZE_3D,
                        });
                    }
                    mesh_header = Some(mesh);
                    saw_first_significant = true;
                    continue;
                }
            }
            // Otherwise fall through to data-row handling — the
            // file is in the headerless variant.
        }

        if tokens.len() != 3 {
            return Err(LutValidationError::MalformedRow {
                line: line_no,
                detail: format!("expected 3 integers, got {}", tokens.len()),
            });
        }
        for (channel_idx, tok) in tokens.iter().enumerate() {
            let v: u32 = tok.parse().map_err(|_| LutValidationError::MalformedRow {
                line: line_no,
                detail: format!(
                    "channel {} value {tok:?} is not a non-negative integer",
                    match channel_idx {
                        0 => "R",
                        1 => "G",
                        _ => "B",
                    }
                ),
            })?;
            data.push(v);
        }
        row_count += 1;
        saw_first_significant = true;
    }

    if row_count == 0 {
        return Err(LutValidationError::Empty);
    }

    // Determine cube edge length: prefer the explicit mesh-point
    // header; otherwise infer from the cube root of the row count.
    let size = if let Some(mesh) = &mesh_header {
        mesh.len()
    } else {
        let cube_root = (row_count as f64).cbrt().round() as usize;
        if !(MIN_SIZE..=MAX_SIZE_3D).contains(&cube_root) {
            return Err(LutValidationError::SizeOutOfRange {
                kind: "3D",
                got: cube_root,
                min: MIN_SIZE,
                max: MAX_SIZE_3D,
            });
        }
        cube_root
    };

    let expected = size.pow(3);
    if row_count != expected {
        return Err(LutValidationError::RowCountMismatch {
            expected,
            got: row_count,
        });
    }

    // Normalization max: the mesh header's last value if present,
    // otherwise the largest seen data value. Clamped to ≥1 so we
    // never divide by zero on a degenerate identity LUT (all zeros).
    let max_value = mesh_header
        .as_ref()
        .and_then(|m| m.last().copied())
        .unwrap_or_else(|| data.iter().copied().max().unwrap_or(1023))
        .max(1) as f32;
    let table: Vec<f32> = data.iter().map(|&v| (v as f32) / max_value).collect();

    Ok(Lut3d {
        size,
        domain_min: [0.0, 0.0, 0.0],
        domain_max: [1.0, 1.0, 1.0],
        title: None,
        table,
    })
}

#[derive(Default)]
struct ParserState {
    title: Option<String>,
    size_3d: Option<usize>,
    size_1d: Option<usize>,
    domain_min: Option<[f32; 3]>,
    domain_max: Option<[f32; 3]>,
    table: Vec<f32>,
    row_count: usize,
}

impl ParserState {
    fn consume_line(&mut self, line: usize, text: &str) -> Result<(), LutValidationError> {
        let mut tokens = text.split_whitespace();
        let Some(first) = tokens.next() else {
            return Ok(());
        };
        let upper = first.to_ascii_uppercase();
        match upper.as_str() {
            "TITLE" => {
                self.guard_pre_data(line, "TITLE")?;
                let rest = text.split_once(char::is_whitespace).map(|(_, r)| r.trim());
                self.title = Some(rest.unwrap_or("").trim_matches('"').to_string());
                Ok(())
            }
            "LUT_3D_SIZE" => {
                self.guard_pre_data(line, "LUT_3D_SIZE")?;
                if self.size_3d.is_some() {
                    return Err(LutValidationError::MalformedHeader {
                        line,
                        detail: "duplicate LUT_3D_SIZE".into(),
                    });
                }
                let size = parse_size(line, tokens.next())?;
                if !(MIN_SIZE..=MAX_SIZE_3D).contains(&size) {
                    return Err(LutValidationError::SizeOutOfRange {
                        kind: "3D",
                        got: size,
                        min: MIN_SIZE,
                        max: MAX_SIZE_3D,
                    });
                }
                self.size_3d = Some(size);
                Ok(())
            }
            "LUT_1D_SIZE" => {
                self.guard_pre_data(line, "LUT_1D_SIZE")?;
                if self.size_1d.is_some() {
                    return Err(LutValidationError::MalformedHeader {
                        line,
                        detail: "duplicate LUT_1D_SIZE".into(),
                    });
                }
                let size = parse_size(line, tokens.next())?;
                if !(MIN_SIZE..=MAX_SIZE_1D).contains(&size) {
                    return Err(LutValidationError::SizeOutOfRange {
                        kind: "1D",
                        got: size,
                        min: MIN_SIZE,
                        max: MAX_SIZE_1D,
                    });
                }
                self.size_1d = Some(size);
                Ok(())
            }
            "DOMAIN_MIN" => {
                self.guard_pre_data(line, "DOMAIN_MIN")?;
                self.domain_min = Some(parse_triple(line, tokens)?);
                Ok(())
            }
            "DOMAIN_MAX" => {
                self.guard_pre_data(line, "DOMAIN_MAX")?;
                self.domain_max = Some(parse_triple(line, tokens)?);
                Ok(())
            }
            _ => {
                let triple = parse_triple(line, text.split_whitespace())?;
                self.table.extend_from_slice(&triple);
                self.row_count += 1;
                Ok(())
            }
        }
    }

    fn guard_pre_data(&self, line: usize, header: &str) -> Result<(), LutValidationError> {
        if self.row_count > 0 {
            return Err(LutValidationError::MalformedHeader {
                line,
                detail: format!("{header} after table rows"),
            });
        }
        Ok(())
    }

    fn finish(self) -> Result<Lut, LutValidationError> {
        match (self.size_3d, self.size_1d) {
            (Some(_), Some(_)) => Err(LutValidationError::ConflictingSize),
            (None, None) if self.row_count == 0 && self.title.is_none() => {
                Err(LutValidationError::Empty)
            }
            (None, None) => Err(LutValidationError::MissingSize),
            (Some(size), None) => {
                let expected = size.pow(3);
                if self.row_count != expected {
                    return Err(LutValidationError::RowCountMismatch {
                        expected,
                        got: self.row_count,
                    });
                }
                let (domain_min, domain_max) = finalize_domain(self.domain_min, self.domain_max)?;
                Ok(Lut::Three(Lut3d {
                    size,
                    domain_min,
                    domain_max,
                    title: self.title,
                    table: self.table,
                }))
            }
            (None, Some(size)) => {
                if self.row_count != size {
                    return Err(LutValidationError::RowCountMismatch {
                        expected: size,
                        got: self.row_count,
                    });
                }
                let (domain_min, domain_max) = finalize_domain(self.domain_min, self.domain_max)?;
                Ok(Lut::One(Lut1d {
                    size,
                    domain_min,
                    domain_max,
                    title: self.title,
                    table: self.table,
                }))
            }
        }
    }
}

fn finalize_domain(
    min: Option<[f32; 3]>,
    max: Option<[f32; 3]>,
) -> Result<([f32; 3], [f32; 3]), LutValidationError> {
    let domain_min = min.unwrap_or([0.0, 0.0, 0.0]);
    let domain_max = max.unwrap_or([1.0, 1.0, 1.0]);
    for (idx, channel) in ['R', 'G', 'B'].into_iter().enumerate() {
        if domain_min[idx] >= domain_max[idx] {
            return Err(LutValidationError::DomainInverted {
                channel,
                min: domain_min[idx],
                max: domain_max[idx],
            });
        }
    }
    Ok((domain_min, domain_max))
}

fn parse_size(line: usize, token: Option<&str>) -> Result<usize, LutValidationError> {
    let Some(tok) = token else {
        return Err(LutValidationError::MalformedHeader {
            line,
            detail: "size keyword missing value".into(),
        });
    };
    tok.parse::<usize>()
        .map_err(|_| LutValidationError::MalformedHeader {
            line,
            detail: format!("size value {tok:?} is not a non-negative integer"),
        })
}

fn parse_triple<'a, I>(line: usize, mut tokens: I) -> Result<[f32; 3], LutValidationError>
where
    I: Iterator<Item = &'a str>,
{
    let mut out = [0.0_f32; 3];
    for (idx, label) in ['R', 'G', 'B'].into_iter().enumerate() {
        let Some(tok) = tokens.next() else {
            return Err(LutValidationError::MalformedRow {
                line,
                detail: format!("expected 3 numbers, missing channel {label}"),
            });
        };
        let value: f32 = tok.parse().map_err(|_| LutValidationError::MalformedRow {
            line,
            detail: format!("channel {label} value {tok:?} is not a number"),
        })?;
        if !value.is_finite() {
            return Err(LutValidationError::NonFinite { line });
        }
        out[idx] = value;
    }
    if tokens.next().is_some() {
        return Err(LutValidationError::MalformedRow {
            line,
            detail: "expected 3 numbers, got extra tokens".into(),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn cube_3d(size: usize) -> String {
        let mut s = format!("LUT_3D_SIZE {size}\n");
        for bi in 0..size {
            for gi in 0..size {
                for ri in 0..size {
                    let r = ri as f32 / (size - 1) as f32;
                    let g = gi as f32 / (size - 1) as f32;
                    let b = bi as f32 / (size - 1) as f32;
                    s.push_str(&format!("{r:.6} {g:.6} {b:.6}\n"));
                }
            }
        }
        s
    }

    fn cube_1d(size: usize) -> String {
        let mut s = format!("LUT_1D_SIZE {size}\n");
        for i in 0..size {
            let v = i as f32 / (size - 1) as f32;
            s.push_str(&format!("{v:.6} {v:.6} {v:.6}\n"));
        }
        s
    }

    #[test]
    fn parses_minimal_3d_lut() {
        let lut = parse_cube_3d(&cube_3d(2)).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.table.len(), 2 * 2 * 2 * 3);
        assert_eq!(lut.domain_min, [0.0, 0.0, 0.0]);
        assert_eq!(lut.domain_max, [1.0, 1.0, 1.0]);
        assert_eq!(lut.title, None);
    }

    #[test]
    fn parses_3d_lut_with_title_and_domain() {
        let mut src = String::from("TITLE \"warm gold\"\n");
        src.push_str("DOMAIN_MIN 0.0 0.0 0.0\n");
        src.push_str("DOMAIN_MAX 1.0 1.0 1.0\n");
        src.push_str(&cube_3d(2));
        let lut = parse_cube_3d(&src).unwrap();
        assert_eq!(lut.title.as_deref(), Some("warm gold"));
    }

    #[test]
    fn parses_1d_lut() {
        let lut = parse_cube_1d(&cube_1d(8)).unwrap();
        assert_eq!(lut.size, 8);
        assert_eq!(lut.table.len(), 8 * 3);
    }

    #[test]
    fn tolerates_utf8_bom() {
        let mut src = String::from("\u{FEFF}");
        src.push_str(&cube_3d(2));
        let lut = parse_cube_3d(&src).unwrap();
        assert_eq!(lut.size, 2);
    }

    #[test]
    fn tolerates_crlf_line_endings() {
        let src = cube_3d(2).replace('\n', "\r\n");
        parse_cube_3d(&src).unwrap();
    }

    #[test]
    fn tolerates_full_line_and_trailing_comments() {
        let mut src = String::from("# header comment\n");
        src.push_str("LUT_3D_SIZE 2 # cube edge length\n");
        for bi in 0..2 {
            for gi in 0..2 {
                for ri in 0..2 {
                    let r = ri as f32;
                    let g = gi as f32;
                    let b = bi as f32;
                    src.push_str(&format!("{r} {g} {b} # entry\n"));
                }
            }
        }
        parse_cube_3d(&src).unwrap();
    }

    #[test]
    fn tolerates_blank_lines() {
        let mut src = String::from("\n\nLUT_3D_SIZE 2\n\n");
        for bi in 0..2 {
            for gi in 0..2 {
                for ri in 0..2 {
                    src.push_str(&format!("{ri}.0 {gi}.0 {bi}.0\n\n"));
                }
            }
        }
        parse_cube_3d(&src).unwrap();
    }

    #[test]
    fn rejects_missing_size() {
        let err = parse_cube("0.0 0.0 0.0\n0.0 0.0 0.0\n").unwrap_err();
        // Without a size header but with data rows we still surface
        // MissingSize: the LUT is not empty, just unsized.
        assert_eq!(err, LutValidationError::MissingSize);
    }

    #[test]
    fn rejects_empty_file() {
        let err = parse_cube("").unwrap_err();
        assert_eq!(err, LutValidationError::Empty);
    }

    #[test]
    fn rejects_whitespace_and_comment_only_file() {
        let err = parse_cube("# only comment\n\n   \n").unwrap_err();
        assert_eq!(err, LutValidationError::Empty);
    }

    #[test]
    fn rejects_conflicting_size_keywords() {
        let src = "LUT_3D_SIZE 2\nLUT_1D_SIZE 2\n0 0 0\n0 0 0\n";
        let err = parse_cube(src).unwrap_err();
        assert_eq!(err, LutValidationError::ConflictingSize);
    }

    #[test]
    fn rejects_3d_size_too_small() {
        let err = parse_cube("LUT_3D_SIZE 1\n0 0 0\n").unwrap_err();
        assert!(matches!(
            err,
            LutValidationError::SizeOutOfRange {
                kind: "3D",
                got: 1,
                ..
            }
        ));
    }

    #[test]
    fn rejects_3d_size_too_large() {
        let err = parse_cube("LUT_3D_SIZE 200\n").unwrap_err();
        assert!(matches!(
            err,
            LutValidationError::SizeOutOfRange {
                kind: "3D",
                got: 200,
                ..
            }
        ));
    }

    #[test]
    fn rejects_row_count_mismatch() {
        let src = "LUT_3D_SIZE 2\n0 0 0\n0 0 0\n";
        let err = parse_cube(src).unwrap_err();
        assert!(matches!(
            err,
            LutValidationError::RowCountMismatch {
                expected: 8,
                got: 2
            }
        ));
    }

    #[test]
    fn rejects_non_finite_value() {
        let src = "LUT_3D_SIZE 2\nNaN 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n0 0 0\n";
        let err = parse_cube(src).unwrap_err();
        assert!(matches!(err, LutValidationError::NonFinite { .. }));
    }

    #[test]
    fn rejects_malformed_row_too_few_tokens() {
        let src = "LUT_3D_SIZE 2\n0 0\n";
        let err = parse_cube(src).unwrap_err();
        assert!(matches!(err, LutValidationError::MalformedRow { .. }));
    }

    #[test]
    fn rejects_malformed_row_extra_tokens() {
        let src = "LUT_3D_SIZE 2\n0 0 0 0\n";
        let err = parse_cube(src).unwrap_err();
        assert!(matches!(err, LutValidationError::MalformedRow { .. }));
    }

    #[test]
    fn rejects_malformed_row_non_numeric() {
        let src = "LUT_3D_SIZE 2\nfoo bar baz\n";
        let err = parse_cube(src).unwrap_err();
        assert!(matches!(err, LutValidationError::MalformedRow { .. }));
    }

    #[test]
    fn rejects_inverted_domain() {
        let mut src =
            String::from("LUT_3D_SIZE 2\nDOMAIN_MIN 1.0 0.0 0.0\nDOMAIN_MAX 0.0 1.0 1.0\n");
        for _ in 0..8 {
            src.push_str("0 0 0\n");
        }
        let err = parse_cube(&src).unwrap_err();
        assert!(matches!(
            err,
            LutValidationError::DomainInverted { channel: 'R', .. }
        ));
    }

    #[test]
    fn rejects_header_after_data() {
        let src = "LUT_3D_SIZE 2\n0 0 0\nDOMAIN_MIN 0 0 0\n";
        let err = parse_cube(src).unwrap_err();
        assert!(matches!(err, LutValidationError::MalformedHeader { .. }));
    }

    #[test]
    fn parse_cube_3d_rejects_a_1d_lut() {
        let err = parse_cube_3d(&cube_1d(4)).unwrap_err();
        assert_eq!(err, LutValidationError::Was1DExpectedThree);
    }

    #[test]
    fn parse_cube_1d_rejects_a_3d_lut() {
        let err = parse_cube_1d(&cube_3d(2)).unwrap_err();
        assert_eq!(err, LutValidationError::Was3DExpectedOne);
    }

    #[test]
    fn content_hash_is_deterministic_across_formatting() {
        // Two formattings of the same minimal 2-cube identity LUT
        // (different TITLE, CRLF, comments) should hash the same.
        let plain = cube_3d(2);
        let mut decorated = String::from("\u{FEFF}TITLE \"identity\"\n");
        decorated.push_str("# annotated\n");
        for line in plain.lines() {
            decorated.push_str(line);
            decorated.push_str("\r\n");
        }
        let a = parse_cube_3d(&plain).unwrap();
        let b = parse_cube_3d(&decorated).unwrap();
        assert_eq!(a.table, b.table);
        let hash_a = Lut::Three(a).content_hash();
        let hash_b = Lut::Three(b).content_hash();
        assert_eq!(hash_a, hash_b, "identical tables must hash identically");
    }

    #[test]
    fn content_hash_differs_for_different_tables() {
        let a = parse_cube_3d(&cube_3d(2)).unwrap();
        // Same size, but flip the corner so the table differs.
        let mut perturbed = a.clone();
        perturbed.table[0] = 0.5;
        let hash_a = Lut::Three(a).content_hash();
        let hash_b = Lut::Three(perturbed).content_hash();
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn content_hash_hex_is_64_lowercase_chars() {
        let lut = parse_cube_3d(&cube_3d(2)).unwrap();
        let hex = Lut::Three(lut).content_hash_hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    fn build_3dl_identity(size: usize, max_value: u32) -> String {
        // Build a mesh-point header for an evenly-spaced grid, then
        // emit the size³ identity entries (each component scaled to
        // `max_value`).
        let mut s = String::new();
        for i in 0..size {
            let v = (i * max_value as usize) / (size - 1);
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&v.to_string());
        }
        s.push('\n');
        let scale = |i: usize| (i * max_value as usize) / (size - 1);
        for bi in 0..size {
            for gi in 0..size {
                for ri in 0..size {
                    s.push_str(&format!("{} {} {}\n", scale(ri), scale(gi), scale(bi)));
                }
            }
        }
        s
    }

    #[test]
    fn parses_3dl_with_mesh_header() {
        let src = build_3dl_identity(3, 1023);
        let lut = parse_3dl(&src).unwrap();
        assert_eq!(lut.size, 3);
        assert_eq!(lut.table.len(), 3 * 3 * 3 * 3);
        // Corner (r=0,g=0,b=0) is black.
        assert!((lut.table[0] - 0.0).abs() < 1e-6);
        // Corner (r=2,g=2,b=2) is white after normalization.
        let last_base = ((2 * 3 + 2) * 3 + 2) * 3;
        assert!((lut.table[last_base] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parses_3dl_without_mesh_header_via_cuberoot() {
        // Headerless: just 2³ = 8 data rows, max value 65535 (16-bit).
        let mut s = String::new();
        let max_value: u32 = 65_535;
        for bi in 0..2 {
            for gi in 0..2 {
                for ri in 0..2 {
                    s.push_str(&format!(
                        "{} {} {}\n",
                        ri * max_value as usize,
                        gi * max_value as usize,
                        bi * max_value as usize,
                    ));
                }
            }
        }
        let lut = parse_3dl(&s).unwrap();
        assert_eq!(lut.size, 2);
        // (1,0,0) corner normalizes to 1.0 via inferred max.
        assert!((lut.table[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_3dl_with_wrong_row_count() {
        let src = "0 1023\n0 0 0\n";
        let err = parse_3dl(src).unwrap_err();
        // Mesh size 2 expects 8 rows; we gave 1.
        assert!(matches!(
            err,
            LutValidationError::RowCountMismatch {
                expected: 8,
                got: 1
            }
        ));
    }

    #[test]
    fn rejects_3dl_with_non_integer_value() {
        let src = "0 1023\n0.5 0 0\n";
        let err = parse_3dl(src).unwrap_err();
        assert!(matches!(err, LutValidationError::MalformedRow { .. }));
    }

    #[test]
    fn rejects_empty_3dl() {
        assert_eq!(parse_3dl("").unwrap_err(), LutValidationError::Empty);
        assert_eq!(
            parse_3dl("# comment only\n").unwrap_err(),
            LutValidationError::Empty
        );
    }

    #[test]
    fn tolerates_crlf_and_comments_in_3dl() {
        let mut src = build_3dl_identity(2, 1023).replace('\n', "\r\n");
        src.insert_str(0, "# 3D LUT v1\r\n");
        let lut = parse_3dl(&src).unwrap();
        assert_eq!(lut.size, 2);
    }

    #[test]
    fn table_layout_is_r_fastest() {
        // For a size-2 identity LUT the entry at (r=1, g=0, b=0) sits
        // at index 1 in row-major R-fastest order — check that the
        // channel values land where the docstring claims.
        let lut = parse_cube_3d(&cube_3d(2)).unwrap();
        let size = lut.size;
        let idx = |r: usize, g: usize, b: usize| ((b * size + g) * size + r) * 3;
        let (r, g, b) = (1, 0, 0);
        let base = idx(r, g, b);
        assert!((lut.table[base] - 1.0).abs() < 1e-6);
        assert!(lut.table[base + 1].abs() < 1e-6);
        assert!(lut.table[base + 2].abs() < 1e-6);
    }
}
