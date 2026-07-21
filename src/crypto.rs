//! AES-256-GCM encryption for at-rest credential storage.
//!
//! API keys stored in the database are encrypted with a key derived from
//! `DECKWATCH_ENCRYPTION_KEY` (a random string generated once by the Helm
//! chart). The ciphertext format is `base64(nonce || ciphertext || tag)` —
//! a single opaque string safe for JSON / SQL text columns.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use sha2::{Digest, Sha256};

/// Derive a 32-byte AES key from the encryption key string using SHA-256.
fn derive_key(key_str: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(key_str.as_bytes());
    hasher.finalize().into()
}

/// Encrypt a plaintext string.
///
/// Returns a base64-encoded blob containing a random 12-byte nonce
/// prepended to the AES-256-GCM ciphertext (which includes the 16-byte
/// authentication tag).
pub fn encrypt(key_str: &str, plaintext: &str) -> Result<String, String> {
    let key = derive_key(key_str);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;

    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);

    Ok(base64::engine::general_purpose::STANDARD.encode(&combined))
}

/// Decrypt a base64-encoded ciphertext produced by [`encrypt`].
pub fn decrypt(key_str: &str, encrypted: &str) -> Result<String, String> {
    let key = derive_key(key_str);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;

    let combined = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| e.to_string())?;

    if combined.len() < 12 {
        return Err("ciphertext too short".to_string());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "decryption failed - wrong key or corrupted data".to_string())?;

    String::from_utf8(plaintext).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = "test-encryption-key-1234567890ab";
        let plaintext = "sk-ant-api03-secret-key-value";
        let encrypted = encrypt(key, plaintext).expect("encrypt failed");
        let decrypted = decrypt(key, &encrypted).expect("decrypt failed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let key = "correct-key-abcdefghijklmnopqrst";
        let wrong = "wrong-key-zyxwvutsrqponmlkjihgf";
        let plaintext = "super-secret";
        let encrypted = encrypt(key, plaintext).expect("encrypt failed");
        let result = decrypt(wrong, &encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn empty_plaintext() {
        let key = "key-for-empty-plaintext-test1234";
        let encrypted = encrypt(key, "").expect("encrypt failed");
        let decrypted = decrypt(key, &encrypted).expect("decrypt failed");
        assert_eq!(decrypted, "");
    }

    #[test]
    fn different_nonces_produce_different_ciphertexts() {
        let key = "key-for-nonce-uniqueness-test123";
        let plaintext = "same-plaintext";
        let enc1 = encrypt(key, plaintext).expect("encrypt 1 failed");
        let enc2 = encrypt(key, plaintext).expect("encrypt 2 failed");
        // Overwhelmingly likely to differ due to random nonces.
        assert_ne!(enc1, enc2);
        // But both decrypt to the same value.
        assert_eq!(decrypt(key, &enc1).unwrap(), decrypt(key, &enc2).unwrap());
    }

    #[test]
    fn ciphertext_too_short() {
        let key = "key-for-short-ciphertext-test123";
        let short = base64::engine::general_purpose::STANDARD.encode(&[0u8; 5]);
        let result = decrypt(key, &short);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too short"));
    }

    #[test]
    fn invalid_base64() {
        let key = "key-for-invalid-base64-test12345";
        let result = decrypt(key, "not-valid-base64!!!");
        assert!(result.is_err());
    }
}
