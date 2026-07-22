//! Per-workflow chat password storage.
//!
//! Backs the standalone workflow-chat page (`/workflow/chat/<index>`) so a
//! workflow can be shared with collaborators (URL + password) without
//! exposing the dashboard token. Passwords live in their own JSON file
//! (not embedded in workflow definitions) and are argon2-hashed at rest.
//!
//! File layout (`{home}/workspace/workflow/chat_secrets.json`):
//! ```json
//! { "<8hex_index>": "<argon2_hash>", "<8hex_index>": "<argon2_hash>" }
//! ```
//!
//! Key is the opaque chat index (`sha256(workflow_name)[0..8]`), same value
//! that appears in the URL. Workflow name is intentionally NOT stored here.

use std::collections::HashMap;
use std::path::PathBuf;

use argon2::Argon2;
use argon2::password_hash::SaltString;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier};
use parking_lot::RwLock;
use rand::rngs::OsRng;

/// A fixed hash used as a decoy when verifying against a non-existent index.
/// Without this, an attacker could distinguish "no such workflow / no password
/// set" (fast rejection) from "wrong password" (slow argon2 verify) by timing.
/// Running a real argon2 verify against this constant on every miss keeps the
/// timing uniform.
const DECOY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$ZGVjb3ltYW5kcmFuZG9tc2FsdA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Per-workflow chat password store. In-memory cache backed by a JSON file
/// on disk. Safe to share via `Arc` across handlers.
pub struct ChatSecretStore {
    inner: RwLock<Inner>,
}

struct Inner {
    /// On-disk JSON path. If None, the store is memory-only (used in tests).
    path: Option<PathBuf>,
    /// Map of chat index → argon2 hash string.
    secrets: HashMap<String, String>,
    /// True if the disk file failed to load (e.g. corrupt JSON). Writes are
    /// still attempted so the file can self-heal; reads return empty.
    dirty: bool,
}

impl ChatSecretStore {
    /// Create a new store backed by the given file path. Loads existing
    /// contents if the file exists; missing file is fine (treated as empty).
    pub fn open(path: PathBuf) -> Self {
        let mut secrets = HashMap::new();
        match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => {
                match serde_json::from_str::<HashMap<String, String>>(&text) {
                    Ok(map) => secrets = map,
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "[chat_secrets] corrupt JSON; starting empty"
                        );
                    }
                }
            }
            Ok(_) => {
                // empty file — leave secrets empty
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // first run — file doesn't exist yet
            }
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "[chat_secrets] failed to read; starting empty"
                );
            }
        }
        Self {
            inner: RwLock::new(Inner {
                path: Some(path),
                secrets,
                dirty: false,
            }),
        }
    }

    /// Create an in-memory-only store (no disk persistence). Used in tests
    /// and for safe-fail when path resolution fails.
    pub fn in_memory() -> Self {
        Self {
            inner: RwLock::new(Inner {
                path: None,
                secrets: HashMap::new(),
                dirty: false,
            }),
        }
    }

    /// Set (or replace) the password for the given chat index. Hashes the
    /// password with argon2 and writes the new map to disk.
    pub fn set_password(&self, index: &str, password: &str) -> Result<(), String> {
        if index.trim().is_empty() {
            return Err("index cannot be empty".to_string());
        }
        let hash = hash_password(password)?;
        let mut guard = self.inner.write();
        guard.secrets.insert(index.to_string(), hash);
        guard.dirty = true;
        self.persist_locked(&mut guard)
    }

    /// Remove the password for the given index. No-op if not set.
    pub fn clear_password(&self, index: &str) -> Result<(), String> {
        let mut guard = self.inner.write();
        if guard.secrets.remove(index).is_some() {
            guard.dirty = true;
            return self.persist_locked(&mut guard);
        }
        Ok(())
    }

    /// Returns true if the index has a password set.
    pub fn has_password(&self, index: &str) -> bool {
        self.inner.read().secrets.contains_key(index)
    }

    /// Verify a password against the stored hash for the index. Returns
    /// `false` if no password is set, **but still spends equivalent time**
    /// running argon2 verify against a decoy hash — so callers can't tell
    /// "no such index" from "wrong password" via timing.
    pub fn verify_password(&self, index: &str, password: &str) -> bool {
        let guard = self.inner.read();
        let hash_to_check = guard.secrets.get(index).cloned();
        drop(guard);

        match hash_to_check {
            Some(hash) => verify_password_inner(password, &hash),
            None => {
                // Decoy verify — keep timing uniform with the success path.
                let _ = verify_password_inner(password, DECOY_HASH);
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

impl ChatSecretStore {
    /// Persist current state to disk. Caller must hold the write lock.
    fn persist_locked(&self, inner: &mut Inner) -> Result<(), String> {
        let path = match inner.path.as_ref() {
            Some(p) => p,
            None => return Ok(()), // in-memory store
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create dir {:?}: {}", parent, e))?;
        }
        let text = serde_json::to_string_pretty(&inner.secrets)
            .map_err(|e| format!("serialize chat secrets: {}", e))?;
        // Write to tmp then rename so a crash mid-write doesn't corrupt
        // the existing file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(|e| format!("write tmp {:?}: {}", tmp, e))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| format!("rename {:?} -> {:?}: {}", tmp, path.display(), e))?;
        inner.dirty = false;
        Ok(())
    }
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    argon2
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("argon2 hash: {}", e))
}

fn verify_password_inner(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
