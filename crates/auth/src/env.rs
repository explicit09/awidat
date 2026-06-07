//! Resolving *where* codex stores credentials, so montage writes to the same place.

use std::path::{Path, PathBuf};

use codex_login::AuthCredentialsStoreMode;
use codex_utils_home_dir::find_codex_home;
use serde::Deserialize;

use crate::AuthError;

/// A managed-install login restriction from codex's `forced_login_method`.
///
/// Managed/enterprise deployments can lock the agent to one method; we honor it
/// so the chooser can't quietly switch a locked install to the other wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForcedMethod {
    /// Only "Sign in with ChatGPT" is permitted.
    ChatGpt,
    /// Only API-key auth is permitted.
    Api,
}

/// The resolved credential location + storage backend codex uses, plus any
/// managed-install login restrictions.
///
/// Held by value and passed to every operation so a login performed through
/// montage lands exactly where the running agent reads it and respects the same
/// policy codex would enforce.
#[derive(Debug, Clone)]
pub struct AuthEnv {
    /// `CODEX_HOME` (default `~/.codex`).
    pub codex_home: PathBuf,
    /// Backend for `auth.json` (file / keyring / auto / ephemeral).
    pub store_mode: AuthCredentialsStoreMode,
    /// `forced_login_method` policy, if a managed install set one.
    pub forced_login_method: Option<ForcedMethod>,
    /// `forced_chatgpt_workspace_id` allow-list, passed to the OAuth callback so
    /// credentials from other workspaces are rejected.
    pub forced_workspace_ids: Option<Vec<String>>,
}

impl AuthEnv {
    /// Resolve from the environment the same way codex does: `CODEX_HOME`
    /// (default `~/.codex`) plus the relevant `config.toml` keys
    /// (`cli_auth_credentials_store`, `forced_login_method`,
    /// `forced_chatgpt_workspace_id`).
    pub fn resolve() -> Result<Self, AuthError> {
        let codex_home = find_codex_home().map_err(AuthError::Home)?.into_path_buf();
        let policy = read_policy(&codex_home);
        Ok(Self {
            codex_home,
            store_mode: policy.store_mode,
            forced_login_method: policy.forced_login_method,
            forced_workspace_ids: policy.forced_workspace_ids,
        })
    }

    /// Construct explicitly with no forced policy. Test seam and advanced callers
    /// that already know the location.
    pub fn new(codex_home: PathBuf, store_mode: AuthCredentialsStoreMode) -> Self {
        Self {
            codex_home,
            store_mode,
            forced_login_method: None,
            forced_workspace_ids: None,
        }
    }
}

/// The subset of `config.toml` this crate cares about.
struct Policy {
    store_mode: AuthCredentialsStoreMode,
    forced_login_method: Option<ForcedMethod>,
    forced_workspace_ids: Option<Vec<String>>,
}

/// Best-effort read of the auth-relevant keys from `$CODEX_HOME/config.toml`.
///
/// A missing file, unreadable file, parse error, or absent keys all fall back to
/// safe defaults (store = `file`, no forced policy), correct for ~all users. We
/// deliberately model only the keys we use so unrelated config can't break us.
fn read_policy(codex_home: &Path) -> Policy {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    #[derive(Deserialize)]
    struct AuthConfig {
        cli_auth_credentials_store: Option<AuthCredentialsStoreMode>,
        forced_login_method: Option<String>,
        forced_chatgpt_workspace_id: Option<OneOrMany>,
    }

    let path = codex_home.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Policy {
            store_mode: AuthCredentialsStoreMode::default(),
            forced_login_method: None,
            forced_workspace_ids: None,
        };
    };
    let Ok(config) = toml::from_str::<AuthConfig>(&text) else {
        return Policy {
            store_mode: AuthCredentialsStoreMode::default(),
            forced_login_method: None,
            forced_workspace_ids: None,
        };
    };

    let forced_login_method = config.forced_login_method.as_deref().and_then(|value| {
        match value.trim().to_ascii_lowercase().as_str() {
            "chatgpt" => Some(ForcedMethod::ChatGpt),
            "api" => Some(ForcedMethod::Api),
            _ => None,
        }
    });
    let forced_workspace_ids = config.forced_chatgpt_workspace_id.map(|ids| match ids {
        OneOrMany::One(id) => vec![id],
        OneOrMany::Many(ids) => ids,
    });

    Policy {
        store_mode: config.cli_auth_credentials_store.unwrap_or_default(),
        forced_login_method,
        forced_workspace_ids,
    }
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
    fn missing_config_defaults_to_file_and_no_policy() {
        let home = TempDir::new().unwrap();
        let policy = read_policy(home.path());
        assert_eq!(policy.store_mode, AuthCredentialsStoreMode::File);
        assert_eq!(policy.forced_login_method, None);
        assert_eq!(policy.forced_workspace_ids, None);
    }

    #[test]
    fn config_without_key_defaults_to_file() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "model = \"gpt-5.5\"\n");
        assert_eq!(
            read_policy(home.path()).store_mode,
            AuthCredentialsStoreMode::File
        );
    }

    #[test]
    fn keyring_mode_is_read() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "cli_auth_credentials_store = \"keyring\"\n");
        assert_eq!(
            read_policy(home.path()).store_mode,
            AuthCredentialsStoreMode::Keyring
        );
    }

    #[test]
    fn unrelated_config_around_the_key_is_ignored() {
        let home = TempDir::new().unwrap();
        write_config(
            home.path(),
            "model = \"gpt-5.5\"\ncli_auth_credentials_store = \"auto\"\n\n[mcp_servers.montage]\ncommand = \"montage-mcp-server\"\n",
        );
        assert_eq!(
            read_policy(home.path()).store_mode,
            AuthCredentialsStoreMode::Auto
        );
    }

    #[test]
    fn forced_login_method_is_parsed() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "forced_login_method = \"chatgpt\"\n");
        assert_eq!(
            read_policy(home.path()).forced_login_method,
            Some(ForcedMethod::ChatGpt)
        );

        write_config(home.path(), "forced_login_method = \"api\"\n");
        assert_eq!(
            read_policy(home.path()).forced_login_method,
            Some(ForcedMethod::Api)
        );
    }

    #[test]
    fn forced_workspace_id_accepts_string_or_list() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "forced_chatgpt_workspace_id = \"ws-1\"\n");
        assert_eq!(
            read_policy(home.path()).forced_workspace_ids,
            Some(vec!["ws-1".to_string()])
        );

        write_config(
            home.path(),
            "forced_chatgpt_workspace_id = [\"ws-1\", \"ws-2\"]\n",
        );
        assert_eq!(
            read_policy(home.path()).forced_workspace_ids,
            Some(vec!["ws-1".to_string(), "ws-2".to_string()])
        );
    }
}
