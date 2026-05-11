//! `use_broll` tool — download a chosen Pexels video to the project
//! and return a ready-to-paste `*** Insert BRoll` EDL fragment for the
//! agent to wrap in `apply_edl`.
//!
//! Two-step flow with `search_broll`:
//!
//! 1. `search_broll(query)` — agent or user picks a `pexels_id`.
//! 2. `use_broll(pexels_id, anchor, duration_s, position?, insert_as?)` — this
//!    tool downloads the video to `<project>/raw/broll/pexels-<id>.mp4`
//!    (idempotent on file existence; we don't refetch a clip we already
//!    have) and returns a structured response containing the EDL
//!    fragment.
//!
//! The agent then constructs an `apply_edl` call from the returned
//! fragment. We deliberately don't apply the EDL ourselves — keeping
//! the apply path single-source-of-truth lets the agent decide
//! whether to bundle the b-roll insertion with other ops (e.g. a
//! continuity-fix in Phase 3.7).
//!
//! Per-session safety: caps at [`MAX_DOWNLOADS_PER_SESSION`] downloads
//! (in-process counter; resets on restart). At ~30MB per 1080p clip,
//! ten downloads is a 300MB project bloat ceiling per session — enough
//! for a tight episode, low enough that the agent can't accidentally
//! pull a hundred clips in a runaway loop.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::pexels;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Safety cap on Pexels downloads in one process lifetime. Resets on
/// restart — this is friction, not policy.
pub const MAX_DOWNLOADS_PER_SESSION: usize = 10;

/// Process-wide counter. Tools are stateless per the trait; we keep
/// the budget here. A second `awidat` process would have its own
/// budget — that's fine, the cap is a runaway-loop guard not a
/// long-lived quota.
static DOWNLOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Reset the download counter. Test-only — production code never
/// touches this.
#[cfg(test)]
fn reset_download_count() {
    DOWNLOAD_COUNT.store(0, Ordering::SeqCst);
}

/// Max video width to download. 1920 is plenty for a typical 1080p
/// timeline; downloading 4K renditions wastes bandwidth.
const DEFAULT_MAX_WIDTH: u32 = 1920;

/// The `use_broll` tool.
pub struct UseBrollTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Pexels video id (from a prior `search_broll` result).
    pexels_id: u64,
    /// Where on the timeline the cutaway should land. The agent
    /// either passes `transcript_snippet` or `clip_uuid` — same
    /// shape as every other anchor in the EDL grammar.
    anchor: AnchorArg,
    /// Cutaway length in seconds. The downloaded clip is usually
    /// longer than this; the EDL trims it.
    duration_s: f64,
    /// `overlay` (default — sits on V2 over the existing clip) or
    /// `replace` (cuts the underlying clip out for `duration_s`).
    #[serde(default)]
    position: Option<String>,
    /// `broll` (default) returns Insert BRoll; `pip` returns Insert PiP.
    #[serde(default)]
    insert_as: Option<String>,
    /// Override max-width for the downloaded rendition. Default
    /// 1920px wide; raise only if the project renders 4K.
    #[serde(default)]
    max_width: Option<u32>,
}

/// Anchor shape passed in from the agent. Matches the JSON form the
/// EDL parser accepts as `@@ anchor: …`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum AnchorArg {
    /// `{"transcript_snippet": "the city skyline reminded me"}`
    Transcript { transcript_snippet: String },
    /// `{"clip_uuid": "clip-3"}`
    Uuid { clip_uuid: String },
}

#[async_trait]
impl ToolHandler for UseBrollTool {
    fn name(&self) -> &'static str {
        "use_broll"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "use_broll".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["pexels_id", "anchor", "duration_s"],
                "properties": {
                    "pexels_id": {
                        "type": "integer",
                        "description": "Pexels video id from a prior search_broll result."
                    },
                    "anchor": {
                        "oneOf": [
                            {
                                "type": "object",
                                "required": ["transcript_snippet"],
                                "properties": {
                                    "transcript_snippet": { "type": "string" }
                                }
                            },
                            {
                                "type": "object",
                                "required": ["clip_uuid"],
                                "properties": {
                                    "clip_uuid": { "type": "string" }
                                }
                            }
                        ],
                        "description": "Where the cutaway lands. Same anchor shape as the EDL grammar."
                    },
                    "duration_s": {
                        "type": "number",
                        "minimum": 0.5,
                        "maximum": 30.0,
                        "description": "Cutaway length in seconds (0.5–30). Stock cutaways read best at 2–4s."
                    },
                    "position": {
                        "type": "string",
                        "enum": ["overlay", "replace"],
                        "description": "Default overlay (V2). Use replace to cut the underlying clip for duration_s."
                    },
                    "insert_as": {
                        "type": "string",
                        "enum": ["broll", "pip"],
                        "description": "Default broll returns Insert BRoll. pip returns an Insert PiP fragment with default layout."
                    },
                    "max_width": {
                        "type": "integer",
                        "minimum": 320,
                        "maximum": 3840,
                        "description": "Cap on downloaded rendition width. Default 1920."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Writes a file under <project>/raw/broll. Treat as mutating
        // so the loop serializes it with apply_edl.
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let pexels_id = invocation
            .args
            .get("pexels_id")
            .and_then(|v| v.as_u64())
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<missing-pexels-id>".to_string());
        let max_width = invocation
            .args
            .get("max_width")
            .and_then(|v| v.as_u64())
            .map(|w| w.to_string())
            .unwrap_or_else(|| "default-width".to_string());
        vec![ApprovalKey::new(
            "use_broll",
            format!("pexels:{pexels_id}:{max_width}"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "use_broll: invalid args ({e}). Required: pexels_id, anchor, duration_s."
            ))
        })?;

        let position = match args.position.as_deref().unwrap_or("overlay") {
            "overlay" => "overlay",
            "replace" => "replace",
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "use_broll: invalid position '{other}'. Use 'overlay' or 'replace'."
                )));
            }
        };
        let insert_as = match args.insert_as.as_deref().unwrap_or("broll") {
            "broll" => "broll",
            "pip" => "pip",
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "use_broll: invalid insert_as '{other}'. Use 'broll' or 'pip'."
                )));
            }
        };
        let max_width = args.max_width.unwrap_or(DEFAULT_MAX_WIDTH);
        if !(0.5..=30.0).contains(&args.duration_s) {
            return Err(FunctionCallError::RespondToModel(format!(
                "use_broll: duration_s={} out of range. Use 0.5–30.0.",
                args.duration_s
            )));
        }

        // Cap downloads per session.
        let prior = DOWNLOAD_COUNT.load(Ordering::SeqCst);
        if prior >= MAX_DOWNLOADS_PER_SESSION {
            return Err(FunctionCallError::RespondToModel(format!(
                "use_broll: per-session download budget reached ({MAX_DOWNLOADS_PER_SESSION}). \
                 Restart the session if this is genuinely needed."
            )));
        }

        let asset_rel = format!("raw/broll/pexels-{}.mp4", args.pexels_id);
        let dest = ctx.project_root.join(&asset_rel);
        let downloaded = !dest.exists();
        if downloaded {
            // Download path: search the video first (we don't carry
            // the prior search response, by design — this tool is
            // self-contained per call). One search call per
            // distinct download, which is fine on the rate budget.
            let client = pexels::Client::from_env_or_keychain(pexels::ClientConfig::default())
                .map_err(map_pexels_err)?;
            let video = fetch_video_by_id(&client, args.pexels_id).await?;
            let file = video.pick_mp4(max_width).ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "use_broll: Pexels video {} has no mp4 renditions; pick another result.",
                    args.pexels_id
                ))
            })?;
            client
                .download_to(&file.link, &dest)
                .await
                .map_err(map_pexels_err)?;
            DOWNLOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        }

        let edl_fragment = build_edl_fragment(
            &asset_rel,
            &args.anchor,
            args.duration_s,
            position,
            insert_as,
        );

        let body = serde_json::json!({
            "asset_path": asset_rel,
            "absolute_path": dest.display().to_string(),
            "downloaded": downloaded,
            "edl_fragment": edl_fragment,
            "downloads_remaining_this_session":
                MAX_DOWNLOADS_PER_SESSION.saturating_sub(DOWNLOAD_COUNT.load(Ordering::SeqCst)),
            "next_step": "Hand the edl_fragment to apply_edl to actually place the cutaway.",
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

async fn fetch_video_by_id(
    client: &pexels::Client,
    id: u64,
) -> Result<pexels::Video, FunctionCallError> {
    // The Pexels API exposes /videos/videos/<id> for a direct lookup;
    // we use the same client surface but it's a thin GET. To keep
    // PexelsClient's surface small we re-do the search via the id
    // fallback by querying for `id:<id>` — but that's not actually
    // a Pexels query operator. So: hit /videos/videos/<id> via raw
    // reqwest by reusing the same client config. To avoid expanding
    // the public Pexels client surface in 3.3, keep the fetch helper
    // local: call the endpoint here.
    //
    // Justification for not adding it to `pexels::Client`: every
    // other Pexels caller uses search; only `use_broll` needs by-id.
    // Until we have a second by-id caller, the helper lives here.
    let url = format!("{}/videos/videos/{id}", pexels::DEFAULT_BASE_URL);
    let key = awidat_secrets::get(
        awidat_secrets::env_vars::PEXELS_API_KEY,
        awidat_secrets::accounts::PEXELS_API_KEY,
    )
    .map_err(|e| {
        FunctionCallError::RespondToModel(format!("use_broll: keychain access failed: {e}"))
    })?
    .ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "use_broll: PEXELS_API_KEY not set in env or keychain.".into(),
        )
    })?;

    // Use the embedded reqwest by way of constructing a fresh client.
    // We can't reach into `pexels::Client`'s private http member, but
    // we can build our own one-off here — same dependency, no shared
    // mutable state.
    let _ = client; // suppress unused; kept in signature for future caching
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!("use_broll: HTTP client build failed: {e}"))
        })?;
    let resp = http
        .get(&url)
        .header("Authorization", &key)
        .send()
        .await
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!("use_broll: Pexels GET failed: {e}"))
        })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FunctionCallError::RespondToModel(format!(
            "use_broll: Pexels returned {} for video {id}: {}",
            status.as_u16(),
            truncate(&body, 256)
        )));
    }
    resp.json::<pexels::Video>().await.map_err(|e| {
        FunctionCallError::RespondToModel(format!("use_broll: malformed Pexels response: {e}"))
    })
}

fn build_edl_fragment(
    asset_rel: &str,
    anchor: &AnchorArg,
    duration_s: f64,
    position: &str,
    insert_as: &str,
) -> String {
    let anchor_line = match anchor {
        AnchorArg::Transcript { transcript_snippet } => {
            // Escape embedded quotes minimally — same convention
            // the parser accepts.
            let escaped = transcript_snippet.replace('"', "\\\"");
            format!("@@ anchor: transcript_snippet=\"{escaped}\"")
        }
        AnchorArg::Uuid { clip_uuid } => {
            format!("@@ anchor: clip_uuid={clip_uuid}")
        }
    };
    if insert_as == "pip" {
        format!(
            "*** Begin EDL\n\
             *** Insert PiP\n\
             {anchor_line}\n\
             + asset: {asset_rel}\n\
             + duration_s: {duration_s}\n\
             + source_start_s: 0\n\
             + corner: bottom_right\n\
             + scale: 0.28\n\
             + margin_pct: 0.035\n\
             *** End EDL\n"
        )
    } else {
        format!(
            "*** Begin EDL\n\
             *** Insert BRoll\n\
             {anchor_line}\n\
             + asset: {asset_rel}\n\
             + duration_s: {duration_s}\n\
             + position: {position}\n\
             *** End EDL\n"
        )
    }
}

fn map_pexels_err(err: pexels::PexelsError) -> FunctionCallError {
    match err {
        pexels::PexelsError::MissingApiKey => FunctionCallError::RespondToModel(
            "use_broll: PEXELS_API_KEY not set. Set the env var or store via OS keychain \
             (service 'awidat', account 'pexels_api_key')."
                .into(),
        ),
        pexels::PexelsError::Api { status: 404, .. } => FunctionCallError::RespondToModel(
            "use_broll: Pexels video id not found. Re-run search_broll for fresh ids.".into(),
        ),
        pexels::PexelsError::Api { status: 429, .. } => FunctionCallError::RespondToModel(
            "use_broll: Pexels rate-limited. Try again later.".into(),
        ),
        other => FunctionCallError::RespondToModel(format!("use_broll: Pexels failed: {other}")),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

/// Project-relative b-roll directory. Exposed so the post-import
/// chain or other tools can discover Pexels-fetched assets.
pub fn broll_dir(project_root: &Path) -> PathBuf {
    project_root.join("raw").join("broll")
}

const DESCRIPTION: &str = "\
Download a Pexels video chosen from a prior `search_broll` result \
and return an EDL fragment ready to hand to `apply_edl`.\
\n\nDoes NOT apply the EDL itself — that's `apply_edl`'s job. The \
returned `edl_fragment` is a `*** Begin EDL ... *** End EDL` block \
you can either submit verbatim or merge with other ops (e.g. \
bundling a b-roll cutaway with a cross-dissolve cut).\
\n\nThe download lands at `raw/broll/pexels-<id>.mp4`. Idempotent: \
if the file already exists the tool returns the EDL fragment without \
re-downloading.\
\n\nPer-session cap: 10 downloads. Excess returns a clear error \
without quietly succeeding-with-partial-state.\
\n\nDefaults: max_width=1920 (skip 4K renditions), \
position=overlay (V2 cutaway over the existing clip). Pass \
position=replace to cut the underlying clip for the duration of \
the b-roll. Pass insert_as=pip to return an Insert PiP fragment \
instead, using the default bottom-right PiP layout.\
\n\nReturns: { asset_path, absolute_path, downloaded, edl_fragment, \
downloads_remaining_this_session, next_step }.\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_fragment_with_transcript_anchor() {
        let anchor = AnchorArg::Transcript {
            transcript_snippet: "the city skyline reminded me".into(),
        };
        let frag = build_edl_fragment(
            "raw/broll/pexels-1234.mp4",
            &anchor,
            2.4,
            "overlay",
            "broll",
        );
        assert!(frag.contains("Insert BRoll"));
        assert!(frag.contains("@@ anchor: transcript_snippet=\"the city skyline reminded me\""));
        assert!(frag.contains("+ asset: raw/broll/pexels-1234.mp4"));
        assert!(frag.contains("+ duration_s: 2.4"));
        assert!(frag.contains("+ position: overlay"));
    }

    #[test]
    fn build_fragment_with_uuid_anchor() {
        let anchor = AnchorArg::Uuid {
            clip_uuid: "clip-3".into(),
        };
        let frag = build_edl_fragment("raw/broll/pexels-9.mp4", &anchor, 3.0, "replace", "broll");
        assert!(frag.contains("@@ anchor: clip_uuid=clip-3"));
        assert!(frag.contains("+ position: replace"));
    }

    #[test]
    fn fragment_escapes_embedded_quotes() {
        let anchor = AnchorArg::Transcript {
            transcript_snippet: r#"he said "yes" loudly"#.into(),
        };
        let frag = build_edl_fragment("raw/broll/pexels-1.mp4", &anchor, 2.0, "overlay", "broll");
        // The parser accepts \" inside the quoted string — we must
        // escape, otherwise the fragment is unparseable.
        assert!(
            frag.contains(r#"transcript_snippet="he said \"yes\" loudly""#),
            "{frag}"
        );
    }

    #[test]
    fn truncate_handles_unicode_boundary() {
        let s = "héllo wörld";
        let out = truncate(s, 4);
        assert!(out.ends_with('…'));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn download_count_cap_constant_is_sane() {
        // If anyone bumps this to a wild number, the test catches it.
        // 10 is the plan; treat 20 as a soft ceiling that should
        // require a code-review conversation.
        assert!(MAX_DOWNLOADS_PER_SESSION <= 20, "raise with care");
        // And reset works for the test-only helper.
        DOWNLOAD_COUNT.store(7, Ordering::SeqCst);
        reset_download_count();
        assert_eq!(DOWNLOAD_COUNT.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn broll_dir_is_under_raw() {
        let dir = broll_dir(Path::new("/tmp/proj"));
        assert!(dir.ends_with("raw/broll"));
    }
}
