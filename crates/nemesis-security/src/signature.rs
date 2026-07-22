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

use base64::Engine;
use ed25519_dalek::Verifier as Ed25519Verifier;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use parking_lot::RwLock;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

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
            added_at: chrono::Local::now().to_rfc3339(),
            fingerprint,
        };
        self.keys.write().insert(public_key.to_string(), entry);
        let _ = self.save();
    }

    /// Remove a key by signer name. Returns true if a key was removed.
    pub fn remove_key(&self, name: &str) -> bool {
        let mut keys = self.keys.write();
        let b64 = keys
            .iter()
            .find(|(_, v)| v.name == name)
            .map(|(k, _)| k.clone());
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
        self.keys
            .read()
            .get(public_key)
            .map(|k| k.level)
            .unwrap_or(TrustLevel::Unknown)
    }

    /// List all keys currently in the trust store. The returned vec is a copy.
    pub fn list_keys(&self) -> Vec<TrustedKey> {
        self.keys.read().values().cloned().collect()
    }

    /// Revoke a key by signer name. Returns an error string if the key was not found.
    pub fn revoke_key(&self, name: &str) -> Result<(), String> {
        let mut keys = self.keys.write();
        let b64 = keys
            .iter()
            .find(|(_, v)| v.name == name)
            .map(|(k, _)| k.clone());
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
        let file = TrustStoreFile { version: 1, keys };
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

        let data =
            std::fs::read_to_string(path).map_err(|e| format!("cannot read trust store: {}", e))?;
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
            timestamp: chrono::Local::now().to_rfc3339(),
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
            timestamp: chrono::Local::now().to_rfc3339(),
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
            timestamp: chrono::Local::now().to_rfc3339(),
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
        let now = chrono::Local::now().to_rfc3339();

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
                error: format!(
                    "invalid signature length: expected 64 bytes, got {}",
                    sig_bytes.len()
                ),
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
    pub fn verify_file(
        &self,
        file_path: &Path,
        signature: &[u8],
    ) -> Result<VerificationResult, String> {
        let now = chrono::Local::now().to_rfc3339();

        let content = std::fs::read(file_path).map_err(|e| format!("cannot read file: {}", e))?;

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
    VerifyingKey::from_bytes(&arr).map_err(|e| format!("invalid public key: {}", e))
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
    let content = std::fs::read(file_path).map_err(|e| format!("cannot read file: {}", e))?;

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
    let now = chrono::Local::now().to_rfc3339();

    let content = std::fs::read(file_path).map_err(|e| format!("cannot read file: {}", e))?;

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
        signed_at: chrono::Local::now().to_rfc3339(),
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
pub fn verify_signature_ed25519(content: &[u8], signature_hex: &str, public_key_hex: &str) -> bool {
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

    let walker = walkdir::WalkDir::new(dir_path)
        .into_iter()
        .filter_entry(|e| {
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
        self.trust_store
            .add_key(public_key, label, TrustLevel::Verified);
    }

    /// Verify a skill file.
    pub fn verify_skill(
        &self,
        content: &str,
        signature_hex: &str,
        public_key_hex: &str,
    ) -> SkillVerification {
        let (_, trusted) = self.trust_store.is_trusted(public_key_hex);
        let trust_level = self.trust_store.trust_level(public_key_hex);
        let valid = self.verify_signature(content, signature_hex, public_key_hex);

        SkillVerification {
            valid,
            trusted,
            trust_level,
            public_key: public_key_hex.to_string(),
            error: if !valid {
                "signature verification failed".to_string()
            } else {
                String::new()
            },
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
mod tests;
