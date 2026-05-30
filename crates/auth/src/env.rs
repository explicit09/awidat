//! Resolving *where* codex stores credentials, so awidat writes to the same place.

use std::path::{Path, PathBuf};

use codex_login::AuthCredentialsStoreMode;
use codex_utils_home_dir::find_codex_home;
use serde::Deserialize;

use crate::AuthError;

/// The resolved credential location + storage backend codex uses.
///
/// Held by value and passed to every operation so a login performed through
/// awidat lands exactly where the running agent reads it.
#[derive(Debug, Clone)]
pub struct AuthEnv {
    /// `CODEX_HOME` (default `~/.codex`).
    pub codex_home: PathBuf,
    /// Backend for `auth.json` (file / keyring / auto / ephemeral).
    pub store_mode: AuthCredentialsStoreMode,
}

impl AuthEnv {
    /// Resolve from the environment the same way codex does: `CODEX_HOME`
    /// (default `~/.codex`) plus the `cli_auth_credentials_store` config key
    /// (default [`AuthCredentialsStoreMode::File`]).
    pub fn resolve() -> Result<Self, AuthError> {
        let codex_home = find_codex_home().map_err(AuthError::Home)?.into_path_buf();
        let store_mode = read_store_mode(&codex_home);
        Ok(Self {
            codex_home,
            store_mode,
        })
    }

    /// Construct explicitly. Test seam and advanced callers that already know
    /// the location.
    pub fn new(codex_home: PathBuf, store_mode: AuthCredentialsStoreMode) -> Self {
        Self {
            codex_home,
            store_mode,
        }
    }
}

/// Best-effort read of `cli_auth_credentials_store` from `$CODEX_HOME/config.toml`.
///
/// A missing file, unreadable file, parse error, or absent key all fall back to
/// codex's default (`file`), which is correct for ~all users. We deliberately
/// parse only the one key so unrelated config we don't model can't break us.
fn read_store_mode(codex_home: &Path) -> AuthCredentialsStoreMode {
    #[derive(Deserialize)]
    struct StoreModeConfig {
        cli_auth_credentials_store: Option<AuthCredentialsStoreMode>,
    }

    let path = codex_home.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return AuthCredentialsStoreMode::default();
    };
    toml::from_str::<StoreModeConfig>(&text)
        .ok()
        .and_then(|config| config.cli_auth_credentials_store)
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &Path, body: &str) {
        std::fs::write(dir.join("config.toml"), body).unwrap();
    }

    #[test]
    fn missing_config_defaults_to_file() {
        let home = TempDir::new().unwrap();
        assert_eq!(read_store_mode(home.path()), AuthCredentialsStoreMode::File);
    }

    #[test]
    fn config_without_key_defaults_to_file() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "model = \"gpt-5.5\"\n");
        assert_eq!(read_store_mode(home.path()), AuthCredentialsStoreMode::File);
    }

    #[test]
    fn keyring_mode_is_read() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "cli_auth_credentials_store = \"keyring\"\n");
        assert_eq!(
            read_store_mode(home.path()),
            AuthCredentialsStoreMode::Keyring
        );
    }

    #[test]
    fn unrelated_config_around_the_key_is_ignored() {
        let home = TempDir::new().unwrap();
        write_config(
            home.path(),
            "model = \"gpt-5.5\"\ncli_auth_credentials_store = \"auto\"\n\n[mcp_servers.awidat]\ncommand = \"awidat-mcp-server\"\n",
        );
        assert_eq!(read_store_mode(home.path()), AuthCredentialsStoreMode::Auto);
    }
}
