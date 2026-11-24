//! AES-256-GCM encryption helpers for storing API keys and other sensitive
//! credentials in the database. The encryption key is derived from a
//! passphrase (the `DECKWATCH_ENCRYPTION_KEY` env var) via SHA-256 so we get
//! a fixed-length 256-bit key regardless of what the operator typed.
//!
//! Ciphertext format: `base64(nonce ++ ciphertext)` where nonce is 12 bytes
//! (AES-GCM standard). Decryption splits the first 12 bytes off, uses them
//! as the nonce, and decrypts the remainder.
//!
//! Nonces are 96 bits read from the operating system CSPRNG
//! ([`rand::rngs::SysRng`], i.e. `getrandom(2)` on Linux) on every call. The
//! OS source is used rather than rand's user-space `ThreadRng` because
//! `ThreadRng` lost its fork protection in rand 0.9 and is not reseeded on
//! fork or snapshot/restore, and a repeated nonce under the same key breaks
//! AES-GCM.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::Engine;
use rand::rngs::SysRng;
use rand::TryRng;
use sha2::{Digest, Sha256};

/// Encrypt `plaintext` with `key_str` using AES-256-GCM.
///
/// Returns a base64-encoded string containing the 12-byte nonce prepended to
/// the ciphertext. A fresh random nonce is drawn from the OS CSPRNG on every
/// call so repeated encryptions of the same plaintext produce different
/// outputs; if the OS RNG fails, an error is returned and nothing is encrypted.
pub fn encrypt(key_str: &str, plaintext: &str) -> Result<String, String> {
    if key_str.is_empty() {
        return Err("encryption key is empty — set DECKWATCH_ENCRYPTION_KEY".to_string());
    }

    let key = derive_key(key_str);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("failed to build cipher: {e}"))?;

    let mut nonce_bytes = [0u8; 12];
    SysRng
        .try_fill_bytes(&mut nonce_bytes)
        .map_err(|e| format!("failed to generate nonce: {e}"))?;
    // `From<[u8; 12]>` only compiles when the array length equals the
    // cipher's nonce size, so this is a compile-time 96-bit check.
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
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
    let nonce = Nonce::try_from(nonce_bytes).map_err(|e| format!("invalid nonce: {e}"))?;

    let key = derive_key(key_str);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("failed to build cipher: {e}"))?;

    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
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

    // Known-answer vectors produced by `encrypt` as built from main before
    // the aes-gcm 0.11 / rand 0.10 upgrade (aes-gcm 0.10.3, nonce from rand
    // 0.8 `thread_rng`), and independently decrypted with Python
    // `cryptography` (OpenSSL). Values already stored in deployed databases
    // have exactly this shape, so they must keep decrypting.
    const KAT_PASSPHRASE: &str = "deckwatch-kat-passphrase";
    const KAT_VECTORS: &[(&str, &str)] = &[
        (
            "sk-ant-api03-deckwatch-known-answer-vector",
            "cXOrzv1camURy3480aIfiO4ncbwGOJVqKIo6IxbU1yTQgns7KjLYtGddqPdy/J6dP4kUtO/aZCzSL4DpqcT+iXoN5nZ5bA==",
        ),
        ("", "xrBnfSKDcZwBk2VyyUxpvgx7Y/waNo+sMpUpTg=="),
        (
            "ghp_\u{e9}t\u{e9}-unicode-\u{1f512}",
            "wqy/a8hzJCHtJPtxDgFB2mzakbFZuvKmMCypoG6W09IQU637qWWt9fuyFpBLoSZmy5g=",
        ),
    ];

    #[test]
    fn decrypts_values_encrypted_by_previous_release() {
        for (plaintext, encrypted) in KAT_VECTORS {
            assert_eq!(decrypt(KAT_PASSPHRASE, encrypted).unwrap(), *plaintext);
        }
    }

    #[test]
    fn fixed_nonce_output_matches_previous_release() {
        // Same key derivation and `nonce ++ ciphertext ++ tag` layout as
        // `encrypt`, with the nonce pinned so the output is deterministic.
        // The expected strings were produced with aes-gcm 0.10.3 (and match
        // OpenSSL), so this proves 0.11 emits byte-identical ciphertext.
        let nonce_bytes: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let cipher = Aes256Gcm::new_from_slice(&derive_key(KAT_PASSPHRASE)).unwrap();
        for (plaintext, expected) in [
            (
                "sk-ant-api03-deckwatch-known-answer-vector",
                "AAECAwQFBgcICQoLH7kYyPnv7VZOP9yDuf6wea6CAhx7Ne9UZEBp1oQJkayTAxp8DaCL89dZLfYOAdwPYfxQAi9YH3wOEg==",
            ),
            ("", "AAECAwQFBgcICQoLlXXZkJ4nZnVf+utnLrpP2g=="),
        ] {
            let ciphertext = cipher
                .encrypt(&Nonce::from(nonce_bytes), plaintext.as_bytes())
                .unwrap();
            let mut combined = nonce_bytes.to_vec();
            combined.extend_from_slice(&ciphertext);
            assert_eq!(
                base64::engine::general_purpose::STANDARD.encode(&combined),
                expected
            );
            assert_eq!(decrypt(KAT_PASSPHRASE, expected).unwrap(), plaintext);
        }
    }

    #[test]
    fn encrypt_output_is_nonce_then_ciphertext_and_tag() {
        // Pins the layout the previous release's `decrypt` expects, so a
        // rollback can still read values written by this build: a 12-byte
        // (96-bit) nonce, then the ciphertext, then the 16-byte GCM tag.
        let plaintext = "sk-ant-api03-layout-check";
        let cipher = Aes256Gcm::new_from_slice(&derive_key(KAT_PASSPHRASE)).unwrap();
        let mut nonces = std::collections::HashSet::new();
        for _ in 0..256 {
            let raw = base64::engine::general_purpose::STANDARD
                .decode(encrypt(KAT_PASSPHRASE, plaintext).unwrap())
                .unwrap();
            assert_eq!(raw.len(), 12 + plaintext.len() + 16);
            let (nonce, rest) = raw.split_at(12);
            let decrypted = cipher
                .decrypt(&Nonce::try_from(nonce).unwrap(), rest)
                .unwrap();
            assert_eq!(decrypted, plaintext.as_bytes());
            assert!(nonces.insert(nonce.to_vec()), "nonce repeated");
        }
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let (_, encrypted) = KAT_VECTORS[0];
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(encrypted)
            .unwrap();
        // First nonce byte, first ciphertext byte, last tag byte.
        for i in [0, 12, raw.len() - 1] {
            raw[i] ^= 0x01;
            let tampered = base64::engine::general_purpose::STANDARD.encode(&raw);
            assert!(decrypt(KAT_PASSPHRASE, &tampered).is_err(), "byte {i}");
            raw[i] ^= 0x01;
        }
        // Sanity: the untampered bytes still decrypt.
        let restored = base64::engine::general_purpose::STANDARD.encode(&raw);
        assert!(decrypt(KAT_PASSPHRASE, &restored).is_ok());
    }
}
