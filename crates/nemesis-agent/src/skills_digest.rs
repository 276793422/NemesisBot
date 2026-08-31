//! Skills catalog digest injection (H3 / P2.2, dsh-alignment second batch).
//!
//! Renders the installed-skills catalog as a compact digest (one line per
//! skill, name + truncated description). Since I2 (U8) the digest rides the
//! MERGED snapshot message as one section among several (time/env + skills +
//! workspace instructions) inside a single <system-reminder> wrapper,
//! injected on EVERY build before the last user message — the injection is
//! NOT persisted in history, so each build must re-emit it, and byte-identical
//! re-emission (deterministic rendering) is what preserves the provider's
//! warm KV prefix.
//!
//! Round-5 note (dead state removed): the original change-detection design
//! (hash the merged content, inject only when it changed) was superseded by
//! the I2 merged-snapshot semantics — with the time section always present
//! and re-emitted every build, the stored hash gated nothing. The map and
//! per-build sha256 bookkeeping were removed; `should_inject` is now a
//! documented passthrough kept for the loop.rs call shape. If true
//! change-gating returns (e.g. per-field supersedes headers per U8's full
//! design), rebuild it as a SnapshotProjection, not by resurrecting this.

use sha2::{Digest, Sha256};

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

/// Digest emission state handle (round-5: stateless).
///
/// The original change-detection map was dead under I2 merged-snapshot
/// semantics (see module doc). Kept as a type so the AgentLoop field and
/// the H5 touch-invalidation call chain (`invalidate_context_digests`)
/// stay intact — the RE-READ of instruction files already happens every
/// build (sections are re-rendered from disk each time), so touch
/// invalidation is structurally covered. If change-gating returns, this is
/// the place to hang per-session state again.
pub struct DigestState;

impl Default for DigestState {
    fn default() -> Self {
        Self::new()
    }
}

impl DigestState {
    pub fn new() -> Self {
        Self
    }

    /// Decide the context-digest message for this build. Under I2
    /// merged-snapshot semantics this ALWAYS re-emits (the injection is not
    /// persisted in history; every build must carry it, byte-identically
    /// when nothing changed — deterministic rendering preserves the provider
    /// prefix). `session_key`/hash bookkeeping was removed with the dead
    /// change-detection state (round-5).
    pub fn should_inject(&self, _session_key: &str, rendered: &str) -> Option<String> {
        Some(rendered.to_string())
    }

    /// H5 (U18): touch-driven invalidation. Structurally a no-op since
    /// round-5 (sections re-read from disk on every build — there is no
    /// cached rendering to invalidate); kept for the call-chain shape and
    /// as the anchor if change-gating returns.
    pub fn clear_all(&self) {}
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

#[cfg(test)]
mod tests;
