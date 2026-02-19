//! ProviderKeyring: encrypted storage for LLM provider API keys only.
//!
//! Separate from SkillKeychain (tool/skill secrets) and Keyring (Ed25519 identity keys).
//! Stored in: `{data_dir}/provider_keys.bin`
//! Format: `HashMap<String, SecretString>` serialized to JSON, DPAPI-encrypted.
//!
//! Keys are stored as bare provider names (`openai`, `anthropic`) — no `provider:` prefix
//! since this store is provider-only by definition.

use crate::dpapi::{dpapi_decrypt, dpapi_encrypt};
use crate::error::{CoreError, Result};
use secrecy::{ExposeSecret, SecretString};
use std::collections::HashMap;
use std::path::PathBuf;

/// Known LLM providers that belong in the keyring (not the skill keychain).
pub const KNOWN_LLM_PROVIDERS: &[&str] = &["openai", "anthropic", "perplexity", "xai", "google"];

/// Returns true if `name` is a known LLM provider.
pub fn is_known_llm_provider(name: &str) -> bool {
    KNOWN_LLM_PROVIDERS.contains(&name)
}

/// DPAPI-encrypted storage exclusively for LLM provider API keys.
///
/// Values use `SecretString` for zeroize-on-drop and redacted `Debug` output.
/// The keyring is designed to be cached in memory (behind an `RwLock`) so
/// read-heavy operations (every chat turn) avoid disk I/O.
#[derive(Clone)]
pub struct ProviderKeyring {
    storage_path: PathBuf,
    keys: HashMap<String, SecretString>,
}

impl std::fmt::Debug for ProviderKeyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderKeyring")
            .field("storage_path", &self.storage_path)
            .field("provider_count", &self.keys.len())
            .finish()
    }
}

impl ProviderKeyring {
    /// Create a new empty keyring at the given directory path.
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            storage_path: data_dir.join("provider_keys.bin"),
            keys: HashMap::new(),
        }
    }

    /// Load keyring from DPAPI-encrypted storage.
    /// Returns an empty keyring if the file doesn't exist yet.
    pub fn load(data_dir: PathBuf) -> Result<Self> {
        let storage_path = data_dir.join("provider_keys.bin");

        if !storage_path.exists() {
            return Ok(Self {
                storage_path,
                keys: HashMap::new(),
            });
        }

        let encrypted = std::fs::read(&storage_path)?;
        let decrypted = dpapi_decrypt(&encrypted)?;
        let raw: HashMap<String, String> = serde_json::from_slice(&decrypted)
            .map_err(|e| CoreError::Keyring(format!("Failed to parse provider keys: {}", e)))?;

        let keys = raw
            .into_iter()
            .map(|(k, v)| (k, SecretString::new(v)))
            .collect();

        Ok(Self { storage_path, keys })
    }

    /// Save keyring to DPAPI-encrypted storage.
    pub fn save(&self) -> Result<()> {
        // Serialize by exposing secrets temporarily for JSON encoding.
        let raw: HashMap<&str, &str> = self
            .keys
            .iter()
            .map(|(k, v)| (k.as_str(), v.expose_secret().as_str()))
            .collect();
        let serialized = serde_json::to_vec(&raw)?;
        let encrypted = dpapi_encrypt(&serialized)?;

        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.storage_path, encrypted)?;
        Ok(())
    }

    /// Get a provider key as a `SecretString` reference.
    pub fn get_key(&self, provider: &str) -> Option<&SecretString> {
        self.keys.get(provider)
    }

    /// Get a provider key as a plain `&str` (convenience via `ExposeSecret`).
    pub fn get_key_str(&self, provider: &str) -> Option<&str> {
        self.keys.get(provider).map(|s| s.expose_secret().as_str())
    }

    /// Set a provider key. Call `save()` to persist.
    pub fn set_key(&mut self, provider: &str, key: SecretString) {
        self.keys.insert(provider.to_string(), key);
    }

    /// Set a provider key from a plain string. Call `save()` to persist.
    pub fn set_key_str(&mut self, provider: &str, key: &str) {
        self.keys
            .insert(provider.to_string(), SecretString::new(key.to_string()));
    }

    /// Remove a provider key. Returns true if it existed. Call `save()` to persist.
    pub fn remove_key(&mut self, provider: &str) -> bool {
        self.keys.remove(provider).is_some()
    }

    /// List all provider names that have stored keys.
    pub fn list_providers(&self) -> Vec<&str> {
        self.keys.keys().map(|s| s.as_str()).collect()
    }

    /// Returns true if a key exists for the given provider.
    pub fn has_key(&self, provider: &str) -> bool {
        self.keys.contains_key(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyring_roundtrip() {
        let tmp = std::env::temp_dir().join("orion_keyring_test_rt");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut kr = ProviderKeyring::new(tmp.clone());
        kr.set_key_str("openai", "sk-test-key-123");
        kr.set_key_str("anthropic", "sk-ant-key-456");
        kr.save().unwrap();

        let loaded = ProviderKeyring::load(tmp.clone()).unwrap();
        assert_eq!(loaded.get_key_str("openai"), Some("sk-test-key-123"));
        assert_eq!(loaded.get_key_str("anthropic"), Some("sk-ant-key-456"));
        assert_eq!(loaded.get_key_str("missing"), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_keyring_empty_load() {
        let tmp = std::env::temp_dir().join("orion_keyring_test_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let kr = ProviderKeyring::load(tmp.clone()).unwrap();
        assert!(kr.list_providers().is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_keyring_remove() {
        let mut kr = ProviderKeyring::new(std::env::temp_dir());
        kr.set_key_str("openai", "sk-test");
        assert!(kr.remove_key("openai"));
        assert!(!kr.remove_key("openai"));
        assert_eq!(kr.get_key_str("openai"), None);
    }

    #[test]
    fn test_keyring_has_key() {
        let mut kr = ProviderKeyring::new(std::env::temp_dir());
        assert!(!kr.has_key("openai"));
        kr.set_key_str("openai", "sk-test");
        assert!(kr.has_key("openai"));
    }

    #[test]
    fn test_known_llm_provider() {
        assert!(is_known_llm_provider("openai"));
        assert!(is_known_llm_provider("anthropic"));
        assert!(is_known_llm_provider("perplexity"));
        assert!(!is_known_llm_provider("tavily"));
        assert!(!is_known_llm_provider("protonmail"));
    }
}
