//! Cryptographic helpers for discovery message authentication and encryption.
//!
//! Provides HMAC-based message signing and AES-256-GCM encryption for cluster
//! discovery messages. Encryption prevents unauthorized nodes from reading or
//! forging discovery broadcasts on the LAN.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, AeadCore, Nonce};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/// Derive a 32-byte AES-256 key from a token string using SHA-256.
///
/// Mirrors Go's `DeriveKey(token string) []byte`. Exported so the cluster
/// module can pre-derive the key before constructing the discovery service.
pub fn derive_key(token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let hash = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

// ---------------------------------------------------------------------------
// AES-256-GCM encrypt / decrypt
// ---------------------------------------------------------------------------

/// Encrypt plaintext using AES-256-GCM.
///
/// Output format: `[12-byte nonce] + [ciphertext + 16-byte GCM tag]`.
/// Mirrors Go's `encryptData(key, plaintext)`.
pub fn encrypt_data(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, aes_gcm::Error> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| aes_gcm::Error)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext)?;
    // Prepend nonce to ciphertext
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt AES-256-GCM encrypted data.
///
/// Expected input format: `[12-byte nonce] + [ciphertext + 16-byte GCM tag]`.
/// Mirrors Go's `decryptData(key, data)`.
pub fn decrypt_data(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, aes_gcm::Error> {
    if data.len() < 12 {
        return Err(aes_gcm::Error);
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| aes_gcm::Error)?;
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext)
}

// ---------------------------------------------------------------------------
// HMAC-based CryptoService (existing, kept for backward compat)
// ---------------------------------------------------------------------------

/// Simple HMAC-based crypto service for discovery messages.
pub struct CryptoService {
    secret: String,
}

impl CryptoService {
    /// Create a new crypto service with the given shared secret.
    pub fn new(secret: String) -> Self {
        Self { secret }
    }

    /// Create a crypto service with no secret (no authentication).
    pub fn no_auth() -> Self {
        Self {
            secret: String::new(),
        }
    }

    /// Sign a message payload and return the HMAC hex digest.
    pub fn sign(&self, payload: &[u8]) -> String {
        if self.secret.is_empty() {
            return String::new();
        }
        let mut hasher = Sha256::new();
        hasher.update(self.secret.as_bytes());
        hasher.update(payload);
        format!("{:x}", hasher.finalize())
    }

    /// Verify a message signature.
    pub fn verify(&self, payload: &[u8], signature: &str) -> bool {
        if self.secret.is_empty() {
            return true; // No auth mode
        }
        let expected = self.sign(payload);
        expected == signature
    }

    /// Generate a node ID hash from a node name and address.
    pub fn node_id_hash(name: &str, address: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{}|{}", name, address).as_bytes());
        let result = format!("{:x}", hasher.finalize());
        result[..16].to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify() {
        let crypto = CryptoService::new("test-secret".into());
        let payload = b"hello world";
        let sig = crypto.sign(payload);

        assert!(crypto.verify(payload, &sig));
        assert!(!crypto.verify(b"tampered", &sig));
    }

    #[test]
    fn test_no_auth_mode() {
        let crypto = CryptoService::no_auth();
        assert!(crypto.verify(b"anything", "any-signature"));
        assert!(crypto.sign(b"test").is_empty());
    }

    #[test]
    fn test_node_id_hash_deterministic() {
        let hash1 = CryptoService::node_id_hash("node-1", "10.0.0.1:9000");
        let hash2 = CryptoService::node_id_hash("node-1", "10.0.0.1:9000");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 16);
    }

    #[test]
    fn test_node_id_hash_different_inputs() {
        let hash1 = CryptoService::node_id_hash("node-1", "10.0.0.1:9000");
        let hash2 = CryptoService::node_id_hash("node-2", "10.0.0.1:9000");
        assert_ne!(hash1, hash2);
    }

    // --- AES-256-GCM tests ---

    #[test]
    fn test_derive_key_deterministic() {
        let k1 = derive_key("my-token");
        let k2 = derive_key("my-token");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_derive_key_different_tokens() {
        let k1 = derive_key("token-a");
        let k2 = derive_key("token-b");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = derive_key("test-secret");
        let plaintext = b"hello discovery world";
        let encrypted = encrypt_data(&key, plaintext).unwrap();
        let decrypted = decrypt_data(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_produces_different_ciphertexts() {
        let key = derive_key("test-secret");
        let plaintext = b"same data";
        let e1 = encrypt_data(&key, plaintext).unwrap();
        let e2 = encrypt_data(&key, plaintext).unwrap();
        // Random nonces → different ciphertext
        assert_ne!(e1, e2);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key1 = derive_key("correct-key");
        let key2 = derive_key("wrong-key");
        let encrypted = encrypt_data(&key1, b"secret").unwrap();
        assert!(decrypt_data(&key2, &encrypted).is_err());
    }

    #[test]
    fn test_decrypt_too_short_fails() {
        let key = derive_key("any");
        assert!(decrypt_data(&key, b"short").is_err());
    }

    #[test]
    fn test_decrypt_tampered_fails() {
        let key = derive_key("test");
        let mut encrypted = encrypt_data(&key, b"data").unwrap();
        // Flip a byte in the ciphertext
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xff;
        assert!(decrypt_data(&key, &encrypted).is_err());
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let key = derive_key("test");
        let encrypted = encrypt_data(&key, b"").unwrap();
        let decrypted = decrypt_data(&key, &encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    #[test]
    fn test_encrypt_large_payload() {
        let key = derive_key("test");
        let plaintext = vec![0xAB_u8; 4096];
        let encrypted = encrypt_data(&key, &plaintext).unwrap();
        let decrypted = decrypt_data(&key, &encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
