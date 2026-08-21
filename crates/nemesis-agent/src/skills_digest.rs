//! Skills catalog digest injection (H3 / P2.2, dsh-alignment second batch).
//!
//! Renders the installed-skills catalog as a compact digest (one line per
//! skill, name + truncated description) and injects it as a system-role
//! message at the same injection point as the time/env hint — but ONLY when
//! the digest CHANGED since the last injection for this session (dsh's
//! catalog-digest discipline: identical catalog ⇒ no new message ⇒ the
//! provider's warm KV prefix is preserved; a changed catalog injects a
//! replacement message that supersedes earlier ones).
//!
//! Known limitation (documented per goal): digest state is in-process,
//! per-session. A restart loses it and re-injects once — acceptable; the
//! restart itself already breaks the prefix.

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

/// Max characters of each skill's description kept in the digest.
const DIGEST_DESC_CHARS: usize = 500;

/// One skill's catalog entry (the minimal shape the digest needs).
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
}

/// Render the catalog digest: skills sorted by name, one line each
/// `name: <description truncated to DIGEST_DESC_CHARS>`.
pub fn render_skills_digest(skills: &[SkillCatalogEntry]) -> String {
    let mut lines: Vec<String> = skills
        .iter()
        .map(|s| {
            let desc: String = s.description.chars().take(DIGEST_DESC_CHARS).collect();
            if desc.is_empty() {
                s.name.clone()
            } else {
                format!("{}: {}", s.name, desc)
            }
        })
        .collect();
    lines.sort();
    lines.join("\n")
}

/// sha256 hex digest of the rendered catalog (first 16 hex chars are plenty
/// as a change key; we compare the full hex anyway).
pub fn digest_hash(rendered: &str) -> String {
    let mut h = Sha256::new();
    h.update(rendered.as_bytes());
    let out = h.finalize();
    out.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Build the injectable system message for a (changed) catalog. The wording
/// declares REPLACEMENT semantics: the model should treat this as the
/// complete current catalog, superseding any earlier one it saw.
pub fn digest_message(rendered: &str) -> String {
    format!(
        "# Available Skills\n{rendered}\n\n(这是当前完整的已安装技能目录，取代之前看到过的任何技能列表。用 skills_list 查看详情，用 skill 工具加载执行。)"
    )
}

/// Per-session digest state: the hash of the last injected catalog. Shared
/// via Arc on AgentLoop.
pub struct DigestState {
    last_injected: Mutex<HashMap<String, String>>,
}

impl DigestState {
    pub fn new() -> Self {
        Self {
            last_injected: Mutex::new(HashMap::new()),
        }
    }

    /// Decide whether this catalog should inject for `session_key`, and
    /// record the decision. Returns `Some(rendered)` when the catalog changed
    /// (caller builds the message and injects it); `None` when unchanged
    /// (inject nothing — preserve the prefix).
    pub fn should_inject(&self, session_key: &str, rendered: &str) -> Option<String> {
        let hash = digest_hash(rendered);
        let mut map = self.last_injected.lock();
        if map.get(session_key) == Some(&hash) {
            return None;
        }
        map.insert(session_key.to_string(), hash);
        Some(rendered.to_string())
    }

    /// H5 (U18): drop every session's recorded digest (touch-driven
    /// invalidation). Each live session re-injects once on its next build.
    pub fn clear_all(&self) {
        self.last_injected.lock().clear();
    }
}

/// Convenience: collect the catalog from a SkillsLoader scan result
/// (SkillInfo), truncating to the digest entry shape.
pub fn catalog_from_skills_infos(
    infos: &[nemesis_skills::types::SkillInfo],
) -> Vec<SkillCatalogEntry> {
    infos
        .iter()
        .map(|s| SkillCatalogEntry {
            name: s.name.clone(),
            description: s.description.clone(),
        })
        .collect()
}

/// Shared digest-state handle.
pub type SharedDigestState = Arc<DigestState>;

#[cfg(test)]
mod tests;
