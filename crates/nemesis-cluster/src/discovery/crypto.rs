//! Cryptographic helpers for discovery message authentication and encryption.
//!
//! Provides HMAC-based message signing and AES-256-GCM encryption for cluster
//! discovery messages. Encryption prevents unauthorized nodes from reading or
//! forging discovery broadcasts on the LAN.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Nonce};
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
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| aes_gcm::Error)?;
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
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| aes_gcm::Error)?;
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ciphertext)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
