//! Skill signer - Ed25519 skill signing wrapper.
//!
//! Wraps the nemesis-security::signature module with skill-specific logic
//! for signing, verifying, and generating key pairs for skills.

use std::path::Path;

use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};
use tracing::debug;

use nemesis_security::signature::{
    SignatureVerifier, SkillVerification, TrustLevel, TrustStore, generate_key_pair,
};

use nemesis_types::error::{NemesisError, Result};

/// Metadata for a generated key pair, saved alongside the keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeyMetadata {
    public_key: String,
    fingerprint: String,
    algorithm: String,
}

/// High-level skill signing and verification operations.
///
/// Wraps the security signature package with skill-specific logic,
/// including directory scanning and manifest-based signing.
pub struct SkillSigner {
    verifier: SignatureVerifier,
}

impl SkillSigner {
    /// Create a SkillSigner with an in-memory trust store.
    pub fn new() -> Self {
        Self {
            verifier: SignatureVerifier::new(),
        }
    }

    /// Create a SkillSigner with a file-persisted trust store.
    pub fn with_persistence(config_path: &str) -> Self {
        let verifier = SignatureVerifier::with_persistence(config_path);
        Self { verifier }
    }

    /// Sign all files in a skill directory.
    ///
    /// Walks the skill directory, concatenates all file contents, signs the
    /// combined content with the private key, and writes a `.signature` manifest.
    ///
    /// `key_path` should point to a file containing the hex-encoded Ed25519
    /// private key (64 hex chars = 32 bytes).
    pub fn sign_skill(&self, skill_path: &str, key_path: &str) -> Result<()> {
        let skill_dir = Path::new(skill_path);
        if !skill_dir.exists() || !skill_dir.is_dir() {
            return Err(NemesisError::Validation(format!(
                "skill path is not a directory: {}",
                skill_path
            )));
        }

        // Load the private key.
        let private_key_hex = std::fs::read_to_string(key_path)
            .map_err(|e| NemesisError::Io(e))?
            .trim()
            .to_string();

        // Parse into a SigningKey.
        let pk_bytes = hex_decode(&private_key_hex)
            .map_err(|e| NemesisError::Security(format!("invalid private key: {}", e)))?;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&pk_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_hex = hex_encode(verifying_key.to_bytes().as_ref());

        // Build manifest content (same method used by verify_skill).
        let manifest = self.build_manifest(skill_dir)?;

        // Sign the manifest content using Ed25519.
        let signature_bytes = signing_key.sign(manifest.content.as_bytes());
        let signature_hex = hex_encode(signature_bytes.to_bytes().as_ref());

        // Write .signature file.
        let sig_path = skill_dir.join(".signature");
        let sig_data = serde_json::json!({
            "algorithm": "ed25519",
            "public_key": public_key_hex,
            "signature": signature_hex,
            "files": manifest.files,
        });
        let sig_json =
            serde_json::to_string_pretty(&sig_data).map_err(|e| NemesisError::Serialization(e))?;
        std::fs::write(&sig_path, sig_json).map_err(|e| NemesisError::Io(e))?;

        debug!("Signed skill at {}", skill_path);
        Ok(())
    }

    /// Verify the signature of a skill directory.
    ///
    /// Reads the `.signature` file, verifies the signature against the
    /// directory contents, and checks the trust store for the public key.
    pub fn verify_skill(&self, skill_path: &str) -> Result<SkillVerification> {
        let skill_dir = Path::new(skill_path);
        if !skill_dir.exists() || !skill_dir.is_dir() {
            return Err(NemesisError::Validation(format!(
                "skill path is not a directory: {}",
                skill_path
            )));
        }

        let sig_path = skill_dir.join(".signature");
        if !sig_path.exists() {
            return Ok(SkillVerification {
                valid: false,
                trusted: false,
                trust_level: TrustLevel::Unknown,
                public_key: String::new(),
                error: "no .signature file found".to_string(),
            });
        }

        let sig_data: serde_json::Value = std::fs::read_to_string(&sig_path)
            .map_err(|e| NemesisError::Io(e))
            .and_then(|s| serde_json::from_str(&s).map_err(|e| NemesisError::Serialization(e)))?;

        let public_key = sig_data["public_key"].as_str().unwrap_or("").to_string();
        let signature = sig_data["signature"].as_str().unwrap_or("").to_string();

        // Rebuild manifest content for verification (same method as sign_skill).
        let manifest = self.build_manifest(skill_dir)?;

        Ok(self
            .verifier
            .verify_skill(&manifest.content, &signature, &public_key))
    }

    /// Generate a new Ed25519 key pair and save to output directory.
    ///
    /// Saves `skill_sign.key` (private key) and `skill_sign.pub` (public key).
    /// Also saves `skill_sign.meta.json` with human-readable metadata.
    /// Returns the output directory path.
    pub fn generate_key_pair(output_dir: &str) -> Result<String> {
        let out_path = Path::new(output_dir);
        std::fs::create_dir_all(out_path).map_err(|e| NemesisError::Io(e))?;

        let key_pair = generate_key_pair()
            .map_err(|e| NemesisError::Security(format!("failed to generate key pair: {}", e)))?;

        // Save private key.
        let priv_path = out_path.join("skill_sign.key");
        std::fs::write(&priv_path, &key_pair.private_key).map_err(|e| NemesisError::Io(e))?;

        // Save public key.
        let pub_path = out_path.join("skill_sign.pub");
        std::fs::write(&pub_path, &key_pair.public_key).map_err(|e| NemesisError::Io(e))?;

        // Save metadata.
        let fingerprint = compute_public_key_fingerprint(&key_pair.public_key);
        let metadata = KeyMetadata {
            public_key: key_pair.public_key.clone(),
            fingerprint,
            algorithm: "ed25519".to_string(),
        };
        let meta_path = out_path.join("skill_sign.meta.json");
        let meta_json =
            serde_json::to_string_pretty(&metadata).map_err(|e| NemesisError::Serialization(e))?;
        std::fs::write(&meta_path, meta_json).map_err(|e| NemesisError::Io(e))?;

        debug!("Generated key pair in {}", output_dir);
        Ok(output_dir.to_string())
    }

    /// Borrow the underlying verifier for advanced operations.
    pub fn verifier(&self) -> &SignatureVerifier {
        &self.verifier
    }

    /// Borrow the underlying trust store.
    pub fn trust_store(&self) -> &TrustStore {
        self.verifier.trust_store_ref()
    }

    /// Build a deterministic manifest of all files in a skill directory.
    ///
    /// Walks the directory, collects relative file paths and their SHA-256 hashes,
    /// and produces a combined content string for signing.
    fn build_manifest(&self, skill_dir: &Path) -> std::result::Result<SkillManifest, NemesisError> {
        let mut files = Vec::new();
        let mut combined = String::new();

        Self::walk_dir(skill_dir, skill_dir, &mut files, &mut combined)?;

        files.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(SkillManifest {
            content: combined,
            files,
            public_key_hint: String::new(),
        })
    }

    fn walk_dir(
        base: &Path,
        current: &Path,
        files: &mut Vec<FileEntry>,
        combined: &mut String,
    ) -> std::result::Result<(), NemesisError> {
        use sha2::{Digest, Sha256};
        let entries = std::fs::read_dir(current).map_err(|e| NemesisError::Io(e))?;
        for entry in entries {
            let entry = entry.map_err(|e| NemesisError::Io(e))?;
            let path = entry.path();

            // Skip hidden files and .signature.
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                Self::walk_dir(base, &path, files, combined)?;
            } else {
                let content = std::fs::read_to_string(&path).map_err(|e| NemesisError::Io(e))?;
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();

                let mut hasher = Sha256::new();
                hasher.update(content.as_bytes());
                let hash = format!("{:x}", hasher.finalize());

                combined.push_str(&relative);
                combined.push('\n');
                combined.push_str(&content);
                combined.push('\n');

                files.push(FileEntry {
                    path: relative,
                    hash,
                });
            }
        }
        Ok(())
    }
}

impl Default for SkillSigner {
    fn default() -> Self {
        Self::new()
    }
}

/// A file entry in the skill manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileEntry {
    path: String,
    hash: String,
}

/// The combined manifest of a skill directory.
struct SkillManifest {
    content: String,
    files: Vec<FileEntry>,
    #[allow(dead_code)]
    public_key_hint: String,
}

/// Compute a SHA-256 fingerprint of the public key.
fn compute_public_key_fingerprint(public_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(public_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Encode bytes to hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Decode hex string to 32 bytes.
fn hex_decode(hex: &str) -> std::result::Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", hex.len()));
    }
    let mut arr = [0u8; 32];
    for i in 0..32 {
        arr[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(arr)
}

#[cfg(test)]
mod tests;
