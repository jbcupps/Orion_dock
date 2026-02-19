//! Skill signing and signature verification using Ed25519.
//!
//! Reuses the Ed25519 infrastructure from `orion-core::keyring`. Each skill can
//! be signed by a mentor (Verified tier) or by the agent itself (AgentBuilt tier).
//! Unsigned skills default to Untrusted.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Who signed a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignerKind {
    Mentor,
    Agent,
    Unknown,
}

/// Signature metadata stored alongside a skill manifest as `skill.sig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSignature {
    pub skill_id: String,
    pub signer: SignerKind,
    pub signature_base64: String,
    pub signed_at: chrono::DateTime<chrono::Utc>,
}

/// Compute SHA-256 hash of a skill manifest's TOML content.
pub fn hash_manifest(manifest_content: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(manifest_content.as_bytes());
    hasher.finalize().to_vec()
}

/// Sign a skill manifest.
///
/// Signature format: `skill|{skill_id}|{manifest_sha256_hex}` → Ed25519 sign → base64.
pub fn sign_skill(
    signing_key: &SigningKey,
    skill_id: &str,
    manifest_hash: &[u8],
    signer: SignerKind,
) -> SkillSignature {
    let hash_hex = hex::encode(manifest_hash);
    let signable = format!("skill|{}|{}", skill_id, hash_hex);
    let signature = signing_key.sign(signable.as_bytes());
    SkillSignature {
        skill_id: skill_id.to_string(),
        signer,
        signature_base64: BASE64.encode(signature.to_bytes()),
        signed_at: chrono::Utc::now(),
    }
}

/// Verify a skill signature against a public key.
pub fn verify_skill_signature(
    pubkey: &VerifyingKey,
    sig: &SkillSignature,
    manifest_hash: &[u8],
) -> bool {
    let hash_hex = hex::encode(manifest_hash);
    let signable = format!("skill|{}|{}", sig.skill_id, hash_hex);
    let Ok(sig_bytes) = BASE64.decode(&sig.signature_base64) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(&sig_bytes) else {
        return false;
    };
    pubkey.verify(signable.as_bytes(), &signature).is_ok()
}

/// Load a skill signature from a `skill.sig` file.
pub fn load_skill_signature(sig_path: &Path) -> Result<SkillSignature, String> {
    let content = std::fs::read_to_string(sig_path).map_err(|e| e.to_string())?;
    serde_json::from_str(&content).map_err(|e| e.to_string())
}

/// Save a skill signature to a `skill.sig` file.
pub fn save_skill_signature(sig_path: &Path, sig: &SkillSignature) -> Result<(), String> {
    let json = serde_json::to_string_pretty(sig).map_err(|e| e.to_string())?;
    std::fs::write(sig_path, json).map_err(|e| e.to_string())
}

/// Hex-encode bytes (used for manifest hash in signature format).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::Rng;

    #[test]
    fn test_sign_and_verify() {
        let key = SigningKey::from_bytes(&rand::rng().random::<[u8; 32]>());
        let pubkey = key.verifying_key();

        let manifest_content = r#"[skill]
id = "com.test.hello"
name = "Hello"
version = "1.0"
description = "Test skill"
"#;
        let hash = hash_manifest(manifest_content);
        let sig = sign_skill(&key, "com.test.hello", &hash, SignerKind::Mentor);

        assert!(verify_skill_signature(&pubkey, &sig, &hash));
        assert_eq!(sig.skill_id, "com.test.hello");
        assert_eq!(sig.signer, SignerKind::Mentor);
    }

    #[test]
    fn test_wrong_key_rejects() {
        let key1 = SigningKey::from_bytes(&rand::rng().random::<[u8; 32]>());
        let key2 = SigningKey::from_bytes(&rand::rng().random::<[u8; 32]>());
        let pubkey2 = key2.verifying_key();

        let hash = hash_manifest("test content");
        let sig = sign_skill(&key1, "com.test.hello", &hash, SignerKind::Agent);

        assert!(!verify_skill_signature(&pubkey2, &sig, &hash));
    }

    #[test]
    fn test_tampered_hash_rejects() {
        let key = SigningKey::from_bytes(&rand::rng().random::<[u8; 32]>());
        let pubkey = key.verifying_key();

        let hash = hash_manifest("original content");
        let sig = sign_skill(&key, "com.test.hello", &hash, SignerKind::Mentor);

        let tampered_hash = hash_manifest("tampered content");
        assert!(!verify_skill_signature(&pubkey, &sig, &tampered_hash));
    }

    #[test]
    fn test_save_and_load_signature() {
        let key = SigningKey::from_bytes(&rand::rng().random::<[u8; 32]>());
        let hash = hash_manifest("test");
        let sig = sign_skill(&key, "com.test.roundtrip", &hash, SignerKind::Agent);

        let tmp = std::env::temp_dir().join("orion_skill_sig_test.sig");
        save_skill_signature(&tmp, &sig).unwrap();
        let loaded = load_skill_signature(&tmp).unwrap();

        assert_eq!(loaded.skill_id, sig.skill_id);
        assert_eq!(loaded.signature_base64, sig.signature_base64);
        assert_eq!(loaded.signer, sig.signer);

        let _ = std::fs::remove_file(&tmp);
    }
}
