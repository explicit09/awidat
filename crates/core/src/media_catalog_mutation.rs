//! Shared mutation helpers for durable media-management metadata.

use std::collections::BTreeSet;

use awidat_proto::AWIDAT_PROJECT_VERSION;
use awidat_proto::awidat_meta::AwidatTimelineMetadata;
use awidat_proto::otio::Timeline;
use awidat_proto::professional::{AssetBin, AssetCatalog, AssetRecord};
use thiserror::Error;

/// Catalog mutation failure.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CatalogMutationError {
    /// A bin already exists with this id.
    #[error("bin {0} already exists")]
    DuplicateBin(String),
    /// The requested parent bin does not exist.
    #[error("parent bin {0} does not exist")]
    MissingParentBin(String),
    /// The requested bin does not exist.
    #[error("bin {0} does not exist")]
    MissingBin(String),
    /// The requested asset does not exist.
    #[error("asset {0} does not exist")]
    MissingAsset(String),
}

/// Ensure the timeline has `metadata.awidat` and return it.
pub fn ensure_awidat_metadata(timeline: &mut Timeline) -> &mut AwidatTimelineMetadata {
    timeline
        .metadata
        .awidat
        .get_or_insert_with(|| AwidatTimelineMetadata {
            version: AWIDAT_PROJECT_VERSION.to_string(),
            ..AwidatTimelineMetadata::default()
        })
}

/// Ensure the metadata has a durable asset catalog and return it.
pub fn ensure_asset_catalog(meta: &mut AwidatTimelineMetadata) -> &mut AssetCatalog {
    meta.asset_catalog.get_or_insert_with(AssetCatalog::default)
}

/// Upsert an asset into the durable catalog and source asset list.
pub fn upsert_asset(meta: &mut AwidatTimelineMetadata, record: AssetRecord) {
    if !meta.source_assets.iter().any(|asset| asset == &record.path) {
        meta.source_assets.push(record.path.clone());
        meta.source_assets.sort();
    }

    let catalog = ensure_asset_catalog(meta);
    match catalog
        .assets
        .iter_mut()
        .find(|asset| asset.id == record.id)
    {
        Some(existing) => {
            let mut tags = existing
                .tags
                .iter()
                .chain(record.tags.iter())
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            std::mem::swap(&mut existing.tags, &mut tags);
            existing.path = record.path;
            existing.role = record.role;
            existing.bin_id = record.bin_id;
            existing.label = record.label.or_else(|| existing.label.take());
            existing.rating = record.rating.or(existing.rating);
            existing.readiness = record.readiness;
            existing.provenance = record.provenance.or_else(|| existing.provenance.take());
            existing.usage = record.usage;
        }
        None => catalog.assets.push(record),
    }
    catalog.assets.sort_by(|a, b| a.id.cmp(&b.id));
}

/// Create a durable user/agent bin.
pub fn create_bin(
    meta: &mut AwidatTimelineMetadata,
    id: String,
    name: String,
    parent_id: Option<String>,
) -> Result<(), CatalogMutationError> {
    let catalog = ensure_asset_catalog(meta);
    if catalog.bins.iter().any(|bin| bin.id == id) {
        return Err(CatalogMutationError::DuplicateBin(id));
    }
    if let Some(parent_id) = parent_id.as_deref()
        && !catalog.bins.iter().any(|bin| bin.id == parent_id)
    {
        return Err(CatalogMutationError::MissingParentBin(
            parent_id.to_string(),
        ));
    }
    catalog.bins.push(AssetBin {
        id,
        name,
        parent_id,
    });
    catalog.bins.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(())
}

/// Move an asset to a durable bin, or clear its bin when `bin_id` is `None`.
pub fn move_asset_to_bin(
    meta: &mut AwidatTimelineMetadata,
    asset_id: &str,
    bin_id: Option<String>,
) -> Result<(), CatalogMutationError> {
    let catalog = ensure_asset_catalog(meta);
    if let Some(bin_id) = bin_id.as_deref()
        && !catalog.bins.iter().any(|bin| bin.id == bin_id)
    {
        return Err(CatalogMutationError::MissingBin(bin_id.to_string()));
    }
    let asset = catalog
        .assets
        .iter_mut()
        .find(|asset| asset.id == asset_id)
        .ok_or_else(|| CatalogMutationError::MissingAsset(asset_id.to_string()))?;
    asset.bin_id = bin_id;
    Ok(())
}

#[cfg(test)]
mod tests {
    use awidat_proto::professional::{AssetReadiness, AssetRole};

    use super::*;

    fn record(id: &str) -> AssetRecord {
        AssetRecord {
            id: id.to_string(),
            path: id.to_string(),
            role: AssetRole::Video,
            readiness: AssetReadiness::default(),
            ..AssetRecord::default()
        }
    }

    #[test]
    fn ensure_awidat_metadata_creates_missing_block() {
        let mut timeline = Timeline::empty("test");
        timeline.metadata.awidat = None;

        let meta = ensure_awidat_metadata(&mut timeline);

        assert_eq!(meta.version, AWIDAT_PROJECT_VERSION);
    }

    #[test]
    fn upsert_asset_creates_catalog_and_source_asset() {
        let mut timeline = Timeline::empty("test");
        let meta = ensure_awidat_metadata(&mut timeline);

        upsert_asset(meta, record("raw/a.mp4"));

        assert_eq!(meta.source_assets, vec!["raw/a.mp4".to_string()]);
        assert_eq!(
            meta.asset_catalog.as_ref().unwrap().assets[0].id,
            "raw/a.mp4"
        );
    }

    #[test]
    fn create_bin_rejects_missing_parent() {
        let mut timeline = Timeline::empty("test");
        let meta = ensure_awidat_metadata(&mut timeline);

        let err =
            create_bin(meta, "takes".into(), "Takes".into(), Some("missing".into())).unwrap_err();

        assert_eq!(
            err,
            CatalogMutationError::MissingParentBin("missing".into())
        );
    }

    #[test]
    fn move_asset_to_bin_updates_asset_bin() {
        let mut timeline = Timeline::empty("test");
        let meta = ensure_awidat_metadata(&mut timeline);
        upsert_asset(meta, record("raw/a.mp4"));
        create_bin(meta, "scene-1".into(), "Scene 1".into(), None).unwrap();

        move_asset_to_bin(meta, "raw/a.mp4", Some("scene-1".into())).unwrap();

        let catalog = meta.asset_catalog.as_ref().unwrap();
        assert_eq!(catalog.assets[0].bin_id.as_deref(), Some("scene-1"));
    }
}
