//! SkillKeychain: encrypted storage for non-provider secrets (skill API keys, passwords, etc.).
//!
//! Separate from ProviderKeyring (LLM provider keys) and Keyring (Ed25519 identity keys).
//! Stored in: `{data_dir}/skill_secrets.bin`
//! Format: `HashMap<String, String>` serialized to JSON, DPAPI-encrypted.
//!
//! Keys are stored as-is: `tavily`, `protonmail`, `email:acc1:password`, etc.

use crate::dpapi::{dpapi_decrypt, dpapi_encrypt};
use crate::error::{CoreError, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// DPAPI-encrypted storage for skill secrets (everything except LLM provider API keys).
#[derive(Clone)]
pub struct SkillKeychain {
    storage_path: PathBuf,
    secrets: HashMap<String, String>,
}

impl SkillKeychain {
    /// Create a new empty keychain at the given directory path.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            storage_path: data_dir.join("skill_secrets.bin"),
            secrets: HashMap::new(),
        }
    }

    /// Load keychain from DPAPI-encrypted storage.
    /// Returns an empty keychain if the file doesn't exist yet.
    pub fn load(data_dir: PathBuf) -> Result<Self> {
        let storage_path = data_dir.join("skill_secrets.bin");

        if !storage_path.exists() {
            return Ok(Self {
                storage_path,
                secrets: HashMap::new(),
            });
        }

        let encrypted = std::fs::read(&storage_path)?;
        let decrypted = dpapi_decrypt(&encrypted)?;
        let secrets: HashMap<String, String> = serde_json::from_slice(&decrypted)
            .map_err(|e| CoreError::Keyring(format!("Failed to parse skill secrets: {}", e)))?;

        Ok(Self {
            storage_path,
            secrets,
        })
    }

    /// Save keychain to DPAPI-encrypted storage.
    pub fn save(&self) -> Result<()> {
        let serialized = serde_json::to_vec(&self.secrets)?;
        let encrypted = dpapi_encrypt(&serialized)?;

        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.storage_path, encrypted)?;
        Ok(())
    }

    /// Get a secret by key name.
    pub fn get_secret(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(|s| s.as_str())
    }

    /// Set a secret. Call `save()` to persist.
    pub fn set_secret(&mut self, key: &str, value: &str) {
        self.secrets.insert(key.to_string(), value.to_string());
    }

    /// Remove a secret. Call `save()` to persist.
    pub fn remove_secret(&mut self, key: &str) -> bool {
        self.secrets.remove(key).is_some()
    }

    /// Returns true if a secret exists for the given key.
    pub fn exists(&self, key: &str) -> bool {
        self.secrets.contains_key(key)
    }

    /// List all key names that have stored secrets.
    pub fn list_keys(&self) -> Vec<&str> {
        self.secrets.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keychain_roundtrip() {
        let _guard = crate::test_util::MASTER_KEY_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join("orion_keychain_test_rt");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut kc = SkillKeychain::new(tmp.clone());
        kc.set_secret("tavily", "tvly-test-key");
        kc.set_secret("protonmail", "bridge-pw-123");
        kc.save().unwrap();

        let loaded = SkillKeychain::load(tmp.clone()).unwrap();
        assert_eq!(loaded.get_secret("tavily"), Some("tvly-test-key"));
        assert_eq!(loaded.get_secret("protonmail"), Some("bridge-pw-123"));
        assert_eq!(loaded.get_secret("missing"), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_keychain_empty_load() {
        let _guard = crate::test_util::MASTER_KEY_ENV_LOCK.lock().unwrap();
        let tmp = std::env::temp_dir().join("orion_keychain_test_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let kc = SkillKeychain::load(tmp.clone()).unwrap();
        assert!(kc.list_keys().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_keychain_remove() {
        let mut kc = SkillKeychain::new(std::env::temp_dir());
        kc.set_secret("tavily", "test");
        assert!(kc.remove_secret("tavily"));
        assert!(!kc.remove_secret("tavily"));
        assert_eq!(kc.get_secret("tavily"), None);
    }

    #[test]
    fn test_keychain_exists() {
        let mut kc = SkillKeychain::new(std::env::temp_dir());
        assert!(!kc.exists("tavily"));
        kc.set_secret("tavily", "test");
        assert!(kc.exists("tavily"));
    }

    #[test]
    fn test_keychain_list_keys() {
        let mut kc = SkillKeychain::new(std::env::temp_dir());
        kc.set_secret("tavily", "k1");
        kc.set_secret("protonmail", "k2");

        let mut keys = kc.list_keys();
        keys.sort();
        assert_eq!(keys, vec!["protonmail", "tavily"]);
    }
}
