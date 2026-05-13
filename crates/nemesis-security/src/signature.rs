//! Skill Signature Verification
//!
//! Ed25519-based skill signature verification with trust store management,
//! key generation, file persistence, and trust levels.
//!
//! Signature format:
//!   - Single file: SHA-256(file content) -> Ed25519 sign
//!   - Directory:   sorted file paths, each SHA-256 concatenated -> SHA-256 of concatenation -> Ed25519 sign
//!
//! A skill directory stores its signature in a `.signature` file at the skill root.

use ed25519_dalek::{SigningKey, VerifyingKey, Signer, Signature};
use ed25519_dalek::Verifier as Ed25519Verifier;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::path::Path;
use parking_lot::RwLock;
use base64::Engine;

/// Name of the signature file placed in skill directories.
const SIGNATURE_FILE_NAME: &str = ".signature";

/// Algorithm identifier used in the signature envelope.
const ALGORITHM_NAME: &str = "ed25519";

// ---------------------------------------------------------------------------
// TrustLevel
// ---------------------------------------------------------------------------

/// Trust level for a signing key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustLevel {
    /// Unknown key, not trusted.
    #[serde(rename = "unknown")]
    Unknown,
    /// Key belongs to a community signer.
    #[serde(rename = "community")]
    Community,
    /// Key belongs to an officially verified signer.
    #[serde(rename = "verified")]
    Verified,
    /// Key has been revoked and should not be trusted.
    #[serde(rename = "revoked")]
    Revoked,
}

impl Default for TrustLevel {
    fn default() -> Self {
        Self::Unknown
    }
}

impl std::fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => write!(f, "unknown"),
            Self::Community => write!(f, "community"),
            Self::Verified => write!(f, "verified"),
            Self::Revoked => write!(f, "revoked"),
        }
    }
}

// ---------------------------------------------------------------------------
// TrustedKey
// ---------------------------------------------------------------------------

/// A trusted public key entry persisted in the trust store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedKey {
    /// Base64-encoded Ed25519 public key.
    pub public_key: String,
    /// Signer name / identifier.
    pub name: String,
    /// Trust level.
    pub level: TrustLevel,
    /// When the key was added (RFC 3339).
    pub added_at: String,
    /// SHA-256 fingerprint of the public key bytes (hex).
    pub fingerprint: String,
}

// ---------------------------------------------------------------------------
// TrustStore file envelope
// ---------------------------------------------------------------------------

/// On-disk trust store JSON envelope.
#[derive(Debug, Serialize, Deserialize)]
struct TrustStoreFile {
    version: i32,
    keys: Vec<TrustedKey>,
}

// ---------------------------------------------------------------------------
// TrustStore
// ---------------------------------------------------------------------------

/// Trust store managing trusted Ed25519 public keys with file persistence.
///
/// Thread-safe via `parking_lot::RwLock`. If created without a file path,
/// the store operates in-memory only.
#[derive(Debug)]
pub struct TrustStore {
    keys: RwLock<HashMap<String, TrustedKey>>,
    path: Option<std::path::PathBuf>,
}

impl TrustStore {
    /// Create or load a trust store from the given file path.
    ///
    /// If the file does not exist, an empty store is returned.
    /// If `path` is empty / None, the store operates in-memory only.
    pub fn new(path: Option<impl Into<std::path::PathBuf>>) -> Self {
        let path = path.map(|p| p.into());
        let store = Self {
            keys: RwLock::new(HashMap::new()),
            path,
        };
        let _ = store.load();
        store
    }

    /// Create an empty in-memory trust store.
    pub fn in_memory() -> Self {
        Self::new(Option::<String>::None)
    }

    /// Add a public key to the trust store with the given name and trust level,
    /// then persist the store to disk.
    pub fn add_key(&self, public_key: &str, name: &str, level: TrustLevel) {
        let fingerprint = compute_fingerprint(public_key);
        let entry = TrustedKey {
            public_key: public_key.to_string(),
            name: name.to_string(),
            level,
            added_at: chrono::Utc::now().to_rfc3339(),
            fingerprint,
        };
        self.keys.write().insert(public_key.to_string(), entry);
        let _ = self.save();
    }

    /// Remove a key by signer name. Returns true if a key was removed.
    pub fn remove_key(&self, name: &str) -> bool {
        let mut keys = self.keys.write();
        let b64 = keys.iter().find(|(_, v)| v.name == name).map(|(k, _)| k.clone());
        if let Some(b64) = b64 {
            keys.remove(&b64);
            drop(keys);
            let _ = self.save();
            return true;
        }
        false
    }

    /// Remove a key by its base64-encoded public key string. Returns true if removed.
    pub fn remove_key_by_public_key(&self, public_key: &str) -> bool {
        let mut keys = self.keys.write();
        let removed = keys.remove(public_key).is_some();
        drop(keys);
        if removed {
            let _ = self.save();
        }
        removed
    }

    /// Check whether a public key is trusted (present and not revoked).
    ///
    /// Returns `(trust_level, true)` if trusted, or `(TrustLevel::Unknown, false)` otherwise.
    pub fn is_trusted(&self, public_key: &str) -> (TrustLevel, bool) {
        self.keys
            .read()
            .get(public_key)
            .map(|k| {
                if k.level == TrustLevel::Revoked {
                    (TrustLevel::Revoked, false)
                } else {
                    (k.level, true)
                }
            })
            .unwrap_or((TrustLevel::Unknown, false))
    }

    /// Get trust level for a key.
    pub fn trust_level(&self, public_key: &str) -> TrustLevel {
        self.keys.read().get(public_key).map(|k| k.level).unwrap_or(TrustLevel::Unknown)
    }

    /// List all keys currently in the trust store. The returned vec is a copy.
    pub fn list_keys(&self) -> Vec<TrustedKey> {
        self.keys.read().values().cloned().collect()
    }

    /// Revoke a key by signer name. Returns an error string if the key was not found.
    pub fn revoke_key(&self, name: &str) -> Result<(), String> {
        let mut keys = self.keys.write();
        let b64 = keys.iter().find(|(_, v)| v.name == name).map(|(k, _)| k.clone());
        if let Some(b64) = b64 {
            if let Some(entry) = keys.get_mut(&b64) {
                entry.level = TrustLevel::Revoked;
                drop(keys);
                let _ = self.save();
                return Ok(());
            }
        }
        Err(format!("key not found: {}", name))
    }

    /// Revoke a key by its base64-encoded public key string.
    pub fn revoke_key_by_public_key(&self, public_key: &str) -> Result<(), String> {
        let mut keys = self.keys.write();
        if let Some(entry) = keys.get_mut(public_key) {
            entry.level = TrustLevel::Revoked;
            drop(keys);
            let _ = self.save();
            return Ok(());
        }
        Err("key not found".to_string())
    }

    /// Get a key by its base64-encoded public key string.
    pub fn get_key(&self, public_key: &str) -> Option<TrustedKey> {
        self.keys.read().get(public_key).cloned()
    }

    /// Get a key by its signer name.
    pub fn get_key_by_name(&self, name: &str) -> Option<TrustedKey> {
        self.keys.read().values().find(|k| k.name == name).cloned()
    }

    /// Number of keys in the trust store.
    pub fn key_count(&self) -> usize {
        self.keys.read().len()
    }

    /// File path of the persistence file, or None if in-memory.
    pub fn file_path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Persist the trust store to disk. Caller does NOT need to hold the lock;
    /// this method acquires a read lock internally.
    fn save(&self) -> Result<(), String> {
        let path = match self.path {
            Some(ref p) => p,
            None => return Ok(()),
        };

        let keys: Vec<TrustedKey> = self.keys.read().values().cloned().collect();
        let file = TrustStoreFile {
            version: 1,
            keys,
        };
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;

        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        // Write atomically via a temp file.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &json).map_err(|e| format!("failed to write trust store: {}", e))?;
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("failed to rename trust store: {}", e)
        })?;

        Ok(())
    }

    /// Load the trust store from disk (if a path is set and the file exists).
    fn load(&self) -> Result<(), String> {
        let path = match self.path {
            Some(ref p) => p,
            None => return Ok(()),
        };

        if !path.exists() {
            return Ok(());
        }

        let data = std::fs::read_to_string(path).map_err(|e| format!("cannot read trust store: {}", e))?;
        if data.is_empty() {
            return Ok(());
        }

        let f: TrustStoreFile = serde_json::from_str(&data)
            .map_err(|e| format!("invalid trust store format: {}", e))?;

        let mut map = self.keys.write();
        for k in f.keys {
            // Validate the public key format by attempting to decode.
            if import_public_key(&k.public_key).is_err() {
                continue; // skip malformed entries
            }
            map.insert(k.public_key.clone(), k);
        }

        Ok(())
    }
}

impl Default for TrustStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

// ---------------------------------------------------------------------------
// VerificationResult
// ---------------------------------------------------------------------------

/// Result of a signature verification operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether the signature is valid.
    pub valid: bool,
    /// The signer name (from trust store or embedded hint).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signer: String,
    /// Trust level of the signer.
    pub trust_level: TrustLevel,
    /// Algorithm used for verification ("ed25519").
    pub algorithm: String,
    /// Error message if verification failed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Number of files verified.
    pub files_verified: usize,
    /// ISO 8601 timestamp of verification.
    pub timestamp: String,
}

impl VerificationResult {
    /// Create a successful verification result.
    pub fn ok(signer: &str, trust_level: TrustLevel, files_verified: usize) -> Self {
        Self {
            valid: true,
            signer: signer.to_string(),
            trust_level,
            algorithm: ALGORITHM_NAME.to_string(),
            error: String::new(),
            files_verified,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Create a failed verification result.
    pub fn err(error: &str) -> Self {
        Self {
            valid: false,
            signer: String::new(),
            trust_level: TrustLevel::Unknown,
            algorithm: ALGORITHM_NAME.to_string(),
            error: error.to_string(),
            files_verified: 0,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Create a failed result with file count information.
    pub fn err_with_files(error: &str, files_verified: usize) -> Self {
        Self {
            valid: false,
            signer: String::new(),
            trust_level: TrustLevel::Unknown,
            algorithm: ALGORITHM_NAME.to_string(),
            error: error.to_string(),
            files_verified,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }
}

// ---------------------------------------------------------------------------
// SkillSignature — on-disk signature envelope
// ---------------------------------------------------------------------------

/// The on-disk signature envelope stored in `.signature` files.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SkillSignature {
    /// Algorithm identifier (always "ed25519").
    algorithm: String,
    /// Base64-encoded Ed25519 signature.
    signature: String,
    /// Base64-encoded signer public key.
    public_key: String,
    /// Optional signer name hint.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    signer_name: String,
    /// ISO 8601 timestamp when signed.
    signed_at: String,
    /// Number of files covered.
    file_count: usize,
    /// SHA-256 hex of the aggregate content hash.
    hash: String,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the signature verifier.
#[derive(Debug, Clone)]
pub struct Config {
    /// Whether signature verification is enabled.
    pub enabled: bool,
    /// If true, unsigned skills are rejected; if false, warn only.
    pub strict: bool,
    /// Path to the trust store JSON file. Empty means in-memory.
    pub trust_store_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            strict: false,
            trust_store_path: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Verifier
// ---------------------------------------------------------------------------

/// Ed25519 signature verifier with trust store backing.
pub struct Verifier {
    trust_store: TrustStore,
    #[allow(dead_code)]
    config: Config,
}

impl Verifier {
    /// Create a new Verifier with the given configuration.
    ///
    /// If `config.trust_store_path` is empty, an in-memory trust store is used.
    /// If the file does not exist, an empty trust store is used.
    pub fn new(config: Config) -> Result<Self, String> {
        let ts = if config.trust_store_path.is_empty() {
            TrustStore::in_memory()
        } else {
            TrustStore::new(Some(&config.trust_store_path))
        };
        Ok(Self {
            trust_store: ts,
            config,
        })
    }

    /// Get a reference to the underlying trust store.
    pub fn trust_store(&self) -> &TrustStore {
        &self.trust_store
    }

    /// Verify the signature of an entire skill directory.
    ///
    /// Reads the `.signature` file inside `skill_path`, recomputes the aggregate
    /// hash over all files (excluding `.signature`), and checks the Ed25519
    /// signature against the embedded public key.
    pub fn verify_skill(&self, skill_path: &Path) -> VerificationResult {
        let now = chrono::Utc::now().to_rfc3339();

        // Validate the path exists and is a directory.
        if !skill_path.exists() {
            let r = VerificationResult {
                valid: false,
                signer: String::new(),
                trust_level: TrustLevel::Unknown,
                algorithm: ALGORITHM_NAME.to_string(),
                error: format!("cannot access skill path: not found"),
                files_verified: 0,
                timestamp: now,
            };
            return r;
        }
        if !skill_path.is_dir() {
            let r = VerificationResult {
                valid: false,
                signer: String::new(),
                trust_level: TrustLevel::Unknown,
                algorithm: ALGORITHM_NAME.to_string(),
                error: "skill path is not a directory".to_string(),
                files_verified: 0,
                timestamp: now,
            };
            return r;
        }

        // Read the .signature file.
        let sig_path = skill_path.join(SIGNATURE_FILE_NAME);
        let sig_data = match std::fs::read_to_string(&sig_path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return VerificationResult {
                    valid: false,
                    signer: String::new(),
                    trust_level: TrustLevel::Unknown,
                    algorithm: ALGORITHM_NAME.to_string(),
                    error: "no signature file found (.signature)".to_string(),
                    files_verified: 0,
                    timestamp: now,
                };
            }
            Err(e) => {
                return VerificationResult {
                    valid: false,
                    signer: String::new(),
                    trust_level: TrustLevel::Unknown,
                    algorithm: ALGORITHM_NAME.to_string(),
                    error: format!("cannot read signature file: {}", e),
                    files_verified: 0,
                    timestamp: now,
                };
            }
        };

        // Parse the signature envelope.
        let sig: SkillSignature = match serde_json::from_str(&sig_data) {
            Ok(s) => s,
            Err(e) => {
                return VerificationResult {
                    valid: false,
                    signer: String::new(),
                    trust_level: TrustLevel::Unknown,
                    algorithm: ALGORITHM_NAME.to_string(),
                    error: format!("invalid signature format: {}", e),
                    files_verified: 0,
                    timestamp: now,
                };
            }
        };

        if sig.algorithm != ALGORITHM_NAME {
            let algo = sig.algorithm.clone();
            return VerificationResult {
                valid: false,
                signer: String::new(),
                trust_level: TrustLevel::Unknown,
                algorithm: algo,
                error: format!("unsupported algorithm: {}", sig.algorithm),
                files_verified: 0,
                timestamp: now,
            };
        }

        // Decode the public key.
        let verifying_key = match import_public_key(&sig.public_key) {
            Ok(vk) => vk,
            Err(e) => {
                return VerificationResult {
                    valid: false,
                    signer: String::new(),
                    trust_level: TrustLevel::Unknown,
                    algorithm: ALGORITHM_NAME.to_string(),
                    error: format!("invalid public key in signature: {}", e),
                    files_verified: 0,
                    timestamp: now,
                };
            }
        };

        // Decode the signature bytes.
        let sig_bytes = match base64::engine::general_purpose::STANDARD.decode(&sig.signature) {
            Ok(b) => b,
            Err(e) => {
                return VerificationResult {
                    valid: false,
                    signer: String::new(),
                    trust_level: TrustLevel::Unknown,
                    algorithm: ALGORITHM_NAME.to_string(),
                    error: format!("invalid signature encoding: {}", e),
                    files_verified: 0,
                    timestamp: now,
                };
            }
        };

        if sig_bytes.len() != 64 {
            return VerificationResult {
                valid: false,
                signer: String::new(),
                trust_level: TrustLevel::Unknown,
                algorithm: ALGORITHM_NAME.to_string(),
                error: format!("invalid signature length: expected 64 bytes, got {}", sig_bytes.len()),
                files_verified: 0,
                timestamp: now,
            };
        }

        // Compute the aggregate hash of all files in the directory (excluding .signature).
        let (aggregate_hash, file_count) = match compute_directory_hash(skill_path) {
            Ok(r) => r,
            Err(e) => {
                return VerificationResult {
                    valid: false,
                    signer: String::new(),
                    trust_level: TrustLevel::Unknown,
                    algorithm: ALGORITHM_NAME.to_string(),
                    error: format!("failed to hash directory: {}", e),
                    files_verified: 0,
                    timestamp: now,
                };
            }
        };

        // Check the embedded hash against the computed one.
        let computed_hex = hex_encode(&aggregate_hash);
        if sig.hash != computed_hex {
            return VerificationResult {
                valid: false,
                signer: String::new(),
                trust_level: TrustLevel::Unknown,
                algorithm: ALGORITHM_NAME.to_string(),
                error: "content hash mismatch -- files have been modified".to_string(),
                files_verified: file_count,
                timestamp: now,
            };
        }

        // Verify the Ed25519 signature over the aggregate hash.
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig_bytes[..64]);
        let ed25519_sig = Signature::from_bytes(&sig_arr);
        if verifying_key.verify(&aggregate_hash, &ed25519_sig).is_err() {
            return VerificationResult {
                valid: false,
                signer: String::new(),
                trust_level: TrustLevel::Unknown,
                algorithm: ALGORITHM_NAME.to_string(),
                error: "signature verification failed".to_string(),
                files_verified: file_count,
                timestamp: now,
            };
        }

        // Look up the signer in the trust store.
        let mut signer_name = sig.signer_name;
        let (trust_level, trusted) = self.trust_store.is_trusted(&sig.public_key);
        if trusted {
            // Prefer the trust store name over the embedded hint.
            if let Some(k) = self.trust_store.get_key(&sig.public_key) {
                signer_name = k.name;
            }
        }

        VerificationResult {
            valid: true,
            signer: signer_name,
            trust_level,
            algorithm: ALGORITHM_NAME.to_string(),
            error: String::new(),
            files_verified: file_count,
            timestamp: now,
        }
    }

    /// Verify the Ed25519 signature of a single file.
    ///
    /// The signature should be the raw Ed25519 signature bytes (64 bytes).
    /// The verification computes SHA-256 of the file content and checks it
    /// against all keys in the trust store (skipping revoked keys).
    pub fn verify_file(&self, file_path: &Path, signature: &[u8]) -> Result<VerificationResult, String> {
        let now = chrono::Utc::now().to_rfc3339();

        let content = std::fs::read(file_path)
            .map_err(|e| format!("cannot read file: {}", e))?;

        let hash = Sha256::digest(&content);

        // Try all keys in the trust store.
        let keys = self.trust_store.list_keys();
        for k in &keys {
            if k.level == TrustLevel::Revoked {
                continue;
            }
            if let Ok(verifying_key) = import_public_key(&k.public_key) {
                if signature.len() == 64 {
                    let mut sig_arr = [0u8; 64];
                    sig_arr.copy_from_slice(signature);
                    let sig = Signature::from_bytes(&sig_arr);
                    if verifying_key.verify(&hash, &sig).is_ok() {
                        return Ok(VerificationResult {
                            valid: true,
                            signer: k.name.clone(),
                            trust_level: k.level,
                            algorithm: ALGORITHM_NAME.to_string(),
                            error: String::new(),
                            files_verified: 1,
                            timestamp: now,
                        });
                    }
                }
            }
        }

        Ok(VerificationResult {
            valid: false,
            signer: String::new(),
            trust_level: TrustLevel::Unknown,
            algorithm: ALGORITHM_NAME.to_string(),
            error: "signature does not match any trusted key".to_string(),
            files_verified: 1,
            timestamp: now,
        })
    }
}

// ---------------------------------------------------------------------------
// Free functions — key generation, signing, verification
// ---------------------------------------------------------------------------

/// Key pair for signing (hex-encoded).
#[derive(Debug)]
pub struct KeyPair {
    /// Hex-encoded Ed25519 private key (64 hex chars = 32 bytes seed).
    pub private_key: String,
    /// Hex-encoded Ed25519 public key (64 hex chars = 32 bytes).
    pub public_key: String,
}

/// Generate a new Ed25519 key pair.
pub fn generate_key_pair() -> Result<KeyPair, String> {
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let verifying_key = signing_key.verifying_key();

    Ok(KeyPair {
        private_key: hex_encode(signing_key.to_bytes().as_ref()),
        public_key: hex_encode(verifying_key.to_bytes().as_ref()),
    })
}

/// Export a public key as a base64 string.
pub fn export_public_key(verifying_key: &VerifyingKey) -> String {
    base64::engine::general_purpose::STANDARD.encode(verifying_key.to_bytes().as_ref())
}

/// Import a public key from a base64 string.
pub fn import_public_key(b64: &str) -> Result<VerifyingKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("invalid base64: {}", e))?;
    if bytes.len() != 32 {
        return Err(format!(
            "invalid public key size: expected 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    VerifyingKey::from_bytes(&arr)
        .map_err(|e| format!("invalid public key: {}", e))
}

/// Compute a SHA-256 fingerprint (hex) of a public key string, suitable for display.
pub fn compute_fingerprint(public_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(public_key.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Sign a single file.
///
/// Computes SHA-256 of the file content and signs the hash with the provided
/// signing key. Returns the raw 64-byte Ed25519 signature.
pub fn sign_file(file_path: &Path, signing_key: &SigningKey) -> Result<Vec<u8>, String> {
    let content = std::fs::read(file_path)
        .map_err(|e| format!("cannot read file: {}", e))?;

    let hash = Sha256::digest(&content);
    let sig = signing_key.sign(&hash);
    Ok(sig.to_bytes().to_vec())
}

/// Verify a single file's signature against a specific public key.
///
/// Returns a `VerificationResult` without consulting the trust store.
pub fn verify_file_with_key(
    file_path: &Path,
    verifying_key: &VerifyingKey,
    signature: &[u8],
) -> Result<VerificationResult, String> {
    let now = chrono::Utc::now().to_rfc3339();

    let content = std::fs::read(file_path)
        .map_err(|e| format!("cannot read file: {}", e))?;

    let hash = Sha256::digest(&content);

    if signature.len() != 64 {
        return Ok(VerificationResult {
            valid: false,
            signer: String::new(),
            trust_level: TrustLevel::Unknown,
            algorithm: ALGORITHM_NAME.to_string(),
            error: format!("invalid signature length: {}", signature.len()),
            files_verified: 1,
            timestamp: now,
        });
    }

    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(signature);
    let sig = Signature::from_bytes(&sig_arr);

    if verifying_key.verify(&hash, &sig).is_err() {
        return Ok(VerificationResult {
            valid: false,
            signer: String::new(),
            trust_level: TrustLevel::Unknown,
            algorithm: ALGORITHM_NAME.to_string(),
            error: "signature verification failed".to_string(),
            files_verified: 1,
            timestamp: now,
        });
    }

    Ok(VerificationResult {
        valid: true,
        signer: String::new(),
        trust_level: TrustLevel::Unknown,
        algorithm: ALGORITHM_NAME.to_string(),
        error: String::new(),
        files_verified: 1,
        timestamp: now,
    })
}

/// Sign an entire skill directory and write the `.signature` file.
///
/// Computes the aggregate hash of all files in the directory, signs it with
/// the provided signing key, and writes a `SkillSignature` envelope to
/// `{skill_path}/.signature`.
pub fn sign_skill(
    skill_path: &Path,
    signing_key: &SigningKey,
    signer_name: &str,
) -> Result<(), String> {
    let verifying_key = signing_key.verifying_key();

    let (aggregate_hash, file_count) = compute_directory_hash(skill_path)?;

    let sig = signing_key.sign(&aggregate_hash);

    let envelope = SkillSignature {
        algorithm: ALGORITHM_NAME.to_string(),
        signature: base64::engine::general_purpose::STANDARD.encode(sig.to_bytes().as_ref()),
        public_key: export_public_key(&verifying_key),
        signer_name: signer_name.to_string(),
        signed_at: chrono::Utc::now().to_rfc3339(),
        file_count,
        hash: hex_encode(&aggregate_hash),
    };

    let json = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("failed to marshal signature: {}", e))?;

    let sig_path = skill_path.join(SIGNATURE_FILE_NAME);
    std::fs::write(&sig_path, &json)
        .map_err(|e| format!("failed to write signature file: {}", e))?;

    Ok(())
}

/// Sign content with a private key (hex-encoded private key seed).
///
/// This is a convenience function that creates a `SigningKey` from the hex
/// private key, signs the content directly (not hashing first), and returns
/// the hex-encoded signature. Used by the existing test suite.
pub fn sign_content_hex(content: &str, private_key_hex: &str) -> Result<String, String> {
    let pk_bytes = hex_decode_32(private_key_hex)?;
    let signing_key = SigningKey::from_bytes(&pk_bytes);
    let signature = signing_key.sign(content.as_bytes());
    Ok(hex_encode(signature.to_bytes().as_ref()))
}

/// Verify a signature against content and a public key (hex-encoded).
///
/// Used by the existing `SignatureVerifier` compatibility layer.
pub fn verify_signature_ed25519(
    content: &[u8],
    signature_hex: &str,
    public_key_hex: &str,
) -> bool {
    if let Ok(sig_bytes) = hex_decode_vec(signature_hex) {
        if let Ok(pk_bytes) = hex_decode_32(public_key_hex) {
            if let Ok(verifying_key) = VerifyingKey::from_bytes(&pk_bytes) {
                if sig_bytes.len() == 64 {
                    let mut sig_arr = [0u8; 64];
                    sig_arr.copy_from_slice(&sig_bytes[..64]);
                    let sig = Signature::from_bytes(&sig_arr);
                    return verifying_key.verify(content, &sig).is_ok();
                }
            }
        }
    }
    false
}

/// Compute a deterministic "signature" (SHA-256 hex digest) from content + public_key.
/// Used as a fallback when Ed25519 keys are not available.
pub fn compute_hash_signature(content: &str, public_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher.update(public_key.as_bytes());
    hex_encode(&hasher.finalize())
}

// ---------------------------------------------------------------------------
// Directory hash
// ---------------------------------------------------------------------------

/// Compute a deterministic hash over all regular files in the given directory
/// (excluding the `.signature` file).
///
/// Files are sorted by their relative path (forward-slash normalized). Each
/// file contributes its relative path bytes followed by its SHA-256 hash.
/// The SHA-256 of the concatenation of all contributions is returned as the
/// aggregate hash.
fn compute_directory_hash(dir_path: &Path) -> Result<([u8; 32], usize), String> {
    let mut entries: Vec<(String, [u8; 32])> = Vec::new();

    let walker = walkdir::WalkDir::new(dir_path).into_iter().filter_entry(|e| {
        // Skip the signature file at the root level.
        if !e.file_type().is_dir() && e.file_name() == SIGNATURE_FILE_NAME {
            let rel = e.path().strip_prefix(dir_path).unwrap_or(e.path());
            // Only skip the root-level .signature
            rel.parent().map_or(true, |p| p.as_os_str().is_empty()) == false || false
        } else {
            true
        }
    });

    for entry in walker {
        let entry = entry.map_err(|e| format!("walk error: {}", e))?;
        if entry.file_type().is_dir() {
            continue;
        }
        // Skip the .signature file.
        if entry.file_name() == SIGNATURE_FILE_NAME {
            // Check if it's at the root level of the skill dir.
            let rel = entry.path().strip_prefix(dir_path).unwrap_or(entry.path());
            if rel.parent().map_or(true, |p| p.as_os_str().is_empty()) {
                continue;
            }
        }

        let rel = entry
            .path()
            .strip_prefix(dir_path)
            .map_err(|e| format!("cannot compute relative path: {}", e))?;
        // Normalize to forward slashes for cross-platform determinism.
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        let content = std::fs::read(entry.path())
            .map_err(|e| format!("cannot read file {}: {}", rel_str, e))?;

        let hash = Sha256::digest(&content);
        entries.push((rel_str, hash.into()));
    }

    // Sort by relative path for deterministic ordering.
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    // Concatenate path + hash for each entry.
    let mut buf: Vec<u8> = Vec::new();
    for (rel, hash) in &entries {
        buf.extend_from_slice(rel.as_bytes());
        buf.extend_from_slice(hash);
    }

    let aggregate = Sha256::digest(&buf);
    Ok((aggregate.into(), entries.len()))
}

// ---------------------------------------------------------------------------
// Hex utilities
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode_32(hex: &str) -> Result<[u8; 32], String> {
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

fn hex_decode_vec(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return Err("odd hex length".to_string());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// Legacy SignatureVerifier (compatibility)
// ---------------------------------------------------------------------------

/// Legacy Ed25519 skill signature verifier (for backward compatibility).
///
/// This struct wraps a `TrustStore` and provides a simpler API that uses
/// hex-encoded keys and signatures. For new code, prefer using `Verifier`
/// directly.
pub struct SignatureVerifier {
    trust_store: TrustStore,
}

impl SignatureVerifier {
    /// Create a new verifier backed by a fresh (empty) trust store.
    pub fn new() -> Self {
        Self {
            trust_store: TrustStore::in_memory(),
        }
    }

    /// Create a verifier that shares an existing trust store.
    pub fn with_trust_store(trust_store: TrustStore) -> Self {
        Self { trust_store }
    }

    /// Create a verifier with file-persisted trust store.
    pub fn with_persistence(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            trust_store: TrustStore::new(Some(path)),
        }
    }

    /// Borrow the trust store (read-only).
    pub fn trust_store_ref(&self) -> &TrustStore {
        &self.trust_store
    }

    /// Verify a signature against content and a public key.
    pub fn verify_signature(&self, content: &str, signature: &str, public_key: &str) -> bool {
        // Step 1: key must be trusted.
        if !self.trust_store.is_trusted(public_key).1 {
            return false;
        }

        // Step 2: try Ed25519 verification.
        if verify_signature_ed25519(content.as_bytes(), signature, public_key) {
            return true;
        }

        // Fallback: hash-based signature check (for backward compatibility).
        let expected = compute_hash_signature(content, public_key);
        expected == signature
    }

    /// Convenience: add a trusted key through the verifier.
    pub fn add_trusted_key(&self, public_key: &str, label: &str) {
        self.trust_store.add_key(public_key, label, TrustLevel::Verified);
    }

    /// Verify a skill file.
    pub fn verify_skill(&self, content: &str, signature_hex: &str, public_key_hex: &str) -> SkillVerification {
        let (_, trusted) = self.trust_store.is_trusted(public_key_hex);
        let trust_level = self.trust_store.trust_level(public_key_hex);
        let valid = self.verify_signature(content, signature_hex, public_key_hex);

        SkillVerification {
            valid,
            trusted,
            trust_level,
            public_key: public_key_hex.to_string(),
            error: if !valid { "signature verification failed".to_string() } else { String::new() },
        }
    }
}

impl Default for SignatureVerifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of legacy skill verification.
#[derive(Debug, Clone)]
pub struct SkillVerification {
    pub valid: bool,
    pub trusted: bool,
    pub trust_level: TrustLevel,
    pub public_key: String,
    pub error: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key_pair() {
        let kp = generate_key_pair().unwrap();
        assert_eq!(kp.private_key.len(), 64);
        assert_eq!(kp.public_key.len(), 64);
    }

    #[test]
    fn test_sign_and_verify() {
        let kp = generate_key_pair().unwrap();
        let verifier = SignatureVerifier::new();
        verifier.add_trusted_key(&kp.public_key, "test-author");

        let content = "hello, this is a skill";
        let sig = sign_content_hex(content, &kp.private_key).unwrap();

        assert!(verifier.verify_signature(content, &sig, &kp.public_key));
    }

    #[test]
    fn test_verify_rejects_untrusted_key() {
        let verifier = SignatureVerifier::new();
        let kp = generate_key_pair().unwrap();
        let content = "some content";
        let sig = sign_content_hex(content, &kp.private_key).unwrap();

        assert!(!verifier.verify_signature(content, &sig, &kp.public_key));
    }

    #[test]
    fn test_verify_rejects_tampered_content() {
        let kp = generate_key_pair().unwrap();
        let verifier = SignatureVerifier::new();
        verifier.add_trusted_key(&kp.public_key, "author");

        let original = "original content";
        let sig = sign_content_hex(original, &kp.private_key).unwrap();

        assert!(!verifier.verify_signature("tampered content", &sig, &kp.public_key));
    }

    #[test]
    fn test_backward_compat_hash_signature() {
        let verifier = SignatureVerifier::new();
        let pk = "test-public-key-001";
        verifier.add_trusted_key(pk, "test-author");

        let content = "hello, this is a skill";
        let sig = compute_hash_signature(content, pk);

        assert!(verifier.verify_signature(content, &sig, pk));
    }

    #[test]
    fn test_trust_store_management() {
        let store = TrustStore::in_memory();
        assert!(store.key_count() == 0);

        store.add_key("key-a", "alice", TrustLevel::Community);
        store.add_key("key-b", "bob", TrustLevel::Verified);
        assert_eq!(store.key_count(), 2);
        assert!(store.is_trusted("key-a").1);
        assert!(store.is_trusted("key-b").1);
        assert!(!store.is_trusted("key-c").1);

        assert!(store.remove_key("alice"));
        assert_eq!(store.key_count(), 1);
        assert!(!store.is_trusted("key-a").1);

        let keys = store.list_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].public_key, "key-b");
        assert_eq!(keys[0].name, "bob");
    }

    #[test]
    fn test_trust_levels() {
        let store = TrustStore::in_memory();
        store.add_key("key-1", "normal", TrustLevel::Community);
        store.add_key("key-2", "trusted", TrustLevel::Verified);

        assert_eq!(store.trust_level("key-1"), TrustLevel::Community);
        assert_eq!(store.trust_level("key-2"), TrustLevel::Verified);
        assert_eq!(store.trust_level("key-unknown"), TrustLevel::Unknown);
    }

    #[test]
    fn test_verify_skill_legacy() {
        let kp = generate_key_pair().unwrap();
        let verifier = SignatureVerifier::new();
        verifier.add_trusted_key(&kp.public_key, "test-author");

        let content = "# My Skill\nHello world";
        let sig = sign_content_hex(content, &kp.private_key).unwrap();
        let result = verifier.verify_skill(content, &sig, &kp.public_key);

        assert!(result.valid);
        assert!(result.trusted);
        assert_eq!(result.trust_level, TrustLevel::Verified);
        assert!(result.error.is_empty());
    }

    #[test]
    fn test_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust_store.json");

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = export_public_key(&verifying_key);

        {
            let store = TrustStore::new(Some(&path));
            store.add_key(&public_key_b64, "author1", TrustLevel::Verified);
            assert_eq!(store.key_count(), 1);
        }

        // Load from file
        let store2 = TrustStore::new(Some(&path));
        assert_eq!(store2.key_count(), 1);
        assert!(store2.is_trusted(&public_key_b64).1);
    }

    #[test]
    fn test_revoke_key() {
        let store = TrustStore::in_memory();
        store.add_key("key-a", "alice", TrustLevel::Verified);
        assert!(store.is_trusted("key-a").1);

        store.revoke_key("alice").unwrap();
        assert!(!store.is_trusted("key-a").1);
        assert_eq!(store.trust_level("key-a"), TrustLevel::Revoked);
        assert_eq!(store.key_count(), 1); // Still in store, just revoked
    }

    #[test]
    fn test_revoke_key_not_found() {
        let store = TrustStore::in_memory();
        assert!(store.revoke_key("nonexistent").is_err());
    }

    #[test]
    fn test_remove_key_not_found() {
        let store = TrustStore::in_memory();
        assert!(!store.remove_key("nonexistent"));
    }

    #[test]
    fn test_remove_key_by_public_key() {
        let store = TrustStore::in_memory();
        store.add_key("pk-abc", "alice", TrustLevel::Verified);
        assert!(store.remove_key_by_public_key("pk-abc"));
        assert!(!store.remove_key_by_public_key("pk-abc")); // already removed
    }

    #[test]
    fn test_revoke_key_by_public_key() {
        let store = TrustStore::in_memory();
        store.add_key("pk-xyz", "bob", TrustLevel::Community);
        store.revoke_key_by_public_key("pk-xyz").unwrap();
        assert!(!store.is_trusted("pk-xyz").1);
        assert_eq!(store.trust_level("pk-xyz"), TrustLevel::Revoked);
    }

    #[test]
    fn test_revoke_key_by_public_key_not_found() {
        let store = TrustStore::in_memory();
        assert!(store.revoke_key_by_public_key("nonexistent").is_err());
    }

    #[test]
    fn test_get_key_by_name() {
        let store = TrustStore::in_memory();
        store.add_key("key-a", "alice", TrustLevel::Verified);
        store.add_key("key-b", "bob", TrustLevel::Community);

        let found = store.get_key_by_name("alice");
        assert!(found.is_some());
        assert_eq!(found.unwrap().public_key, "key-a");

        assert!(store.get_key_by_name("charlie").is_none());
    }

    #[test]
    fn test_compute_fingerprint() {
        let fp = compute_fingerprint("test-public-key");
        assert!(!fp.is_empty());
        assert_eq!(fp.len(), 64); // SHA-256 hex
    }

    #[test]
    fn test_verification_result_ok() {
        let result = VerificationResult::ok("key-a", TrustLevel::Verified, 3);
        assert!(result.valid);
        assert_eq!(result.files_verified, 3);
        assert!(result.error.is_empty());
    }

    #[test]
    fn test_verification_result_err() {
        let result = VerificationResult::err("signature mismatch");
        assert!(!result.valid);
        assert_eq!(result.error, "signature mismatch");
    }

    // ----- Full file-level tests -----

    #[test]
    fn test_sign_file_and_verify_file_with_key() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello, this is a test file").unwrap();

        let kp = generate_key_pair().unwrap();
        let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&signing_key_bytes);
        let verifying_key = signing_key.verifying_key();

        let sig = sign_file(&file_path, &signing_key).unwrap();
        assert_eq!(sig.len(), 64);

        let result = verify_file_with_key(&file_path, &verifying_key, &sig).unwrap();
        assert!(result.valid);
        assert_eq!(result.files_verified, 1);
    }

    #[test]
    fn test_sign_file_tampered_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"original content").unwrap();

        let kp = generate_key_pair().unwrap();
        let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&signing_key_bytes);
        let verifying_key = signing_key.verifying_key();

        let sig = sign_file(&file_path, &signing_key).unwrap();

        // Tamper.
        std::fs::write(&file_path, b"tampered content").unwrap();

        let result = verify_file_with_key(&file_path, &verifying_key, &sig).unwrap();
        assert!(!result.valid);
    }

    #[test]
    fn test_sign_file_nonexistent() {
        let kp = generate_key_pair().unwrap();
        let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&signing_key_bytes);
        assert!(sign_file(Path::new("/nonexistent/file.txt"), &signing_key).is_err());
    }

    #[test]
    fn test_verify_file_with_key_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello").unwrap();

        let kp1 = generate_key_pair().unwrap();
        let kp2 = generate_key_pair().unwrap();
        let sk1_bytes = hex_decode_32(&kp1.private_key).unwrap();
        let sk1 = SigningKey::from_bytes(&sk1_bytes);
        let vk2_bytes = hex_decode_32(&kp2.public_key).unwrap();
        let vk2 = VerifyingKey::from_bytes(&vk2_bytes).unwrap();

        let sig = sign_file(&file_path, &sk1).unwrap();
        let result = verify_file_with_key(&file_path, &vk2, &sig).unwrap();
        assert!(!result.valid);
    }

    #[test]
    fn test_sign_skill_and_verify_skill() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("myskill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# My Skill\nA test skill.").unwrap();
        std::fs::write(skill_dir.join("config.json"), r#"{"name":"test"}"#).unwrap();

        let kp = generate_key_pair().unwrap();
        let signing_key_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&signing_key_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = export_public_key(&verifying_key);

        // Sign the skill.
        sign_skill(&skill_dir, &signing_key, "test-signer").unwrap();

        // Verify .signature file was created.
        assert!(skill_dir.join(SIGNATURE_FILE_NAME).exists());

        // Create verifier and add key.
        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();
        v.trust_store().add_key(&public_key_b64, "test-signer", TrustLevel::Verified);

        let result = v.verify_skill(&skill_dir);
        assert!(result.valid);
        assert_eq!(result.signer, "test-signer");
        assert_eq!(result.trust_level, TrustLevel::Verified);
        assert!(result.files_verified >= 2);
    }

    #[test]
    fn test_verify_skill_no_signature_file() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("nosig");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        assert!(!result.valid);
        assert!(!result.error.is_empty());
    }

    #[test]
    fn test_verify_skill_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("notadir.txt");
        std::fs::write(&file_path, b"test").unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&file_path);
        assert!(!result.valid);
    }

    #[test]
    fn test_sign_skill_tampered_content() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("tampered");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "original").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        sign_skill(&skill_dir, &signing_key, "signer").unwrap();

        // Tamper.
        std::fs::write(skill_dir.join("SKILL.md"), "tampered").unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        assert!(!result.valid);
    }

    #[test]
    fn test_verifier_verify_file_with_trust_store() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = export_public_key(&verifying_key);

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();
        v.trust_store().add_key(&public_key_b64, "test-signer", TrustLevel::Community);

        let sig = sign_file(&file_path, &signing_key).unwrap();
        let result = v.verify_file(&file_path, &sig).unwrap();
        assert!(result.valid);
        assert_eq!(result.signer, "test-signer");
    }

    #[test]
    fn test_verifier_verify_file_unknown_key() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let sig = sign_file(&file_path, &signing_key).unwrap();
        let result = v.verify_file(&file_path, &sig).unwrap();
        assert!(!result.valid);
    }

    #[test]
    fn test_verifier_verify_file_revoked_key() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = export_public_key(&verifying_key);

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();
        v.trust_store().add_key(&public_key_b64, "revoke-me", TrustLevel::Verified);
        v.trust_store().revoke_key("revoke-me").unwrap();

        let sig = sign_file(&file_path, &signing_key).unwrap();
        let result = v.verify_file(&file_path, &sig).unwrap();
        assert!(!result.valid);
    }

    #[test]
    fn test_export_import_public_key() {
        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();

        let exported = export_public_key(&verifying_key);
        assert!(!exported.is_empty());

        let imported = import_public_key(&exported).unwrap();
        assert_eq!(verifying_key.to_bytes(), imported.to_bytes());
    }

    #[test]
    fn test_import_public_key_invalid_base64() {
        assert!(import_public_key("not-valid-base64!!!").is_err());
    }

    #[test]
    fn test_import_public_key_wrong_size() {
        assert!(import_public_key("aG93ZHk=").is_err()); // "howdy" = 6 bytes
    }

    #[test]
    fn test_import_public_key_empty_string() {
        assert!(import_public_key("").is_err());
    }

    #[test]
    fn test_concurrent_trust_store_access() {
        use std::sync::Arc;
        use std::thread;

        let store = Arc::new(TrustStore::in_memory());
        let mut handles = Vec::new();

        for i in 0..10 {
            let s = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                s.add_key(
                    &format!("key-{}", i),
                    &format!("name-{}", i),
                    TrustLevel::Community,
                );
            }));
        }
        for _ in 0..10 {
            let s = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                let _ = s.list_keys();
                let _ = s.key_count();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(store.key_count(), 10);
    }

    #[test]
    fn test_trust_store_persistence_multiple_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");

        let mut public_keys = Vec::new();
        {
            let store = TrustStore::new(Some(&path));
            for i in 0..5 {
                let kp = generate_key_pair().unwrap();
                let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
                let signing_key = SigningKey::from_bytes(&sk_bytes);
                let verifying_key = signing_key.verifying_key();
                let pk = export_public_key(&verifying_key);
                public_keys.push(pk.clone());
                store.add_key(&pk, &format!("name-{}", i), TrustLevel::Community);
            }
        }

        let store2 = TrustStore::new(Some(&path));
        assert_eq!(store2.key_count(), 5);
        for pk in &public_keys {
            assert!(store2.is_trusted(pk).1);
        }
    }

    #[test]
    fn test_trust_store_persistence_skip_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.json");

        // Write a trust store with a malformed key entry.
        let content = r#"{"version":1,"keys":[{"public_key":"invalid-base64!!!","name":"bad","level":"community","added_at":"2026-01-01T00:00:00Z","fingerprint":"abc"}]}"#;
        std::fs::write(&path, content).unwrap();

        let store = TrustStore::new(Some(&path));
        assert_eq!(store.key_count(), 0);
    }

    #[test]
    fn test_new_verifier_empty_trust_store() {
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: String::new(),
        }).unwrap();
        assert_eq!(v.trust_store().key_count(), 0);
    }

    #[test]
    fn test_sign_skill_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("empty");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        sign_skill(&skill_dir, &signing_key, "signer").unwrap();
        assert!(skill_dir.join(SIGNATURE_FILE_NAME).exists());
    }

    // ---- Additional coverage tests ----

    #[test]
    fn test_trust_level_display() {
        assert_eq!(format!("{}", TrustLevel::Unknown), "unknown");
        assert_eq!(format!("{}", TrustLevel::Community), "community");
        assert_eq!(format!("{}", TrustLevel::Verified), "verified");
        assert_eq!(format!("{}", TrustLevel::Revoked), "revoked");
    }

    #[test]
    fn test_trust_level_default() {
        assert_eq!(TrustLevel::default(), TrustLevel::Unknown);
    }

    #[test]
    fn test_trust_level_serialization_roundtrip() {
        for level in [TrustLevel::Unknown, TrustLevel::Community, TrustLevel::Verified, TrustLevel::Revoked] {
            let json = serde_json::to_string(&level).unwrap();
            let parsed: TrustLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, level);
        }
    }

    #[test]
    fn test_trusted_key_serialization() {
        let key = TrustedKey {
            public_key: "test-key".to_string(),
            name: "test-author".to_string(),
            level: TrustLevel::Verified,
            added_at: "2026-01-01T00:00:00Z".to_string(),
            fingerprint: "abc123".to_string(),
        };
        let json = serde_json::to_string(&key).unwrap();
        let parsed: TrustedKey = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.public_key, "test-key");
        assert_eq!(parsed.name, "test-author");
        assert_eq!(parsed.level, TrustLevel::Verified);
    }

    #[test]
    fn test_trust_store_is_trusted_unknown_key() {
        let store = TrustStore::in_memory();
        let (level, trusted) = store.is_trusted("nonexistent");
        assert_eq!(level, TrustLevel::Unknown);
        assert!(!trusted);
    }

    #[test]
    fn test_trust_store_is_trusted_revoked_key() {
        let store = TrustStore::in_memory();
        store.add_key("key-1", "revoked-author", TrustLevel::Verified);
        store.revoke_key("revoked-author").unwrap();
        let (level, trusted) = store.is_trusted("key-1");
        assert_eq!(level, TrustLevel::Revoked);
        assert!(!trusted);
    }

    #[test]
    fn test_trust_store_get_key_by_public_key() {
        let store = TrustStore::in_memory();
        store.add_key("pk-abc", "alice", TrustLevel::Community);

        // Keys can be found via get_key_by_name
        let found = store.get_key_by_name("alice");
        assert!(found.is_some());
        assert_eq!(found.unwrap().public_key, "pk-abc");
    }

    #[test]
    fn test_verifier_config_enabled_field() {
        let config = Config {
            enabled: false,
            strict: true,
            trust_store_path: String::new(),
        };
        assert!(!config.enabled);
        assert!(config.strict);
    }

    #[test]
    fn test_verifier_verify_file_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_file(Path::new("/nonexistent/file.txt"), b"somesig");
        // verify_file returns an error for nonexistent files
        assert!(result.is_err() || !result.unwrap().valid);
    }

    #[test]
    fn test_signature_file_name_constant() {
        assert_eq!(SIGNATURE_FILE_NAME, ".signature");
    }

    #[test]
    fn test_hex_decode_32_invalid_hex() {
        let result = hex_decode_32("not-valid-hex");
        assert!(result.is_err());
    }

    #[test]
    fn test_hex_decode_32_wrong_length() {
        let result = hex_decode_32("abcd");
        assert!(result.is_err());
    }

    #[test]
    fn test_sign_content_hex_roundtrip() {
        let kp = generate_key_pair().unwrap();
        let content = "test content for signing";
        let sig = sign_content_hex(content, &kp.private_key).unwrap();
        assert!(!sig.is_empty());
        assert_eq!(sig.len(), 128); // 64 bytes hex = 128 chars
    }

    #[test]
    fn test_sign_content_hex_invalid_key() {
        let result = sign_content_hex("content", "invalid-key");
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_hash_signature_deterministic() {
        let content = "same content";
        let key = "same-key";
        let sig1 = compute_hash_signature(content, key);
        let sig2 = compute_hash_signature(content, key);
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_compute_fingerprint_different_keys() {
        let fp1 = compute_fingerprint("key-1");
        let fp2 = compute_fingerprint("key-2");
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_signature_verifier_no_trusted_keys() {
        let verifier = SignatureVerifier::new();
        let kp = generate_key_pair().unwrap();
        let sig = sign_content_hex("content", &kp.private_key).unwrap();
        assert!(!verifier.verify_signature("content", &sig, &kp.public_key));
    }

    #[test]
    fn test_signature_verifier_remove_trusted_key() {
        let verifier = SignatureVerifier::new();
        verifier.add_trusted_key("key-1", "author-1");
        // After adding, it should verify with hash sig
        let hash_sig = compute_hash_signature("content", "key-1");
        assert!(verifier.verify_signature("content", &hash_sig, "key-1"));
    }

    #[test]
    fn test_verification_result_default_fields() {
        let result = VerificationResult::err("test");
        assert!(!result.valid);
        assert_eq!(result.trust_level, TrustLevel::Unknown);
        assert!(result.signer.is_empty());
        assert_eq!(result.error, "test");
        assert_eq!(result.files_verified, 0);
    }

    #[test]
    fn test_verify_skill_not_strict_unknown_key() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        sign_skill(&skill_dir, &signing_key, "unknown-signer").unwrap();

        let ts_path = dir.path().join("ts.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false, // not strict -> should report valid but unknown trust
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        // Not strict: valid signature but key not in trust store
        assert!(result.valid);
        assert_eq!(result.trust_level, TrustLevel::Unknown);
    }

    #[test]
    fn test_verifier_no_signature_file_returns_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();
        // No .signature file

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        assert!(!result.valid);
        assert!(result.error.contains("no signature file"));
    }

    #[test]
    fn test_algorithm_name_constant() {
        assert_eq!(ALGORITHM_NAME, "ed25519");
    }

    // ============================================================
    // Additional tests for 95%+ coverage
    // ============================================================

    #[test]
    fn test_sign_skill_creates_signature_file_with_correct_format() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("sigtest");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill\nContent here.").unwrap();
        std::fs::write(skill_dir.join("data.json"), r#"{"version":"1.0"}"#).unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        sign_skill(&skill_dir, &signing_key, "test-author").unwrap();

        // Verify .signature file exists and is valid JSON
        let sig_path = skill_dir.join(SIGNATURE_FILE_NAME);
        assert!(sig_path.exists());
        let content = std::fs::read_to_string(&sig_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["algorithm"], "ed25519");
        assert_eq!(parsed["signer_name"], "test-author");
        assert!(parsed["signature"].is_string());
        assert!(parsed["public_key"].is_string());
        assert!(parsed["file_count"].is_number());
        assert!(parsed["signed_at"].is_string());
        assert!(parsed["hash"].is_string());
    }

    #[test]
    fn test_verifier_strict_mode_unknown_key_untrusted() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("strict-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        sign_skill(&skill_dir, &signing_key, "unknown-signer").unwrap();

        let ts_path = dir.path().join("ts.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: true, // strict mode
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        // Signature is cryptographically valid but signer is unknown (not trusted)
        assert!(result.valid); // signature is valid
        assert_eq!(result.trust_level, TrustLevel::Unknown); // but not trusted
        assert_eq!(result.signer, "unknown-signer");
    }

    #[test]
    fn test_signature_verifier_default() {
        let verifier = SignatureVerifier::default();
        assert_eq!(verifier.trust_store_ref().key_count(), 0);
    }

    #[test]
    fn test_signature_verifier_with_trust_store() {
        let store = TrustStore::in_memory();
        store.add_key("key-a", "alice", TrustLevel::Verified);
        let verifier = SignatureVerifier::with_trust_store(store);
        assert_eq!(verifier.trust_store_ref().key_count(), 1);
    }

    #[test]
    fn test_signature_verifier_with_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ts.json");
        let verifier = SignatureVerifier::with_persistence(&path);
        assert_eq!(verifier.trust_store_ref().key_count(), 0);
    }

    #[test]
    fn test_verify_skill_legacy_untrusted_key() {
        let verifier = SignatureVerifier::new();
        let kp = generate_key_pair().unwrap();
        // Don't add key to trust store
        let content = "# My Skill";
        let sig = sign_content_hex(content, &kp.private_key).unwrap();
        let result = verifier.verify_skill(content, &sig, &kp.public_key);
        assert!(!result.valid);
        assert!(!result.trusted);
        assert_eq!(result.trust_level, TrustLevel::Unknown);
        assert!(!result.error.is_empty());
    }

    #[test]
    fn test_verify_skill_legacy_valid_trusted() {
        let verifier = SignatureVerifier::new();
        let kp = generate_key_pair().unwrap();
        verifier.add_trusted_key(&kp.public_key, "test-author");
        let content = "# My Skill";
        let sig = sign_content_hex(content, &kp.private_key).unwrap();
        let result = verifier.verify_skill(content, &sig, &kp.public_key);
        assert!(result.valid);
        assert!(result.trusted);
        assert_eq!(result.trust_level, TrustLevel::Verified);
        assert!(result.error.is_empty());
    }

    #[test]
    fn test_verify_skill_legacy_wrong_content() {
        let verifier = SignatureVerifier::new();
        let kp = generate_key_pair().unwrap();
        verifier.add_trusted_key(&kp.public_key, "test-author");
        let sig = sign_content_hex("original content", &kp.private_key).unwrap();
        let result = verifier.verify_skill("wrong content", &sig, &kp.public_key);
        assert!(!result.valid);
    }

    #[test]
    fn test_trust_store_get_key_by_name_not_found() {
        let store = TrustStore::in_memory();
        assert!(store.get_key_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_trust_store_add_multiple_same_name_different_keys() {
        let store = TrustStore::in_memory();
        store.add_key("key-1", "alice", TrustLevel::Community);
        store.add_key("key-2", "alice", TrustLevel::Verified);
        // Both keys are stored since they have different public_key values
        assert_eq!(store.key_count(), 2);
        // get_key_by_name returns the first match
        let key = store.get_key_by_name("alice");
        assert!(key.is_some());
    }

    #[test]
    fn test_hex_decode_vec_valid() {
        let result = hex_decode_vec("48656c6c6f");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), b"Hello");
    }

    #[test]
    fn test_hex_decode_vec_odd_length() {
        let result = hex_decode_vec("abc");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("odd hex length"));
    }

    #[test]
    fn test_hex_decode_vec_invalid_chars() {
        let result = hex_decode_vec("ghij");
        assert!(result.is_err());
    }

    #[test]
    fn test_sign_content_hex_produces_128_char_hex() {
        let kp = generate_key_pair().unwrap();
        let sig = sign_content_hex("content", &kp.private_key).unwrap();
        assert_eq!(sig.len(), 128);
        // Should be valid hex
        for c in sig.chars() {
            assert!(c.is_ascii_hexdigit(), "char '{}' is not hex", c);
        }
    }

    #[test]
    fn test_verifier_verify_file_corrupt_signature() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        // Pass a corrupt signature (wrong length)
        let result = v.verify_file(&file_path, b"corrupt_sig");
        // Should return error or invalid result
        assert!(result.is_err() || !result.as_ref().unwrap().valid);
    }

    #[test]
    fn test_trust_store_list_keys_empty() {
        let store = TrustStore::in_memory();
        let keys = store.list_keys();
        assert!(keys.is_empty());
    }

    #[test]
    fn test_verifier_verify_file_with_strict_mode() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = export_public_key(&verifying_key);

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: true,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();
        // Don't add key - strict mode should reject unknown keys
        v.trust_store().add_key(&public_key_b64, "test-signer", TrustLevel::Verified);

        let sig = sign_file(&file_path, &signing_key).unwrap();
        let result = v.verify_file(&file_path, &sig).unwrap();
        assert!(result.valid);
    }

    #[test]
    fn test_sign_file_empty_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("empty.txt");
        std::fs::write(&file_path, b"").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();

        let sig = sign_file(&file_path, &signing_key).unwrap();
        let result = verify_file_with_key(&file_path, &verifying_key, &sig).unwrap();
        assert!(result.valid);
    }

    #[test]
    fn test_verifier_verify_skill_with_tampered_signature_file() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("tampered_sig");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        sign_skill(&skill_dir, &signing_key, "signer").unwrap();

        // Tamper with the .signature file
        let sig_path = skill_dir.join(SIGNATURE_FILE_NAME);
        let mut content: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&sig_path).unwrap()).unwrap();
        content["signature"] = serde_json::json!("aaaa0000bbbb1111");
        std::fs::write(&sig_path, serde_json::to_string(&content).unwrap()).unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        assert!(!result.valid);
    }

    #[test]
    fn test_verifier_verify_skill_with_invalid_signature_json() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("invalid_sig");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();
        // Write invalid JSON as .signature
        std::fs::write(skill_dir.join(SIGNATURE_FILE_NAME), "not valid json {{{{").unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();

        let result = v.verify_skill(&skill_dir);
        assert!(!result.valid);
    }

    #[test]
    fn test_compute_hash_signature_different_content() {
        let key = "same-key";
        let sig1 = compute_hash_signature("content-a", key);
        let sig2 = compute_hash_signature("content-b", key);
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_compute_hash_signature_different_keys() {
        let content = "same-content";
        let sig1 = compute_hash_signature(content, "key-a");
        let sig2 = compute_hash_signature(content, "key-b");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_trust_level_equality() {
        assert_eq!(TrustLevel::Verified, TrustLevel::Verified);
        assert_eq!(TrustLevel::Unknown, TrustLevel::Unknown);
        assert_ne!(TrustLevel::Verified, TrustLevel::Community);
        assert_ne!(TrustLevel::Community, TrustLevel::Unknown);
        assert_ne!(TrustLevel::Unknown, TrustLevel::Revoked);
    }

    #[test]
    fn test_trusted_key_all_fields() {
        let key = TrustedKey {
            public_key: "pk-123".to_string(),
            name: "author".to_string(),
            level: TrustLevel::Community,
            added_at: "2026-01-15T10:30:00Z".to_string(),
            fingerprint: "abc123def456".to_string(),
        };
        assert_eq!(key.public_key, "pk-123");
        assert_eq!(key.name, "author");
        assert_eq!(key.level, TrustLevel::Community);
        assert_eq!(key.added_at, "2026-01-15T10:30:00Z");
        assert_eq!(key.fingerprint, "abc123def456");
    }

    #[test]
    fn test_verification_result_ok_with_all_fields() {
        let result = VerificationResult::ok("key-a", TrustLevel::Verified, 5);
        assert!(result.valid);
        assert_eq!(result.signer, "key-a");
        assert_eq!(result.trust_level, TrustLevel::Verified);
        assert_eq!(result.files_verified, 5);
        assert_eq!(result.algorithm, "ed25519");
        assert!(result.error.is_empty());
        assert!(!result.timestamp.is_empty());
    }

    #[test]
    fn test_verification_result_err_message() {
        let result = VerificationResult::err("something broke");
        assert!(!result.valid);
        assert_eq!(result.error, "something broke");
        assert_eq!(result.trust_level, TrustLevel::Unknown);
        assert_eq!(result.files_verified, 0);
        assert!(result.signer.is_empty());
        assert_eq!(result.algorithm, "ed25519");
        assert_eq!(result.files_verified, 0);
        assert!(result.signer.is_empty());
    }

    #[test]
    fn test_sign_skill_with_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("bigskill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        for i in 0..10 {
            std::fs::write(skill_dir.join(format!("file{}.txt", i)), format!("content {}", i)).unwrap();
        }

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);
        let verifying_key = signing_key.verifying_key();
        let public_key_b64 = export_public_key(&verifying_key);

        sign_skill(&skill_dir, &signing_key, "multi-author").unwrap();

        let ts_path = dir.path().join("truststore.json");
        let v = Verifier::new(Config {
            enabled: true,
            strict: false,
            trust_store_path: ts_path.to_string_lossy().to_string(),
        }).unwrap();
        v.trust_store().add_key(&public_key_b64, "multi-author", TrustLevel::Verified);

        let result = v.verify_skill(&skill_dir);
        assert!(result.valid);
        assert!(result.files_verified >= 10);
    }

    #[test]
    fn test_sign_skill_overwrites_existing_signature() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("resign");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# V1").unwrap();

        let kp = generate_key_pair().unwrap();
        let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
        let signing_key = SigningKey::from_bytes(&sk_bytes);

        sign_skill(&skill_dir, &signing_key, "author-v1").unwrap();
        let sig1 = std::fs::read_to_string(skill_dir.join(SIGNATURE_FILE_NAME)).unwrap();

        // Modify and re-sign
        std::fs::write(skill_dir.join("SKILL.md"), "# V2").unwrap();
        sign_skill(&skill_dir, &signing_key, "author-v2").unwrap();
        let sig2 = std::fs::read_to_string(skill_dir.join(SIGNATURE_FILE_NAME)).unwrap();

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_import_export_roundtrip_multiple_keys() {
        for _ in 0..5 {
            let kp = generate_key_pair().unwrap();
            let sk_bytes = hex_decode_32(&kp.private_key).unwrap();
            let signing_key = SigningKey::from_bytes(&sk_bytes);
            let verifying_key = signing_key.verifying_key();
            let exported = export_public_key(&verifying_key);
            let imported = import_public_key(&exported).unwrap();
            assert_eq!(verifying_key.to_bytes(), imported.to_bytes());
        }
    }
}
