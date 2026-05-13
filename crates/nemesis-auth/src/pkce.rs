//! PKCE (Proof Key for Code Exchange) implementation.

use rand::Rng;
use sha2::{Sha256, Digest};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

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
mod tests {
    use super::*;

    #[test]
    fn test_generate_pkce() {
        let pkce = generate_pkce();
        assert!(!pkce.code_verifier.is_empty());
        assert!(!pkce.code_challenge.is_empty());
        assert_ne!(pkce.code_verifier, pkce.code_challenge);
    }

    #[test]
    fn test_deterministic_challenge() {
        let pkce1 = generate_pkce();
        let challenge = compute_challenge(&pkce1.code_verifier);
        assert_eq!(challenge, pkce1.code_challenge);
    }

    #[test]
    fn test_pkce_verifier_length() {
        // 32 random bytes -> base64url-no-pad => 43 chars (ceil(32*4/3)=43)
        let pkce = generate_pkce();
        assert_eq!(pkce.code_verifier.len(), 43);
    }

    #[test]
    fn test_pkce_challenge_length() {
        // SHA256 produces 32 bytes -> base64url-no-pad => 43 chars
        let pkce = generate_pkce();
        assert_eq!(pkce.code_challenge.len(), 43);
    }

    #[test]
    fn test_pkce_uniqueness() {
        let mut verifiers = std::collections::HashSet::new();
        for _ in 0..100 {
            let pkce = generate_pkce();
            verifiers.insert(pkce.code_verifier.clone());
        }
        // All 100 verifiers should be unique (statistically guaranteed with 256 bits)
        assert_eq!(verifiers.len(), 100);
    }

    #[test]
    fn test_pkce_verifier_is_url_safe_base64() {
        let pkce = generate_pkce();
        for c in pkce.code_verifier.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "unexpected char '{}' in verifier",
                c
            );
        }
    }

    #[test]
    fn test_pkce_challenge_is_url_safe_base64() {
        let pkce = generate_pkce();
        for c in pkce.code_challenge.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '-' || c == '_',
                "unexpected char '{}' in challenge",
                c
            );
        }
    }

    #[test]
    fn test_compute_challenge_known_input() {
        // Verify that the challenge is SHA256(verifier) base64url-no-pad
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let verifier = "test-verifier-value";
        let challenge = compute_challenge(verifier);
        // Manually compute expected
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let expected = URL_SAFE_NO_PAD.encode(hash);
        assert_eq!(challenge, expected);
    }

    #[test]
    fn test_pkce_codes_debug_clone() {
        let pkce = generate_pkce();
        let cloned = pkce.clone();
        assert_eq!(pkce.code_verifier, cloned.code_verifier);
        assert_eq!(pkce.code_challenge, cloned.code_challenge);
        // Debug trait should work
        let debug_str = format!("{:?}", pkce);
        assert!(debug_str.contains("code_verifier"));
        assert!(debug_str.contains("code_challenge"));
    }

    #[test]
    fn test_generate_code_verifier_is_not_empty() {
        let verifier = generate_code_verifier();
        assert!(!verifier.is_empty());
    }
}
