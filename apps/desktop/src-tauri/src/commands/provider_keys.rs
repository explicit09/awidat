//! Desktop commands for provider API keys stored in the shared secrets vault.
//!
//! IPC never returns raw secret values. The UI gets provider metadata, status,
//! and a short redacted hint only.

use montage_secrets::{SecretVault, accounts, env_vars};
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
struct ProviderDefinition {
    key: &'static str,
    label: &'static str,
    account: &'static str,
    env_var: &'static str,
    capability: &'static str,
}

const PROVIDERS: &[ProviderDefinition] = &[
    ProviderDefinition {
        key: "hugging_face",
        label: "Hugging Face",
        account: accounts::HF_TOKEN,
        env_var: env_vars::HF_TOKEN,
        capability: "Diarization model downloads",
    },
    ProviderDefinition {
        key: "deepgram",
        label: "Deepgram",
        account: accounts::DEEPGRAM_API_KEY,
        env_var: env_vars::DEEPGRAM_API_KEY,
        capability: "Speech-to-text transcription",
    },
    ProviderDefinition {
        key: "openrouter",
        label: "OpenRouter",
        account: accounts::OPENROUTER_API_KEY,
        env_var: env_vars::OPENROUTER_API_KEY,
        capability: "Generated-media models",
    },
    ProviderDefinition {
        key: "anthropic",
        label: "Anthropic",
        account: accounts::ANTHROPIC_API_KEY,
        env_var: env_vars::ANTHROPIC_API_KEY,
        capability: "Premium topic labeling",
    },
    ProviderDefinition {
        key: "pexels",
        label: "Pexels",
        account: accounts::PEXELS_API_KEY,
        env_var: env_vars::PEXELS_API_KEY,
        capability: "Stock b-roll search",
    },
    ProviderDefinition {
        key: "x",
        label: "X",
        account: accounts::X_BEARER_TOKEN,
        env_var: env_vars::X_BEARER_TOKEN,
        capability: "Trend and context reads",
    },
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyRow {
    pub key: String,
    pub label: String,
    pub account: String,
    pub env_var: String,
    pub capability: String,
    pub status: ProviderKeyStatus,
    pub redacted: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKeyStatus {
    NotSet,
    Configured,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyTestResult {
    pub key: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderKeyImportSummary {
    pub imported: Vec<String>,
    pub rows: Vec<ProviderKeyRow>,
}

#[tauri::command]
pub async fn list_provider_keys() -> Result<Vec<ProviderKeyRow>, String> {
    let vault = montage_secrets::load_vault().map_err(|e| e.to_string())?;
    Ok(provider_rows(&vault))
}

#[tauri::command]
pub async fn save_provider_key(
    provider: String,
    value: String,
) -> Result<Vec<ProviderKeyRow>, String> {
    let definition = provider_definition(&provider)?;
    let value = validate_provider_value(definition, &value)?;
    let mut vault = montage_secrets::load_vault().map_err(|e| e.to_string())?;
    vault.set(definition.account, value);
    montage_secrets::save_vault(&vault).map_err(|e| e.to_string())?;
    Ok(provider_rows(&vault))
}

#[tauri::command]
pub async fn remove_provider_key(provider: String) -> Result<Vec<ProviderKeyRow>, String> {
    let definition = provider_definition(&provider)?;
    let mut vault = montage_secrets::load_vault().map_err(|e| e.to_string())?;
    vault.remove(definition.account);
    montage_secrets::save_vault(&vault).map_err(|e| e.to_string())?;
    Ok(provider_rows(&vault))
}

#[tauri::command]
pub async fn import_legacy_provider_keys() -> Result<ProviderKeyImportSummary, String> {
    let mut vault = montage_secrets::load_vault().map_err(|e| e.to_string())?;
    let mut imported = Vec::new();

    for definition in PROVIDERS {
        let Some(value) =
            montage_secrets::get_legacy_keychain(definition.env_var, definition.account)
                .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        vault.set(definition.account, value);
        imported.push(definition.key.to_string());
    }

    montage_secrets::save_vault(&vault).map_err(|e| e.to_string())?;
    Ok(ProviderKeyImportSummary {
        imported,
        rows: provider_rows(&vault),
    })
}

#[tauri::command]
pub async fn test_provider_key(
    provider: String,
    value: String,
) -> Result<ProviderKeyTestResult, String> {
    let definition = provider_definition(&provider)?;
    validate_provider_value(definition, &value)?;
    Ok(ProviderKeyTestResult {
        key: definition.key.to_string(),
        ok: true,
        message: "Key format looks usable. Live provider checks are not enabled yet.".to_string(),
    })
}

fn provider_rows(vault: &SecretVault) -> Vec<ProviderKeyRow> {
    PROVIDERS
        .iter()
        .map(|definition| {
            let value = vault.get(definition.account);
            ProviderKeyRow {
                key: definition.key.to_string(),
                label: definition.label.to_string(),
                account: definition.account.to_string(),
                env_var: definition.env_var.to_string(),
                capability: definition.capability.to_string(),
                status: if value.is_some() {
                    ProviderKeyStatus::Configured
                } else {
                    ProviderKeyStatus::NotSet
                },
                redacted: value.map(redact_secret),
            }
        })
        .collect()
}

fn provider_definition(provider: &str) -> Result<&'static ProviderDefinition, String> {
    PROVIDERS
        .iter()
        .find(|definition| definition.key == provider)
        .ok_or_else(|| {
            format!(
                "unknown provider '{}'; known providers: {}",
                provider,
                known_provider_keys()
            )
        })
}

fn known_provider_keys() -> String {
    PROVIDERS
        .iter()
        .map(|definition| definition.key)
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_provider_value<'a>(
    definition: &ProviderDefinition,
    value: &'a str,
) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{} key cannot be empty", definition.label));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!(
            "{} key cannot contain whitespace",
            definition.label
        ));
    }
    Ok(trimmed)
}

fn redact_secret(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "configured".to_string();
    }

    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_rows_redact_configured_keys() {
        let mut vault = SecretVault::default();
        vault.set(accounts::HF_TOKEN, "hf_secret_token_123456");
        vault.set(accounts::OPENROUTER_API_KEY, "sk-or-v1-secret");

        let rows = provider_rows(&vault);
        let hf = rows
            .iter()
            .find(|row| row.key == "hugging_face")
            .expect("hf row");
        let openrouter = rows
            .iter()
            .find(|row| row.key == "openrouter")
            .expect("openrouter row");

        assert_eq!(hf.status, ProviderKeyStatus::Configured);
        assert_eq!(hf.redacted.as_deref(), Some("hf_s...3456"));
        assert_eq!(openrouter.status, ProviderKeyStatus::Configured);
        assert_eq!(openrouter.redacted.as_deref(), Some("sk-o...cret"));
        assert_ne!(hf.redacted.as_deref(), Some("hf_secret_token_123456"));
        assert!(!format!("{rows:?}").contains("hf_secret_token_123456"));
        assert!(!format!("{rows:?}").contains("sk-or-v1-secret"));
    }

    #[test]
    fn provider_rows_include_required_provider_metadata() {
        let rows = provider_rows(&SecretVault::default());
        let keys = rows.iter().map(|row| row.key.as_str()).collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                "hugging_face",
                "deepgram",
                "openrouter",
                "anthropic",
                "pexels",
                "x"
            ]
        );
        assert!(rows
            .iter()
            .all(|row| row.status == ProviderKeyStatus::NotSet));
        assert!(rows.iter().all(|row| row.redacted.is_none()));
        assert!(
            rows.iter()
                .any(|row| row.env_var == env_vars::DEEPGRAM_API_KEY)
        );
    }

    #[test]
    fn unknown_provider_error_lists_known_keys() {
        let err = provider_definition("missing").expect_err("unknown provider should fail");

        assert!(err.contains("unknown provider 'missing'"));
        assert!(err.contains("hugging_face"));
        assert!(err.contains("deepgram"));
        assert!(err.contains("openrouter"));
        assert!(err.contains("x"));
    }

    #[test]
    fn validate_provider_value_trims_and_rejects_empty_or_whitespace() {
        let definition = provider_definition("deepgram").expect("provider");

        assert_eq!(
            validate_provider_value(definition, "  dg-valid-token  ").expect("valid token"),
            "dg-valid-token"
        );
        assert!(validate_provider_value(definition, "   ").is_err());
        assert!(validate_provider_value(definition, "dg invalid").is_err());
    }

    #[test]
    fn test_provider_key_result_does_not_include_secret() {
        let definition = provider_definition("pexels").expect("provider");
        validate_provider_value(definition, "pexels-secret-123").expect("valid");
        let result = ProviderKeyTestResult {
            key: definition.key.to_string(),
            ok: true,
            message: "Key format looks usable. Live provider checks are not enabled yet."
                .to_string(),
        };

        assert!(!format!("{result:?}").contains("pexels-secret-123"));
    }
}
