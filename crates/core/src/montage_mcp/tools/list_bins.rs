//! `list_bins` — enumerate available asset bins for the project.
//! Ported in step 5 from `crates/core/src/tools/list_bins.rs`.
//!
//! Returns the union of:
//!   - User-defined bins from `AssetCatalog::bins` (kind=user).
//!   - Built-in role buckets keyed by [`AssetRole`] (kind=role), with
//!     synthetic ids of the form `role:<snake_case>`. These mirror
//!     Kdenlive's "Audio Clips" / "Video Clips" sidebar buckets so the
//!     agent can filter `list_assets` on role without the user manually
//!     creating bins.

use montage_proto::professional::{AssetRecord, AssetRole};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Synthetic-bin id prefix for built-in role buckets.
pub const ROLE_BIN_PREFIX: &str = "role:";

/// All built-in role buckets, in stable display order.
const ROLE_BUCKETS: &[(AssetRole, &str)] = &[
    (AssetRole::Video, "Video clips"),
    (AssetRole::Audio, "Audio clips"),
    (AssetRole::Still, "Stills"),
    (AssetRole::Graphic, "Graphics"),
    (AssetRole::Caption, "Captions"),
    (AssetRole::Support, "Support media"),
];

/// `list_bins` takes no arguments.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ListBinsArgs {}

pub fn run(_args: ListBinsArgs, ctx: McpToolCtx) -> Result<String, String> {
    // Read the project to get the user-defined bins. If the project
    // doesn't exist yet (e.g. fresh init), gracefully degrade to just
    // the role buckets.
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("list_bins: unable to read project: {e}"))?;

    let mut lines: Vec<String> = Vec::new();
    let mut count = 0usize;

    // Role buckets first. They're always present so the agent always
    // has a stable surface to filter against.
    for (role, label) in ROLE_BUCKETS {
        count += 1;
        lines.push(format!(
            "{idx:>3}. kind=role id={id} name=\"{name}\"",
            idx = count,
            id = role_bin_id(*role),
            name = label,
        ));
    }

    // User-defined bins, if any.
    if let Some(catalog) = project
        .timeline
        .metadata
        .montage
        .as_ref()
        .and_then(|meta| meta.asset_catalog.as_ref())
    {
        for bin in &catalog.bins {
            count += 1;
            let parent = bin
                .parent_id
                .as_deref()
                .map(|p| format!(" parent={p}"))
                .unwrap_or_default();
            lines.push(format!(
                "{idx:>3}. kind=user id={id} name=\"{name}\"{parent}",
                idx = count,
                id = bin.id,
                name = bin.name,
            ));
        }
    }

    let mut out = format!("total={count}\n");
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// Stable synthetic bin id for an [`AssetRole`].
pub fn role_bin_id(role: AssetRole) -> String {
    let suffix = match role {
        AssetRole::Video => "video",
        AssetRole::Audio => "audio",
        AssetRole::Still => "still",
        AssetRole::Graphic => "graphic",
        AssetRole::Caption => "caption",
        AssetRole::Support => "support",
    };
    format!("{ROLE_BIN_PREFIX}{suffix}")
}

/// Resolve the role bin id for an [`AssetRecord`].
pub fn role_id_for_record(record: &AssetRecord) -> String {
    role_bin_id(record.role)
}

pub const DESCRIPTION: &str = "\
List the available asset bins in this project. Returns built-in role \
buckets (kind=role, ids like 'role:video', 'role:audio') plus any \
user/agent-defined bins (kind=user). Pass any returned id as the \
`bin` argument to `list_assets` to filter that surface.";
