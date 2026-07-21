//! AES-256-GCM encryption helpers for storing API keys and other sensitive
//! credentials in the database. The encryption key is derived from a
//! passphrase (the `DECKWATCH_ENCRYPTION_KEY` env var) via SHA-256 so we get
//! a fixed-length 256-bit key regardless of what the operator typed.
//!
//! Ciphertext format: `base64(nonce ++ ciphertext)` where nonce is 12 bytes
//! (AES-GCM standard). Decryption splits the first 12 bytes off, uses them
//! as the nonce, and decrypts the remainder.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Encrypt `plaintext` with `key_str` using AES-256-GCM.
///
/// Returns a base64-encoded string containing the 12-byte nonce prepended to
/// the ciphertext. A fresh random nonce is generated on every call so repeated
/// encryptions of the same plaintext produce different outputs.
pub fn encrypt(key_str: &str, plaintext: &str) -> Result<String, String> {
    if key_str.is_empty() {
        return Err("encryption key is empty — set DECKWATCH_ENCRYPTION_KEY".to_string());
    }

    let key = derive_key(key_str);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("failed to build cipher: {e}"))?;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("encryption failed: {e}"))?;

    // nonce ++ ciphertext
    let mut combined = Vec::with_capacity(12 + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);

    Ok(base64::engine::general_purpose::STANDARD.encode(combined))
}

/// Decrypt a value previously produced by [`encrypt`].
///
/// Expects a base64-encoded blob whose first 12 bytes are the nonce and the
/// rest is AES-256-GCM ciphertext.
pub fn decrypt(key_str: &str, encrypted: &str) -> Result<String, String> {
    if key_str.is_empty() {
        return Err("encryption key is empty — set DECKWATCH_ENCRYPTION_KEY".to_string());
    }

    let combined = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| format!("base64 decode failed: {e}"))?;

    if combined.len() < 13 {
        // 12 nonce + at least 1 byte of ciphertext
        return Err("ciphertext too short".to_string());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let key = derive_key(key_str);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("failed to build cipher: {e}"))?;

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("decryption failed (wrong key?): {e}"))?;

    String::from_utf8(plaintext).map_err(|e| format!("decrypted bytes are not valid UTF-8: {e}"))
}

/// Derive a 256-bit key from an arbitrary-length passphrase using SHA-256.
fn derive_key(passphrase: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    let result = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&result);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = "test-key-32-chars-long-exactly!!";
        let plaintext = "sk-ant-api03-super-secret-key";
        let encrypted = encrypt(key, plaintext).expect("encrypt should succeed");
        let decrypted = decrypt(key, &encrypted).expect("decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn different_nonces() {
        let key = "my-encryption-key";
        let plaintext = "same-value";
        let enc1 = encrypt(key, plaintext).unwrap();
        let enc2 = encrypt(key, plaintext).unwrap();
        // Random nonces mean the same plaintext encrypts differently each time.
        assert_ne!(enc1, enc2);
        // But both decrypt back to the same value.
        assert_eq!(decrypt(key, &enc1).unwrap(), plaintext);
        assert_eq!(decrypt(key, &enc2).unwrap(), plaintext);
    }

    #[test]
    fn wrong_key_fails() {
        let enc = encrypt("key-a", "secret").unwrap();
        let result = decrypt("key-b", &enc);
        assert!(result.is_err());
    }

    #[test]
    fn empty_key_rejected() {
        assert!(encrypt("", "hello").is_err());
        assert!(decrypt("", "anything").is_err());
    }

    #[test]
    fn short_ciphertext_rejected() {
        let result = decrypt("key", "dG9vc2hvcnQ="); // "tooshort" base64
        assert!(result.is_err());
    }

    #[test]
    fn empty_plaintext() {
        let key = "my-key";
        let enc = encrypt(key, "").unwrap();
        let dec = decrypt(key, &enc).unwrap();
        assert_eq!(dec, "");
    }
}
