//! `montage upgrade` placeholder.

use anyhow::{Result, bail};

/// CLI args. Wired into clap in main.rs.
pub struct UpgradeArgs {
    /// Reserved for the future restored release path.
    pub from: Option<String>,
    /// Reserved for the future restored release path.
    pub check: bool,
    /// Reserved for the future restored release path.
    pub skip_uv_sync: bool,
}

pub fn run(args: UpgradeArgs) -> Result<()> {
    let UpgradeArgs {
        from,
        check,
        skip_uv_sync,
    } = args;
    let _ = (from, check, skip_uv_sync);
    bail!(
        "release upgrades are not available in this source checkout; \
         build from source with Cargo until release packaging is restored"
    )
}
