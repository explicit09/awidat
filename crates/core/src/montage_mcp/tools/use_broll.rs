//! `use_broll` — download a chosen Pexels video to the project and
//! return a ready-to-paste `*** Insert BRoll` EDL fragment for the
//! agent to wrap in `apply_edl`. Ported from
//! `crates/core/src/tools/use_broll.rs` to the in-process MCP server.
//!
//! Mutating: writes a downloaded file under `raw/broll/`. The original
//! `ToolHandler` had `is_mutating = true` for the same reason and
//! routed an `ApprovalKey` through `ToolContext.approval_tx`. Both
//! behaviors are intentionally dropped in the port: codex performs the
//! destructive-hint approval before dispatching the call.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::pexels;

/// Safety cap on Pexels downloads in one process lifetime. Resets on
/// restart — this is friction, not policy.
pub const MAX_DOWNLOADS_PER_SESSION: usize = 10;

/// Process-wide counter. Tools are stateless per the trait; we keep
/// the budget here. A second `montage` process would have its own
/// budget — that's fine, the cap is a runaway-loop guard not a
/// long-lived quota.
static DOWNLOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Max video width to download. 1920 is plenty for a typical 1080p
/// timeline; downloading 4K renditions wastes bandwidth.
const DEFAULT_MAX_WIDTH: u32 = 1920;

/// Arguments to `use_broll`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct UseBrollArgs {
    /// Pexels video id (from a prior `search_broll` result).
    pub pexels_id: u64,
    /// Where on the timeline the cutaway should land. The agent
    /// either passes `transcript_snippet` or `clip_uuid` — same
    /// shape as every other anchor in the EDL grammar.
    pub anchor: AnchorArg,
    /// Cutaway length in seconds. The downloaded clip is usually
    /// longer than this; the EDL trims it.
    pub duration_s: f64,
    /// `overlay` (default — sits on V2 over the existing clip) or
    /// `replace` (cuts the underlying clip out for `duration_s`).
    #[serde(default)]
    pub position: Option<String>,
    /// `broll` (default) returns Insert BRoll; `pip` returns Insert PiP.
    #[serde(default)]
    pub insert_as: Option<String>,
    /// Override max-width for the downloaded rendition. Default
    /// 1920px wide; raise only if the project renders 4K.
    #[serde(default)]
    pub max_width: Option<u32>,
}

/// Anchor shape passed in from the agent. Matches the JSON form the
/// EDL parser accepts as `@@ anchor: …`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AnchorArg {
    /// `{"transcript_snippet": "the city skyline reminded me"}`
    Transcript { transcript_snippet: String },
    /// `{"clip_uuid": "clip-3"}`
    Uuid { clip_uuid: String },
}

impl Default for AnchorArg {
    fn default() -> Self {
        AnchorArg::Uuid {
            clip_uuid: String::new(),
        }
    }
}

/// Run `use_broll` against the project resolved from [`McpToolCtx`].
/// Returns a JSON status body as `Ok(String)`; validation / download
/// failures return `Err(String)`.
pub async fn run(args: UseBrollArgs, ctx: McpToolCtx) -> Result<String, String> {
    let position = match args.position.as_deref().unwrap_or("overlay") {
        "overlay" => "overlay",
        "replace" => "replace",
        other => {
            return Err(format!(
                "use_broll: invalid position '{other}'. Use 'overlay' or 'replace'."
            ));
        }
    };
    let insert_as = match args.insert_as.as_deref().unwrap_or("broll") {
        "broll" => "broll",
        "pip" => "pip",
        other => {
            return Err(format!(
                "use_broll: invalid insert_as '{other}'. Use 'broll' or 'pip'."
            ));
        }
    };
    let max_width = args.max_width.unwrap_or(DEFAULT_MAX_WIDTH);
    if !(0.5..=30.0).contains(&args.duration_s) {
        return Err(format!(
            "use_broll: duration_s={} out of range. Use 0.5–30.0.",
            args.duration_s
        ));
    }

    // Cap downloads per session.
    let prior = DOWNLOAD_COUNT.load(Ordering::SeqCst);
    if prior >= MAX_DOWNLOADS_PER_SESSION {
        return Err(format!(
            "use_broll: per-session download budget reached ({MAX_DOWNLOADS_PER_SESSION}). \
             Restart the session if this is genuinely needed."
        ));
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
            format!(
                "use_broll: Pexels video {} has no mp4 renditions; pick another result.",
                args.pexels_id
            )
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
    Ok(body.to_string())
}

async fn fetch_video_by_id(client: &pexels::Client, id: u64) -> Result<pexels::Video, String> {
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
    let key = montage_secrets::get(
        montage_secrets::env_vars::PEXELS_API_KEY,
        montage_secrets::accounts::PEXELS_API_KEY,
    )
    .map_err(|e| format!("use_broll: keychain access failed: {e}"))?
    .ok_or_else(|| "use_broll: PEXELS_API_KEY not set in env or keychain.".to_string())?;

    // Use the embedded reqwest by way of constructing a fresh client.
    // We can't reach into `pexels::Client`'s private http member, but
    // we can build our own one-off here — same dependency, no shared
    // mutable state.
    let _ = client; // suppress unused; kept in signature for future caching
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("use_broll: HTTP client build failed: {e}"))?;
    let resp = http
        .get(&url)
        .header("Authorization", &key)
        .send()
        .await
        .map_err(|e| format!("use_broll: Pexels GET failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "use_broll: Pexels returned {} for video {id}: {}",
            status.as_u16(),
            truncate(&body, 256)
        ));
    }
    resp.json::<pexels::Video>()
        .await
        .map_err(|e| format!("use_broll: malformed Pexels response: {e}"))
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

fn map_pexels_err(err: pexels::PexelsError) -> String {
    match err {
        pexels::PexelsError::MissingApiKey => {
            "use_broll: PEXELS_API_KEY not set. Set the env var or store via OS keychain \
             (service 'montage', account 'pexels_api_key')."
                .to_string()
        }
        pexels::PexelsError::Api { status: 404, .. } => {
            "use_broll: Pexels video id not found. Re-run search_broll for fresh ids.".to_string()
        }
        pexels::PexelsError::Api { status: 429, .. } => {
            "use_broll: Pexels rate-limited. Try again later.".to_string()
        }
        other => format!("use_broll: Pexels failed: {other}"),
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

pub const DESCRIPTION: &str = "\
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
