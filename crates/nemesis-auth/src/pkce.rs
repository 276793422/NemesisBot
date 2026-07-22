//! PKCE (Proof Key for Code Exchange) implementation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::Rng;
use sha2::{Digest, Sha256};

/// PKCE code pair.
#[derive(Debug, Clone)]
pub struct PkceCodes {
    pub code_verifier: String,
    pub code_challenge: String,
}

/// Generate a new PKCE code pair.
pub fn generate_pkce() -> PkceCodes {
    let verifier = generate_code_verifier();
    let challenge = compute_challenge(&verifier);
    PkceCodes {
        code_verifier: verifier,
        code_challenge: challenge,
    }
}

fn generate_code_verifier() -> String {
    let mut rng = rand::thread_rng();
    let bytes: [u8; 32] = rng.r#gen();
    URL_SAFE_NO_PAD.encode(bytes)
}

fn compute_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

#[cfg(test)]
mod tests;
