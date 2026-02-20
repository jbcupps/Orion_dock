//! Platform-specific DPAPI encrypt/decrypt helpers.
//!
//! On Windows, uses CryptProtectData/CryptUnprotectData.
//! On other platforms, uses plaintext passthrough (dev only).

#[cfg(windows)]
use crate::error::Result;

#[cfg(windows)]
use crate::error::CoreError;

#[cfg(windows)]
pub fn dpapi_encrypt(data: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::*;
    use windows::Win32::Security::Cryptography::*;

    unsafe {
        let mut input_data = data.to_vec();
        let input = CRYPT_INTEGER_BLOB {
            cbData: input_data.len() as u32,
            pbData: input_data.as_mut_ptr(),
        };

        let mut output = CRYPT_INTEGER_BLOB::default();

        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            Default::default(),
            &mut output,
        )
        .map_err(|e| CoreError::Crypto(format!("DPAPI encrypt failed: {}", e)))?;

        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));

        Ok(result)
    }
}

#[cfg(windows)]
pub fn dpapi_decrypt(data: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::*;
    use windows::Win32::Security::Cryptography::*;

    unsafe {
        let mut input_data = data.to_vec();
        let input = CRYPT_INTEGER_BLOB {
            cbData: input_data.len() as u32,
            pbData: input_data.as_mut_ptr(),
        };

        let mut output = CRYPT_INTEGER_BLOB::default();

        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            Default::default(),
            &mut output,
        )
        .map_err(|e| CoreError::Crypto(format!("DPAPI decrypt failed: {}", e)))?;

        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));

        Ok(result)
    }
}

#[cfg(not(windows))]
mod platform_crypto {
    use crate::error::{CoreError, Result};

    /// Version byte prefix for encrypted data.
    const ENCRYPTED_VERSION: u8 = 0x01;
    /// Nonce length for ChaCha20-Poly1305.
    const NONCE_LEN: usize = 12;

    /// Derive a 32-byte key from the `ORION_MASTER_KEY` env var.
    /// Returns `None` if the env var is not set or empty.
    fn derive_key() -> Option<[u8; 32]> {
        let raw = std::env::var("ORION_MASTER_KEY").ok()?;
        if raw.is_empty() {
            return None;
        }
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(raw.as_bytes());
        let mut key = [0u8; 32];
        key.copy_from_slice(&hash);
        Some(key)
    }

    pub fn dpapi_encrypt(data: &[u8]) -> Result<Vec<u8>> {
        let Some(key_bytes) = derive_key() else {
            tracing::warn!(
                "ORION_MASTER_KEY not set — using plaintext storage (dev only). \
                 Set ORION_MASTER_KEY env var for encrypted storage on Linux/macOS."
            );
            return Ok(data.to_vec());
        };

        use chacha20poly1305::{
            aead::{Aead, KeyInit},
            ChaCha20Poly1305, Nonce,
        };

        let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
            .map_err(|e| CoreError::Crypto(format!("cipher init: {}", e)))?;

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::Fill::fill(&mut nonce_bytes, &mut rand::rng());
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, data)
            .map_err(|e| CoreError::Crypto(format!("encrypt: {}", e)))?;

        // Format: [1-byte version][12-byte nonce][ciphertext + auth tag]
        let mut output = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        output.push(ENCRYPTED_VERSION);
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn dpapi_decrypt(data: &[u8]) -> Result<Vec<u8>> {
        // Check if this looks like encrypted data (version byte prefix).
        if data.is_empty() || data[0] != ENCRYPTED_VERSION {
            // Legacy plaintext data or ORION_MASTER_KEY was not set when written.
            if derive_key().is_some() {
                tracing::warn!(
                    "Data appears to be unencrypted (legacy). Returning as-is. \
                     Re-save to encrypt with ORION_MASTER_KEY."
                );
            } else {
                tracing::warn!("ORION_MASTER_KEY not set — reading as plaintext (dev only).");
            }
            return Ok(data.to_vec());
        }

        let Some(key_bytes) = derive_key() else {
            return Err(CoreError::Crypto(
                "Data is encrypted but ORION_MASTER_KEY is not set. \
                 Cannot decrypt without the master key."
                    .into(),
            ));
        };

        if data.len() < 1 + NONCE_LEN {
            return Err(CoreError::Crypto("Encrypted data too short".into()));
        }

        use chacha20poly1305::{
            aead::{Aead, KeyInit},
            ChaCha20Poly1305, Nonce,
        };

        let nonce = Nonce::from_slice(&data[1..1 + NONCE_LEN]);
        let ciphertext = &data[1 + NONCE_LEN..];

        let cipher = ChaCha20Poly1305::new_from_slice(&key_bytes)
            .map_err(|e| CoreError::Crypto(format!("cipher init: {}", e)))?;

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| CoreError::Crypto(format!("decrypt failed: {}", e)))
    }
}

#[cfg(not(windows))]
pub use platform_crypto::{dpapi_decrypt, dpapi_encrypt};

#[cfg(test)]
#[cfg(not(windows))]
mod tests {
    use super::*;
    use crate::test_util::MASTER_KEY_ENV_LOCK;

    #[test]
    fn test_plaintext_fallback_without_master_key() {
        let _guard = MASTER_KEY_ENV_LOCK.lock().unwrap();
        // Ensure env var is unset for this test
        std::env::remove_var("ORION_MASTER_KEY");
        let data = b"hello secrets";
        let encrypted = dpapi_encrypt(data).unwrap();
        // Without key, should be plaintext passthrough
        assert_eq!(encrypted, data);
        let decrypted = dpapi_decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip_with_master_key() {
        let _guard = MASTER_KEY_ENV_LOCK.lock().unwrap();
        std::env::set_var("ORION_MASTER_KEY", "test-key-roundtrip");
        let data = b"sensitive api key sk-abc123";
        let encrypted = dpapi_encrypt(data).unwrap();
        // Encrypted data should NOT equal plaintext
        assert_ne!(encrypted.as_slice(), data);
        // Version byte present
        assert_eq!(encrypted[0], 0x01);
        let decrypted = dpapi_decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, data);
        std::env::remove_var("ORION_MASTER_KEY");
    }

    #[test]
    fn test_version_byte_present() {
        let _guard = MASTER_KEY_ENV_LOCK.lock().unwrap();
        std::env::set_var("ORION_MASTER_KEY", "test-key-version");
        let encrypted = dpapi_encrypt(b"data").unwrap();
        assert_eq!(encrypted[0], 0x01);
        // Must be at least 1 (version) + 12 (nonce) + 16 (tag) + data
        assert!(encrypted.len() >= 1 + 12 + 16 + 4);
        std::env::remove_var("ORION_MASTER_KEY");
    }

    #[test]
    fn test_legacy_plaintext_migration() {
        let _guard = MASTER_KEY_ENV_LOCK.lock().unwrap();
        // Data written without key (plaintext), then read with key set
        std::env::remove_var("ORION_MASTER_KEY");
        let data = b"legacy plaintext data";
        let stored = dpapi_encrypt(data).unwrap();
        assert_eq!(stored, data); // plaintext

        // Now set key and try to decrypt legacy data
        std::env::set_var("ORION_MASTER_KEY", "test-key-migration");
        let decrypted = dpapi_decrypt(&stored).unwrap();
        assert_eq!(decrypted, data); // returns plaintext with migration warning
        std::env::remove_var("ORION_MASTER_KEY");
    }

    #[test]
    fn test_encrypted_without_key_errors() {
        let _guard = MASTER_KEY_ENV_LOCK.lock().unwrap();
        // Encrypt with key
        std::env::set_var("ORION_MASTER_KEY", "test-key-error");
        let encrypted = dpapi_encrypt(b"secret").unwrap();
        // Remove key, try to decrypt
        std::env::remove_var("ORION_MASTER_KEY");
        let result = dpapi_decrypt(&encrypted);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("ORION_MASTER_KEY"));
    }
}
