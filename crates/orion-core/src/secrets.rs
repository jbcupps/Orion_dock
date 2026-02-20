//! SecretsVault: DPAPI-encrypted storage for provider API keys.
//!
//! Separate from Keyring (identity keys) and ExternalVault (pubkey verification).
//! Stored in: `{data_dir}/secrets.bin`
//! Format: `HashMap<String, String>` serialized to JSON, DPAPI-encrypted.

use crate::dpapi::{dpapi_decrypt, dpapi_encrypt};
use crate::error::{CoreError, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// DPAPI-encrypted storage for provider API keys and other secrets.
pub struct SecretsVault {
    storage_path: PathBuf,
    secrets: HashMap<String, String>,
}

impl SecretsVault {
    /// Create a new empty vault at the given directory path.
    /// Call `load()` to read existing secrets, or `save()` to persist.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            storage_path: data_dir.join("secrets.bin"),
            secrets: HashMap::new(),
        }
    }

    /// Load secrets from DPAPI-encrypted storage.
    /// Returns an empty vault if the file doesn't exist yet.
    pub fn load(data_dir: PathBuf) -> Result<Self> {
        let storage_path = data_dir.join("secrets.bin");

        if !storage_path.exists() {
            return Ok(Self {
                storage_path,
                secrets: HashMap::new(),
            });
        }

        let encrypted = std::fs::read(&storage_path)?;
        let decrypted = dpapi_decrypt(&encrypted)?;
        let secrets: HashMap<String, String> = serde_json::from_slice(&decrypted)
            .map_err(|e| CoreError::Keyring(format!("Failed to parse secrets: {}", e)))?;

        Ok(Self {
            storage_path,
            secrets,
        })
    }

    /// Save secrets to DPAPI-encrypted storage.
    pub fn save(&self) -> Result<()> {
        let serialized = serde_json::to_vec(&self.secrets)?;
        let encrypted = dpapi_encrypt(&serialized)?;

        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.storage_path, encrypted)?;

        Ok(())
    }

    /// Get a secret by provider name.
    pub fn get_secret(&self, provider: &str) -> Option<&str> {
        self.secrets.get(provider).map(|s| s.as_str())
    }

    /// Set a secret for a provider. Call `save()` to persist.
    pub fn set_secret(&mut self, provider: &str, key: &str) {
        self.secrets.insert(provider.to_string(), key.to_string());
    }

    /// Remove a secret. Call `save()` to persist.
    pub fn remove_secret(&mut self, provider: &str) -> bool {
        self.secrets.remove(provider).is_some()
    }

    /// Returns true if a secret exists for the given key (without decrypting).
    pub fn exists(&self, key: &str) -> bool {
        self.secrets.contains_key(key)
    }

    /// List all provider names that have stored secrets.
    pub fn list_providers(&self) -> Vec<&str> {
        self.secrets.keys().map(|s| s.as_str()).collect()
    }

    /// Migrate un-namespaced provider keys to `provider:` namespace.
    /// E.g. key "openai" → "provider:openai".
    /// Returns a list of providers that were migrated. Call `save()` after.
    pub fn migrate_namespace(&mut self) -> Vec<String> {
        let known_providers = [
            "openai",
            "anthropic",
            "perplexity",
            "xai",
            "google",
            "tavily",
        ];
        let mut migrated = Vec::new();
        for &provider in &known_providers {
            let ns_key = format!("provider:{}", provider);
            if self.secrets.contains_key(provider) && !self.secrets.contains_key(&ns_key) {
                if let Some(val) = self.secrets.remove(provider) {
                    self.secrets.insert(ns_key, val);
                    migrated.push(provider.to_string());
                    tracing::info!(
                        "Migrated vault key '{}' → 'provider:{}'",
                        provider,
                        provider
                    );
                }
            }
        }
        migrated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secrets_vault_roundtrip() {
        let _guard = crate::test_util::MASTER_KEY_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join("orion_secrets_test_rt");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut vault = SecretsVault::new(tmp.clone());
        vault.set_secret("openai", "sk-test-key-123");
        vault.set_secret("anthropic", "sk-ant-key-456");
        vault.save().unwrap();

        let loaded = SecretsVault::load(tmp.clone()).unwrap();
        assert_eq!(loaded.get_secret("openai"), Some("sk-test-key-123"));
        assert_eq!(loaded.get_secret("anthropic"), Some("sk-ant-key-456"));
        assert_eq!(loaded.get_secret("missing"), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_secrets_vault_empty_load() {
        let _guard = crate::test_util::MASTER_KEY_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join("orion_secrets_test_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let vault = SecretsVault::load(tmp.clone()).unwrap();
        assert!(vault.list_providers().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_secrets_vault_remove() {
        let tmp = std::env::temp_dir().join("orion_secrets_test_rm");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut vault = SecretsVault::new(tmp.clone());
        vault.set_secret("openai", "sk-test");
        assert!(vault.remove_secret("openai"));
        assert!(!vault.remove_secret("openai"));
        assert_eq!(vault.get_secret("openai"), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_secrets_vault_list_providers() {
        let mut vault = SecretsVault::new(std::env::temp_dir());
        vault.set_secret("openai", "key1");
        vault.set_secret("anthropic", "key2");

        let mut providers = vault.list_providers();
        providers.sort();
        assert_eq!(providers, vec!["anthropic", "openai"]);
    }
}
