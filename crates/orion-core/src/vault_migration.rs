//! One-time migration from legacy `secrets.bin` to split `provider_keys.bin` + `skill_secrets.bin`.
//!
//! Runs at startup. If `secrets.bin` exists but `provider_keys.bin` does not, the legacy
//! vault is loaded, keys are partitioned, new files are saved, and the original is renamed
//! to `secrets.bin.migrated` as a backup.

use crate::provider_keyring::{is_known_llm_provider, ProviderKeyring};
use crate::skill_keychain::SkillKeychain;
use std::path::Path;
use tracing;

/// Run the migration if needed. Safe to call unconditionally at startup.
///
/// Returns `true` if migration was performed, `false` if skipped (already migrated or no legacy file).
pub fn migrate_if_needed(data_dir: &Path) -> bool {
    let legacy_path = data_dir.join("secrets.bin");
    let new_keyring_path = data_dir.join("provider_keys.bin");

    // Already migrated or no legacy file — nothing to do.
    if !legacy_path.exists() || new_keyring_path.exists() {
        return false;
    }

    tracing::info!("Found legacy secrets.bin without provider_keys.bin — running vault migration");

    // Load the legacy vault using raw DPAPI (avoid depending on the deprecated SecretsVault struct).
    let encrypted = match std::fs::read(&legacy_path) {
        Ok(data) => data,
        Err(e) => {
            tracing::error!("Failed to read legacy secrets.bin: {}", e);
            return false;
        }
    };
    let decrypted = match crate::dpapi::dpapi_decrypt(&encrypted) {
        Ok(data) => data,
        Err(e) => {
            tracing::error!("Failed to decrypt legacy secrets.bin: {}", e);
            return false;
        }
    };
    let legacy_map: std::collections::HashMap<String, String> =
        match serde_json::from_slice(&decrypted) {
            Ok(map) => map,
            Err(e) => {
                tracing::error!("Failed to parse legacy secrets.bin: {}", e);
                return false;
            }
        };

    let mut keyring = ProviderKeyring::new(data_dir.to_path_buf());
    let mut keychain = SkillKeychain::new(data_dir.to_path_buf());

    for (key, value) in &legacy_map {
        if let Some(provider) = key.strip_prefix("provider:") {
            if provider == "tavily" {
                // Tavily is NOT an LLM provider — goes to skill keychain only.
                keychain.set_secret("tavily", value);
                tracing::debug!("Migration: provider:tavily -> SkillKeychain as 'tavily'");
            } else if is_known_llm_provider(provider) {
                // Known LLM provider — goes to keyring (prefix stripped).
                keyring.set_key_str(provider, value);
                tracing::debug!("Migration: provider:{} -> ProviderKeyring", provider);
                // Perplexity is dual-use: also copy to skill keychain for search skill.
                if provider == "perplexity" {
                    keychain.set_secret("perplexity", value);
                    tracing::debug!(
                        "Migration: provider:perplexity -> SkillKeychain (dual-use copy)"
                    );
                }
            } else {
                // Unknown provider: prefix — treat as skill secret (strip prefix).
                keychain.set_secret(provider, value);
                tracing::debug!(
                    "Migration: provider:{} -> SkillKeychain (unknown provider)",
                    provider
                );
            }
        } else {
            // No prefix — goes to skill keychain as-is.
            keychain.set_secret(key, value);
            tracing::debug!("Migration: '{}' -> SkillKeychain", key);
        }
    }

    // Save new files.
    if let Err(e) = keyring.save() {
        tracing::error!("Failed to save provider_keys.bin during migration: {}", e);
        return false;
    }
    if let Err(e) = keychain.save() {
        tracing::error!("Failed to save skill_secrets.bin during migration: {}", e);
        return false;
    }

    // Rename the legacy file as a backup.
    let migrated_path = data_dir.join("secrets.bin.migrated");
    if let Err(e) = std::fs::rename(&legacy_path, &migrated_path) {
        tracing::warn!(
            "Migration succeeded but failed to rename secrets.bin -> secrets.bin.migrated: {}",
            e
        );
    }

    tracing::info!(
        providers = keyring.list_providers().len(),
        skill_secrets = keychain.list_keys().len(),
        "Vault migration complete: secrets.bin split into provider_keys.bin + skill_secrets.bin"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dpapi::dpapi_encrypt;
    use std::collections::HashMap;

    fn write_legacy_vault(dir: &Path, entries: &[(&str, &str)]) {
        let map: HashMap<String, String> = entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let json = serde_json::to_vec(&map).unwrap();
        let encrypted = dpapi_encrypt(&json).unwrap();
        std::fs::write(dir.join("secrets.bin"), encrypted).unwrap();
    }

    #[test]
    fn test_migration_splits_correctly() {
        let tmp = std::env::temp_dir().join("orion_vault_migration_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        write_legacy_vault(
            &tmp,
            &[
                ("provider:openai", "sk-openai-key"),
                ("provider:anthropic", "sk-ant-key"),
                ("provider:tavily", "tvly-key"),
                ("provider:perplexity", "pplx-key"),
                ("email:acc1:password", "email-pw"),
                ("protonmail", "proton-pw"),
            ],
        );

        assert!(migrate_if_needed(&tmp));

        // Verify provider keyring
        let kr = ProviderKeyring::load(tmp.clone()).unwrap();
        assert_eq!(kr.get_key_str("openai"), Some("sk-openai-key"));
        assert_eq!(kr.get_key_str("anthropic"), Some("sk-ant-key"));
        assert_eq!(kr.get_key_str("perplexity"), Some("pplx-key"));
        assert!(!kr.has_key("tavily")); // tavily NOT in keyring

        // Verify skill keychain
        let kc = SkillKeychain::load(tmp.clone()).unwrap();
        assert_eq!(kc.get_secret("tavily"), Some("tvly-key"));
        assert_eq!(kc.get_secret("perplexity"), Some("pplx-key")); // dual-use
        assert_eq!(kc.get_secret("email:acc1:password"), Some("email-pw"));
        assert_eq!(kc.get_secret("protonmail"), Some("proton-pw"));

        // Legacy file renamed
        assert!(!tmp.join("secrets.bin").exists());
        assert!(tmp.join("secrets.bin.migrated").exists());

        // Second call is no-op
        assert!(!migrate_if_needed(&tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_migration_noop_no_legacy() {
        let tmp = std::env::temp_dir().join("orion_vault_migration_noop");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        assert!(!migrate_if_needed(&tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_migration_noop_already_migrated() {
        let tmp = std::env::temp_dir().join("orion_vault_migration_already");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Create both legacy and new files
        write_legacy_vault(&tmp, &[("provider:openai", "key")]);
        let kr = ProviderKeyring::new(tmp.clone());
        kr.save().unwrap();

        assert!(!migrate_if_needed(&tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
