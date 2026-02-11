//! PKCE (RFC 7636) for OAuth2 public clients.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::Digest;

/// Generate a code_verifier (43–128 chars, unreserved).
pub fn code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64_encode_unreserved(&bytes)
}

/// Code challenge S256: BASE64URL(SHA256(verifier)).
pub fn code_challenge_s256(verifier: &str) -> String {
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// PKCE pair for one OAuth2 flow (use for both Gmail and Outlook).
#[derive(Debug, Clone)]
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
}

impl PkcePair {
    pub fn new() -> Self {
        let code_verifier = code_verifier();
        let code_challenge = code_challenge_s256(&code_verifier);
        Self {
            code_verifier,
            code_challenge,
        }
    }
}

impl Default for PkcePair {
    fn default() -> Self {
        Self::new()
    }
}

fn base64_encode_unreserved(bytes: &[u8]) -> String {
    let s = URL_SAFE_NO_PAD.encode(bytes);
    s.trim_end_matches('=').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verifier_length() {
        let v = code_verifier();
        assert!(v.len() >= 43 && v.len() <= 128);
    }

    #[test]
    fn test_challenge_deterministic() {
        let v = "test_verifier_43_chars________________________";
        let c1 = code_challenge_s256(v);
        let c2 = code_challenge_s256(v);
        assert_eq!(c1, c2);
    }
}
