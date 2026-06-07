//! `clip_search` tool — find frames matching a free-text query.
//!
//! Reads the CLIP sidecar (per-second 512-dim image embeddings,
//! float16/base64) and asks the live `clip-mcp` server to encode the
//! query text via the `query_text` tool. Cosine similarity (which is
//! a dot product since both sides are L2-normalized) ranks every
//! frame. Top-K returned as `[t_s, score]`.
//!
//! Performance: the first call in a session pays clip-mcp's ~6s torch
//! import + model load (charged to whichever vision tool happens to
//! land first). Subsequent calls reuse the warm process and run in
//! ~80ms (encode_text smoke result).

use async_trait::async_trait;
use base64::Engine as _;
use montage_index::walk_indexer;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// The `clip_search` tool.
pub struct ClipSearchTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Free-text description of what to find. Examples: "person
    /// holding a phone", "dark room", "outdoor establishing shot".
    query: String,
    /// Restrict to one asset id; otherwise rank across all assets.
    #[serde(default)]
    asset_id: Option<String>,
    /// Top-K to return per asset. Default 5, hard cap 25.
    #[serde(default)]
    limit: Option<usize>,
    /// Minimum cosine score floor. CLIP image-text alignment scores
    /// for ViT-B-32 typically sit in [0.15, 0.30] for relevant matches;
    /// 0.18 is a sane "matches the query at all" threshold. Default 0.0
    /// (return whatever's most similar even if weak).
    #[serde(default)]
    min_score: Option<f32>,
}

#[async_trait]
impl ToolHandler for ClipSearchTool {
    fn name(&self) -> &'static str {
        "clip_search"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "clip_search".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Free-text description. CLIP retrieves frames whose contents match the description."
                    },
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's CLIP sidecar."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 25,
                        "description": "Top-K per asset. Default 5."
                    },
                    "min_score": {
                        "type": "number",
                        "description": "Cosine-similarity floor. CLIP scores for relevant matches are typically 0.18+."
                    }
                },
                "required": ["query"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "clip_search: invalid args ({e}). 'query' is required."
            ))
        })?;
        let limit = args.limit.unwrap_or(5).min(25);
        let min_score = args.min_score.unwrap_or(0.0);

        // Encode the query via clip-mcp's query_text tool. This is the
        // McpHost-mediated call; first hit warms the python process.
        if !ctx.mcp_host.has_server("clip").await {
            return Err(FunctionCallError::RespondToModel(
                "clip_search: no MCP server named 'clip' is registered. \
                 Add a [[mcp.servers]] entry pointing at clip-mcp in your \
                 project's .montage/config.toml or your global montage config. \
                 See python/SMOKE.md for an example block."
                    .into(),
            ));
        }
        let response = ctx
            .mcp_host
            .call_tool(
                "clip",
                "query_text",
                serde_json::json!({"query": args.query}),
            )
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "clip_search: query_text call failed: {e}"
                ))
            })?;

        // The MCP server returns either a structured-content dict or a
        // text content block. clip-mcp returns structured; fall back
        // to parsing the text variant for safety.
        let body: serde_json::Value = match response {
            serde_json::Value::String(s) => serde_json::from_str(&s).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "clip_search: query_text returned unparsable text: {e}"
                ))
            })?,
            v => v,
        };

        let q_b64 = body
            .get("embedding_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "clip_search: query_text response missing 'embedding_b64'".into(),
                )
            })?;
        let q_dim = body
            .get("embedding_dim")
            .and_then(|v| v.as_u64())
            .unwrap_or(512) as usize;
        let q_vec = decode_b64_f16(q_b64, q_dim)?;

        // Walk every CLIP sidecar, compute scores, collect top-K per
        // asset. We deliberately don't cross-asset rank — the agent
        // usually wants per-asset results so they can pick which asset
        // to draw from.
        let walker = walk_indexer(&ctx.project_root, "clip").map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "clip_search: clip sidecars not readable ({e}). \
                 Run `montage index --indexer clip <project>` and retry."
            ))
        })?;

        let mut all_results: Vec<serde_json::Value> = Vec::new();
        for (asset_id, sidecar) in walker {
            if let Some(filter) = &args.asset_id
                && filter != &asset_id
            {
                continue;
            }
            let Some(data) = sidecar.get("data") else {
                continue;
            };
            let timestamps = data
                .get("timestamps_s")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let frame_count = data
                .get("frame_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let dim = data
                .get("embedding_dim")
                .and_then(|v| v.as_u64())
                .unwrap_or(512) as usize;
            if dim != q_dim {
                tracing::warn!(
                    asset = %asset_id,
                    sidecar_dim = dim,
                    query_dim = q_dim,
                    "clip_search: dim mismatch; skipping asset"
                );
                continue;
            }
            let Some(b64) = data.get("embeddings_b64").and_then(|v| v.as_str()) else {
                continue;
            };
            let frames = decode_b64_f16_matrix(b64, frame_count, dim)?;

            // Cosine = dot product (both L2-normalized).
            let mut scored: Vec<(usize, f32)> = (0..frame_count)
                .map(|i| {
                    let row = &frames[i * dim..(i + 1) * dim];
                    let s = row
                        .iter()
                        .zip(q_vec.iter())
                        .map(|(a, b)| a * b)
                        .sum::<f32>();
                    (i, s)
                })
                .filter(|(_, s)| *s >= min_score)
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(limit);

            for (i, score) in scored {
                let t_s = timestamps
                    .get(i)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(i as f64);
                all_results.push(serde_json::json!({
                    "asset_id": asset_id,
                    "t_s": t_s,
                    "score": score,
                }));
            }
        }

        // Sort the global list by score so the caller sees the
        // strongest matches up top regardless of asset.
        all_results.sort_by(|a, b| {
            let sa = a.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let sb = b.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(ToolOutput::text(
            serde_json::json!({
                "results": all_results,
                "query": args.query,
                "model": body.get("model").cloned().unwrap_or(serde_json::Value::Null),
            })
            .to_string(),
        ))
    }
}

/// Decode `dim` little-endian float16 values from a base64 string into
/// a Vec<f32>. Used both for query and per-row matrix decode.
fn decode_b64_f16(b64: &str, dim: usize) -> Result<Vec<f32>, FunctionCallError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| FunctionCallError::RespondToModel(format!("clip_search: bad base64: {e}")))?;
    if bytes.len() != dim * 2 {
        return Err(FunctionCallError::RespondToModel(format!(
            "clip_search: query embedding length mismatch (got {} bytes, expected {} for dim={})",
            bytes.len(),
            dim * 2,
            dim
        )));
    }
    let mut out = Vec::with_capacity(dim);
    for i in 0..dim {
        let pair = [bytes[i * 2], bytes[i * 2 + 1]];
        out.push(f16_to_f32(u16::from_le_bytes(pair)));
    }
    Ok(out)
}

/// Decode an `n_rows × dim` float16 matrix into a flat row-major Vec<f32>.
fn decode_b64_f16_matrix(
    b64: &str,
    n_rows: usize,
    dim: usize,
) -> Result<Vec<f32>, FunctionCallError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!("clip_search: bad sidecar base64: {e}"))
        })?;
    let want = n_rows * dim * 2;
    if bytes.len() != want {
        return Err(FunctionCallError::RespondToModel(format!(
            "clip_search: sidecar embeddings byte length mismatch ({} vs {})",
            bytes.len(),
            want
        )));
    }
    let mut out = Vec::with_capacity(n_rows * dim);
    for i in 0..(n_rows * dim) {
        let pair = [bytes[i * 2], bytes[i * 2 + 1]];
        out.push(f16_to_f32(u16::from_le_bytes(pair)));
    }
    Ok(out)
}

/// IEEE-754 half-precision → single-precision conversion.
///
/// We only need this in the read direction (clip-mcp produces float16,
/// we consume it). Pulling a float16 crate just for one direction is
/// overkill; the conversion is 12 lines.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x3ff) as u32;

    let f32_bits = if exp == 0 {
        if mant == 0 {
            // ±0.
            (sign as u32) << 31
        } else {
            // Subnormal — normalize.
            let mut m = mant;
            let mut e = 1i32;
            while m & 0x400 == 0 {
                m <<= 1;
                e += 1;
            }
            let exp32 = (127 - 15 - e + 1) as u32;
            ((sign as u32) << 31) | (exp32 << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 0x1f {
        // Inf or NaN.
        ((sign as u32) << 31) | (0xff << 23) | (mant << 13)
    } else {
        let exp32 = (exp - 15 + 127) as u32;
        ((sign as u32) << 31) | (exp32 << 23) | (mant << 13)
    };
    f32::from_bits(f32_bits)
}

const DESCRIPTION: &str = "\
Find frames matching a free-text query via CLIP image-text similarity. \
Reads the `clip` indexer's per-second sidecar embeddings and uses \
clip-mcp's `query_text` tool (long-lived, model warm) to encode the \
query, then ranks frames by cosine similarity.\
\n\n\
Examples:\
\n  clip_search(query='person holding a phone')\
\n  clip_search(query='dark room',  min_score=0.20)\
\n  clip_search(query='outdoor establishing shot', limit=10)\
\n\n\
Score scale: ViT-B-32 cosine scores for relevant matches usually sit \
in [0.18, 0.30]. Below 0.15 the model is grasping at straws. Use \
min_score=0.18 if you want only confident matches.\
\n\n\
Requires: `clip` indexer sidecar + clip-mcp registered as an MCP \
server. First call per session pays the model warm-up (~6s); after \
that, ~80ms per query.\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_to_f32_basics() {
        // 1.0 = 0x3c00
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        // -1.0 = 0xbc00
        assert_eq!(f16_to_f32(0xbc00), -1.0);
        // 0.0
        assert_eq!(f16_to_f32(0x0000), 0.0);
        // -0.0
        assert_eq!(f16_to_f32(0x8000).to_bits(), (-0.0_f32).to_bits());
        // 0.5 = 0x3800
        assert_eq!(f16_to_f32(0x3800), 0.5);
        // 2.0 = 0x4000
        assert_eq!(f16_to_f32(0x4000), 2.0);
    }

    #[test]
    fn decode_b64_f16_round_trips_a_unit_vector() {
        // Build a 4-dim vector [1,0,0,0] in float16 little-endian
        // bytes, base64 it, then decode back. Asserts the unpack
        // matches the python encoder's bit layout.
        let bytes: Vec<u8> = [0x3c00u16, 0u16, 0u16, 0u16]
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let v = decode_b64_f16(&b64, 4).unwrap();
        assert_eq!(v, vec![1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn decode_b64_f16_rejects_wrong_length() {
        // 4 bytes for a claimed dim=4 (need 8).
        let b64 = base64::engine::general_purpose::STANDARD.encode([0u8; 4]);
        let err = decode_b64_f16(&b64, 4).unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("length mismatch")),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }
}
