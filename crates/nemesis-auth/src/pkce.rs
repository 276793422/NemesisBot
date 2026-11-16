//! PKCE (Proof Key for Code Exchange) implementation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
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
    // BUG-20261117-049：verifier 是防授权码拦截的凭据（RFC 7636 要求
    // 高熵加密强随机），thread_rng 是非加密 PRNG——OsRng。
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
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
