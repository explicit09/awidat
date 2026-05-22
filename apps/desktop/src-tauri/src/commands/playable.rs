//! Resolves what file the player should hand to <video src> for a
//! clip on the timeline. Falls back through (proxy → source → none)
//! so the timeline preview can play the original asset while a proxy
//! is still transcoding, instead of staring at a "Generating preview…"
//! placeholder.

use std::path::{Path, PathBuf};

use awidat_desktop_protocol::PlayableKind;

/// What the frontend will play for a given asset id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playable {
    pub path: Option<PathBuf>,
    pub kind: PlayableKind,
}

/// Resolve the play URL for `<project>/<asset_id>`. Prefers the 1080p
/// proxy when it exists, falls back to the source asset, returns
/// `Missing` only when the source itself is gone.
pub fn playable_for_asset_id(project_root: &Path, asset_id: &str) -> Playable {
    let abs = project_root.join(asset_id);
    if !abs.is_file() {
        return Playable {
            path: None,
            kind: PlayableKind::Missing,
        };
    }
    if let Some(proxy) = crate::commands::media::proxy_path_for_asset_id(project_root, asset_id) {
        return Playable {
            path: Some(PathBuf::from(proxy)),
            kind: PlayableKind::Proxy,
        };
    }
    Playable {
        path: Some(abs),
        kind: PlayableKind::Source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::media::proxy_path_for;

    #[test]
    fn returns_missing_when_source_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let got = playable_for_asset_id(tmp.path(), "raw/nope.mov");
        assert_eq!(got.kind, PlayableKind::Missing);
        assert!(got.path.is_none());
    }

    #[test]
    fn returns_source_when_source_present_but_no_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let asset = raw.join("clip.mov");
        std::fs::write(&asset, b"src").unwrap();
        let got = playable_for_asset_id(tmp.path(), "raw/clip.mov");
        assert_eq!(got.kind, PlayableKind::Source);
        assert_eq!(got.path.as_ref().unwrap(), &asset);
    }

    #[test]
    fn returns_proxy_when_proxy_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().join("raw");
        let proxies = tmp.path().join(".awidat").join("proxies");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::create_dir_all(&proxies).unwrap();
        let asset = raw.join("clip.mov");
        std::fs::write(&asset, b"src").unwrap();
        let proxy_path = proxy_path_for(&proxies, &asset);
        std::fs::write(&proxy_path, b"proxy").unwrap();
        let got = playable_for_asset_id(tmp.path(), "raw/clip.mov");
        assert_eq!(got.kind, PlayableKind::Proxy);
        assert_eq!(got.path.as_ref().unwrap(), &proxy_path);
    }
}
