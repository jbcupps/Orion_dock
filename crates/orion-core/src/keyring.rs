use crate::document::DocumentTier;
use crate::dpapi::{dpapi_decrypt, dpapi_encrypt};
use crate::error::{CoreError, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier as DalekVerifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct StoredKeysV2 {
    mentor_secret: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct StoredKeysV1 {
    install_pubkey: [u8; 32],
    mentor_secret: Vec<u8>,
}

/// DPAPI-protected keyring for the mentor's Ed25519 signing identity.
///
/// Manages generation, persistence (encrypted via DPAPI on Windows, plaintext
/// fallback on Linux/Docker), and cryptographic signing/verification of data
/// using the mentor's keypair.
pub struct Keyring {
    mentor_keypair: SigningKey,
    storage_path: PathBuf,
}

impl Keyring {
    /// Generate a fresh Ed25519 mentor keypair for a new installation.
    pub fn generate(storage_path: PathBuf) -> Result<Self> {
        let mentor_keypair = SigningKey::generate(&mut OsRng);
        Ok(Self {
            mentor_keypair,
            storage_path,
        })
    }

    /// Load an existing mentor keypair from encrypted storage (`keys.bin`).
    /// Supports both V1 (with install pubkey) and V2 (mentor-only) formats.
    pub fn load(storage_path: PathBuf) -> Result<Self> {
        let keys_file = storage_path.join("keys.bin");
        let encrypted = std::fs::read(&keys_file)?;
        let decrypted = dpapi_decrypt(&encrypted)?;
        if let Ok(stored) = serde_json::from_slice::<StoredKeysV2>(&decrypted) {
            let mentor_slice: [u8; 32] = stored
                .mentor_secret
                .as_slice()
                .try_into()
                .map_err(|_| CoreError::Crypto("Invalid mentor key length".into()))?;
            return Ok(Self {
                mentor_keypair: SigningKey::from_bytes(&mentor_slice),
                storage_path,
            });
        }
        let stored: StoredKeysV1 =
            serde_json::from_slice(&decrypted).map_err(|e| CoreError::Keyring(e.to_string()))?;
        let mentor_slice: [u8; 32] = stored
            .mentor_secret
            .as_slice()
            .try_into()
            .map_err(|_| CoreError::Crypto("Invalid mentor key length".into()))?;
        Ok(Self {
            mentor_keypair: SigningKey::from_bytes(&mentor_slice),
            storage_path,
        })
    }

    /// Persist the mentor keypair to encrypted storage (`keys.bin`).
    pub fn save(&self) -> Result<()> {
        let stored = StoredKeysV2 {
            mentor_secret: self.mentor_keypair.to_bytes().to_vec(),
        };
        let serialized = serde_json::to_vec(&stored)?;
        let encrypted = dpapi_encrypt(&serialized)?;
        let keys_file = self.storage_path.join("keys.bin");
        std::fs::create_dir_all(&self.storage_path)?;
        std::fs::write(keys_file, encrypted)?;
        Ok(())
    }

    /// Sign arbitrary data with the mentor's Ed25519 private key.
    pub fn sign_with_mentor(&self, data: &[u8]) -> Signature {
        self.mentor_keypair.sign(data)
    }

    /// Verify a signature against a public key and the original data.
    pub fn verify_signature(pubkey: &VerifyingKey, data: &[u8], signature: &Signature) -> bool {
        pubkey.verify(data, signature).is_ok()
    }
}

impl Keyring {
    /// Encrypt raw bytes via DPAPI (Windows) or plaintext fallback.
    pub fn encrypt_bytes(data: &[u8]) -> Result<Vec<u8>> {
        dpapi_encrypt(data)
    }

    /// Decrypt raw bytes previously encrypted via [`Self::encrypt_bytes`].
    pub fn decrypt_bytes(data: &[u8]) -> Result<Vec<u8>> {
        dpapi_decrypt(data)
    }
}

// ============================================================================
// Master Key Generation (for Hive identity signing)
// ============================================================================

/// Result of generating a new Hive master signing key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterKeyResult {
    pub master_key_path: PathBuf,
}

/// Generate a new Ed25519 master key for the Hive and persist it (DPAPI-encrypted).
///
/// The master key signs per-agent public keys so agents can prove membership
/// in the same Hive installation.
pub fn generate_master_key(data_dir: &Path) -> Result<MasterKeyResult> {
    let signing_key = SigningKey::generate(&mut OsRng);
    std::fs::create_dir_all(data_dir)?;
    let master_key_path = data_dir.join("master.key");
    let stored = MasterKeyStored {
        secret: signing_key.to_bytes().to_vec(),
    };
    let serialized = serde_json::to_vec(&stored)?;
    let encrypted = dpapi_encrypt(&serialized)?;
    std::fs::write(&master_key_path, encrypted)?;
    Ok(MasterKeyResult { master_key_path })
}

/// Load an existing master key from its encrypted file on disk.
pub fn load_master_key(path: &Path) -> Result<SigningKey> {
    let encrypted = std::fs::read(path)?;
    let decrypted = dpapi_decrypt(&encrypted)?;
    let stored: MasterKeyStored =
        serde_json::from_slice(&decrypted).map_err(|e| CoreError::Keyring(e.to_string()))?;
    let key_bytes: [u8; 32] = stored
        .secret
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::Crypto("Invalid master key length".into()))?;
    Ok(SigningKey::from_bytes(&key_bytes))
}

/// Sign an agent's public key with the Hive master key (proves Hive membership).
pub fn sign_agent_key(master_key: &SigningKey, agent_pubkey: &VerifyingKey) -> Vec<u8> {
    let signature = master_key.sign(agent_pubkey.as_bytes());
    signature.to_bytes().to_vec()
}

/// Verify that an agent's public key was signed by the Hive master key.
pub fn verify_agent_signature(
    master_pubkey: &VerifyingKey,
    agent_pubkey: &VerifyingKey,
    signature_bytes: &[u8],
) -> bool {
    let Ok(signature) = Signature::from_slice(signature_bytes) else {
        return false;
    };
    master_pubkey
        .verify(agent_pubkey.as_bytes(), &signature)
        .is_ok()
}

/// High-level function to sign an agent's pubkey with the Hive master key and
/// write the lineage signature file. Called during Darkness stage.
///
/// Returns Ok(true) if lineage was signed, Ok(false) if master key doesn't exist.
pub fn sign_agent_lineage(
    master_key_path: &Path,
    agent_pubkey_path: &Path,
    lineage_sig_path: &Path,
) -> Result<bool> {
    if !master_key_path.exists() || !agent_pubkey_path.exists() {
        return Ok(false);
    }
    let master_key = load_master_key(master_key_path)?;
    let pubkey_bytes = std::fs::read(agent_pubkey_path)?;
    let pubkey_arr: [u8; 32] = pubkey_bytes
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::Crypto("Invalid agent pubkey length".into()))?;
    let agent_verifying_key = VerifyingKey::from_bytes(&pubkey_arr)
        .map_err(|e| CoreError::Crypto(format!("Invalid agent pubkey: {}", e)))?;

    let lineage_sig = sign_agent_key(&master_key, &agent_verifying_key);
    let lineage_data = serde_json::json!({
        "hive_master_pubkey": BASE64.encode(master_key.verifying_key().to_bytes()),
        "agent_pubkey": BASE64.encode(agent_verifying_key.to_bytes()),
        "signature": BASE64.encode(&lineage_sig),
        "signed_at": chrono::Utc::now().to_rfc3339(),
    });
    let json = serde_json::to_string_pretty(&lineage_data)?;
    std::fs::write(lineage_sig_path, json)?;
    Ok(true)
}

/// Verify an agent's Hive lineage from the signature file.
///
/// Returns Ok(true) if the lineage is valid, Ok(false) if files are missing.
pub fn verify_agent_lineage(
    master_key_path: &Path,
    agent_pubkey_path: &Path,
    lineage_sig_path: &Path,
) -> Result<bool> {
    if !master_key_path.exists() || !agent_pubkey_path.exists() || !lineage_sig_path.exists() {
        return Ok(false);
    }

    let master_key = load_master_key(master_key_path)?;
    let master_pubkey = master_key.verifying_key();

    let pubkey_bytes = std::fs::read(agent_pubkey_path)?;
    let pubkey_arr: [u8; 32] = pubkey_bytes
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::Crypto("Invalid agent pubkey length".into()))?;
    let agent_verifying_key = VerifyingKey::from_bytes(&pubkey_arr)
        .map_err(|e| CoreError::Crypto(format!("Invalid agent pubkey: {}", e)))?;

    let lineage_json = std::fs::read_to_string(lineage_sig_path)?;
    let lineage: serde_json::Value = serde_json::from_str(&lineage_json)?;
    let sig_b64 = lineage
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CoreError::Crypto("Missing signature in lineage file".into()))?;
    let sig_bytes = BASE64
        .decode(sig_b64)
        .map_err(|e| CoreError::Crypto(format!("Invalid signature base64: {}", e)))?;

    Ok(verify_agent_signature(
        &master_pubkey,
        &agent_verifying_key,
        &sig_bytes,
    ))
}

#[derive(Serialize, Deserialize)]
struct MasterKeyStored {
    secret: Vec<u8>,
}

// ============================================================================
// External Signing Keypair Generation (for constitutional document signing)
// ============================================================================

/// Result of generating an external Ed25519 keypair for constitutional document signing.
///
/// The private key is returned as base64 and shown to the user exactly once.
/// The public key is persisted to `external_pubkey.bin` for later verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalKeypairResult {
    pub private_key_base64: String,
    pub public_key_path: PathBuf,
}

/// Metadata stored alongside a signed constitutional document (`.sig` file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureMetadata {
    pub signature: String,
    pub tier: DocumentTier,
    pub signed_at: chrono::DateTime<chrono::Utc>,
}

/// Generate an Ed25519 keypair for constitutional document signing (Darkness stage).
///
/// Persists the public key to `external_pubkey.bin` and returns the private key
/// as base64 so the user can save it. The private key is never stored on disk.
pub fn generate_external_keypair(data_dir: &Path) -> Result<ExternalKeypairResult> {
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    std::fs::create_dir_all(data_dir)?;
    let pubkey_path = data_dir.join("external_pubkey.bin");
    std::fs::write(&pubkey_path, verifying_key.to_bytes())?;
    let private_key_base64 = BASE64.encode(signing_key.to_bytes());
    Ok(ExternalKeypairResult {
        private_key_base64,
        public_key_path: pubkey_path,
    })
}

/// Sign a single document using the format `{doc_name}|{tier}|{content}`.
pub fn sign_document(
    signing_key: &SigningKey,
    doc_name: &str,
    content: &str,
    tier: DocumentTier,
) -> SignatureMetadata {
    let signable = format!("{}|{}|{}", doc_name, tier.as_str(), content);
    let signature = signing_key.sign(signable.as_bytes());
    SignatureMetadata {
        signature: BASE64.encode(signature.to_bytes()),
        tier,
        signed_at: chrono::Utc::now(),
    }
}

/// Sign all three constitutional documents (soul.md, ethics.md, instincts.md)
/// and write their `.sig` files. Called during the Emergence stage.
pub fn sign_constitutional_documents(signing_key: &SigningKey, docs_dir: &Path) -> Result<()> {
    let docs = ["soul.md", "ethics.md", "instincts.md"];
    for doc_name in docs {
        let doc_path = docs_dir.join(doc_name);
        if !doc_path.exists() {
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Constitutional document not found: {}", doc_path.display()),
            )));
        }
        let content = std::fs::read_to_string(&doc_path)?;
        let sig_meta = sign_document(
            signing_key,
            doc_name,
            &content,
            DocumentTier::Constitutional,
        );
        let sig_path = docs_dir.join(format!("{}.sig", doc_name));
        let json = serde_json::to_string_pretty(&sig_meta)?;
        std::fs::write(&sig_path, json)?;
    }
    Ok(())
}

// ============================================================================
// Signing Key Persistence (encrypted to disk for birth recovery across restarts)
// ============================================================================

const SIGNING_KEY_FILENAME: &str = "signing.key";

/// Persist a signing key to disk (encrypted via DPAPI on Windows, plaintext on Linux/Docker).
/// Used to survive server restarts during the birth process (Darkness → Emergence).
pub fn persist_signing_key(data_dir: &Path, key_bytes: &[u8]) -> Result<()> {
    let path = data_dir.join(SIGNING_KEY_FILENAME);
    crate::encrypted_storage::write_encrypted(&path, key_bytes)
}

/// Load a persisted signing key from disk. Returns None if the file doesn't exist.
pub fn load_signing_key(data_dir: &Path) -> Result<Option<Vec<u8>>> {
    let path = data_dir.join(SIGNING_KEY_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = crate::encrypted_storage::read_encrypted(&path)?;
    Ok(Some(bytes))
}

/// Delete the persisted signing key file (called after Emergence completes).
pub fn delete_signing_key(data_dir: &Path) -> Result<()> {
    let path = data_dir.join(SIGNING_KEY_FILENAME);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Parse a base64-encoded Ed25519 private key string into a [`SigningKey`].
pub fn parse_private_key(base64_key: &str) -> Result<SigningKey> {
    let bytes = BASE64
        .decode(base64_key)
        .map_err(|e| CoreError::Crypto(format!("Invalid base64 private key: {}", e)))?;
    let key_bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::Crypto("Private key must be exactly 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&key_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_persist_load_delete_signing_key() {
        let tmp = std::env::temp_dir().join("orion_keyring_persist_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let key_bytes = [42u8; 32];
        persist_signing_key(&tmp, &key_bytes).unwrap();
        assert!(tmp.join(SIGNING_KEY_FILENAME).exists());

        let loaded = load_signing_key(&tmp).unwrap();
        assert_eq!(loaded.unwrap(), key_bytes.to_vec());

        delete_signing_key(&tmp).unwrap();
        assert!(!tmp.join(SIGNING_KEY_FILENAME).exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_signing_key_nonexistent() {
        let tmp = std::env::temp_dir().join("orion_keyring_nokey_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let loaded = load_signing_key(&tmp).unwrap();
        assert!(loaded.is_none());

        let _ = fs::remove_dir_all(&tmp);
    }
}
