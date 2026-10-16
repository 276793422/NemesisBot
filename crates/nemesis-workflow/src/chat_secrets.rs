//! Per-workflow chat password storage.
//!
//! Backs the standalone workflow-chat page (`/workflow/chat/<index>`) so a
//! workflow can be shared with collaborators (URL + password) without
//! exposing the dashboard token. Passwords live in their own JSON file
//! (not embedded in workflow definitions) and are argon2-hashed at rest.
//!
//! File layout (`{home}/workspace/workflow/chat_secrets.json`):
//! ```json
//! { "<16hex_index>": "<argon2_hash>", "<8hex_index>": "<argon2_hash>" }
//! ```
//!
//! Key is the opaque chat index (`sha256(workflow_name)[0..16]`, 64 bit —
//! the canonical form since the 2026-10-16 hardening). Entries written by
//! the old 32-bit form (`[0..8]`) keep working: the read paths fall back to
//! the legacy key and the write paths clean the legacy twin up on rewrite.
//! Workflow name is intentionally NOT stored here.

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

/// Length of the canonical (64-bit) chat index in hex chars.
const CANONICAL_INDEX_LEN: usize = 16;

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
    /// True when the disk file could not be loaded (corrupt JSON or a
    /// non-NotFound read error) — the on-disk state is UNKNOWN, so the
    /// store fails closed: writes are refused (the file may hold
    /// recoverable data and must never be overwritten blindly) and every
    /// password check denies ([`Self::has_password`] reports true so
    /// callers keep demanding a password; [`Self::verify_password`]
    /// denies after a decoy-timed argon2 run). Recovers only by manual
    /// intervention on the file, then a gateway restart.
    poisoned: bool,
}

impl ChatSecretStore {
    /// Create a new store backed by the given file path. Loads existing
    /// contents if the file exists; missing file is fine (treated as empty).
    /// A file that exists but cannot be read/parse-poisons the store
    /// (fail-closed — see [`Inner::poisoned`]).
    pub fn open(path: PathBuf) -> Self {
        let mut secrets = HashMap::new();
        let mut poisoned = false;
        match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => {
                match serde_json::from_str::<HashMap<String, String>>(&text) {
                    Ok(map) => secrets = map,
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "[chat_secrets] corrupt JSON; store poisoned (fail-closed, file preserved)"
                        );
                        poisoned = true;
                        preserve_corrupt_copy(&path);
                    }
                }
            }
            Ok(_) => {
                // empty file — leave secrets empty
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // first run — file doesn't exist yet
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotADirectory => {
                // A parent component is a regular file — the target file
                // structurally cannot exist, so there is no unknown
                // on-disk state to protect. Windows reports this shape as
                // NotFound (ERROR_PATH_NOT_FOUND); Linux reports ENOTDIR —
                // treat both as "missing" so the store starts empty and a
                // broken path surfaces honestly at persist time instead
                // of poisoning on open.
            }
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "[chat_secrets] failed to read; store poisoned (fail-closed)"
                );
                poisoned = true;
            }
        }
        Self {
            inner: RwLock::new(Inner {
                path: Some(path),
                secrets,
                poisoned,
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
                poisoned: false,
            }),
        }
    }

    /// True when the disk file failed to load (corrupt JSON / read error).
    /// While poisoned, mutations return `Err` and verifications deny — the
    /// store stays locked until the file is recovered manually and the
    /// gateway restarted.
    pub fn is_poisoned(&self) -> bool {
        self.inner.read().poisoned
    }

    /// Set (or replace) the password for the given chat index. Hashes the
    /// password with argon2 and writes the new map to disk. When `index` is
    /// the canonical 16-hex form, any leftover legacy 8-hex twin entry is
    /// removed in the same write (self-healing migration to canonical keys).
    ///
    /// Persist-then-swap: the candidate map is written to disk first and the
    /// in-memory map is swapped only on success — a failed write leaves
    /// memory consistent with disk (the password is simply not set, on
    /// either side).
    pub fn set_password(&self, index: &str, password: &str) -> Result<(), String> {
        if index.trim().is_empty() {
            return Err("index cannot be empty".to_string());
        }
        let hash = hash_password(password)?;
        let mut guard = self.inner.write();
        if guard.poisoned {
            return Err(poisoned_err());
        }
        let mut candidate = guard.secrets.clone();
        candidate.insert(index.to_string(), hash);
        if index.len() == CANONICAL_INDEX_LEN {
            // Self-healing: drop the legacy 8-hex twin so the map converges
            // on canonical keys.
            candidate.remove(legacy_key_of(index));
        }
        persist_candidate_locked(&mut guard, candidate)
    }

    /// Remove the password for the given index. No-op if not set. Clears both
    /// the canonical key and its legacy 8-hex twin when given the 16-hex form.
    /// Persist-then-swap, same as [`Self::set_password`].
    pub fn clear_password(&self, index: &str) -> Result<(), String> {
        let mut guard = self.inner.write();
        if guard.poisoned {
            return Err(poisoned_err());
        }
        let mut candidate = guard.secrets.clone();
        let mut removed = candidate.remove(index).is_some();
        if index.len() == CANONICAL_INDEX_LEN {
            removed |= candidate.remove(legacy_key_of(index)).is_some();
        }
        if !removed {
            return Ok(());
        }
        persist_candidate_locked(&mut guard, candidate)
    }

    /// Returns true if the index has a password set. For the canonical 16-hex
    /// form, a legacy 8-hex entry also counts (pre-migration passwords). A
    /// poisoned store reports true — unknown state must look password-
    /// protected so callers keep demanding a credential (fail closed).
    pub fn has_password(&self, index: &str) -> bool {
        let guard = self.inner.read();
        if guard.poisoned {
            return true;
        }
        if guard.secrets.contains_key(index) {
            return true;
        }
        if index.len() == CANONICAL_INDEX_LEN {
            return guard.secrets.contains_key(legacy_key_of(index));
        }
        false
    }

    /// Verify a password against the stored hash for the index. Returns
    /// `false` if no password is set, **but still spends equivalent time**
    /// running argon2 verify against a decoy hash — so callers can't tell
    /// "no such index" from "wrong password" via timing. For the canonical
    /// 16-hex form, a legacy 8-hex entry is used when no canonical entry
    /// exists (pre-migration passwords keep working). A poisoned store
    /// denies (fail closed) with the same decoy timing.
    pub fn verify_password(&self, index: &str, password: &str) -> bool {
        let guard = self.inner.read();
        if guard.poisoned {
            drop(guard);
            let _ = verify_password_inner(password, DECOY_HASH);
            return false;
        }
        let mut hash_to_check = guard.secrets.get(index).cloned();
        if hash_to_check.is_none() && index.len() == CANONICAL_INDEX_LEN {
            hash_to_check = guard.secrets.get(legacy_key_of(index)).cloned();
        }
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

/// Uniform error for poisoned-store mutations (write-side fail-closed).
fn poisoned_err() -> String {
    "chat secret store is locked: the on-disk file failed to load and must \
     be recovered manually (see gateway logs for the preserved copy)"
        .to_string()
}

/// Best-effort diagnostic copy of a corrupt secrets file before anything
/// can overwrite it: `chat_secrets.json` → `chat_secrets.json.corrupt-<unix_ts>`.
/// Failure to copy is logged but never blocks startup.
fn preserve_corrupt_copy(path: &std::path::Path) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut dest = path.as_os_str().to_os_string();
    dest.push(format!(".corrupt-{}", ts));
    match std::fs::copy(path, &dest) {
        Ok(_) => tracing::warn!(
            preserved = %PathBuf::from(&dest).display(),
            "[chat_secrets] corrupt file preserved for manual recovery"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            "[chat_secrets] could not preserve a copy of the corrupt file"
        ),
    }
}

/// Serialize the candidate map, write it to disk atomically, and only then
/// swap the candidate into the live in-memory map. Caller holds the write
/// lock. On any write failure the live map is untouched (memory stays
/// consistent with disk).
fn persist_candidate_locked(
    inner: &mut Inner,
    candidate: HashMap<String, String>,
) -> Result<(), String> {
    let path = match inner.path.as_ref() {
        Some(p) => p,
        None => {
            // in-memory store — nothing to persist, just swap
            inner.secrets = candidate;
            return Ok(());
        }
    };
    let text = serde_json::to_string_pretty(&candidate)
        .map_err(|e| format!("serialize chat secrets: {}", e))?;
    nemesis_utils::file::write_file_atomic(&path.to_string_lossy(), text.as_bytes(), 0o600)
        .map_err(|e| format!("persist chat secrets: {}", e))?;
    inner.secrets = candidate;
    Ok(())
}

/// The legacy 32-bit key that a canonical 16-hex index was written under
/// before the 2026-10-16 hardening (first 8 hex chars of the same digest).
fn legacy_key_of(canonical: &str) -> &str {
    &canonical[..8]
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
