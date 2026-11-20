//! Skills catalog digest injection (/.2).
//!
//! Renders the installed-skills catalog as a compact digest (one line per
//! skill, name + truncated description). Since the digest rides the
//! MERGED snapshot message as one section among several (time/env + skills +
//! workspace instructions) inside a single <system-reminder> wrapper,
//! injected on EVERY build before the last user message — the injection is
//! NOT persisted in history, so each build must re-emit it, and byte-identical
//! re-emission (deterministic rendering) is what preserves the provider's
//! warm KV prefix.
//!
//! Round-5 note (dead state removed): the original change-detection design
//! (hash the merged content, inject only when it changed) was superseded by
//! the merged-snapshot semantics — with the time section always present
//! and re-emitted every build, the stored hash gated nothing. The map and
//! per-build sha256 bookkeeping were removed; `should_inject` is now a
//! documented passthrough kept for the loop.rs call shape. If true
//! change-gating returns (e.g. per-field supersedes headers per 's full
//! design), rebuild it as a SnapshotProjection, not by resurrecting this.
//!
//! P2-1（2026-11-19）：`change_reminder` 把 per-session
//! 状态请了回来——但只记**技能名集**（不是渲染内容 hash），产出一行
//! `+x/-y` 提醒。cache 纪律成立：提醒行只在集变化轮出现，而那一轮目录
//! 字节本来就变了（零额外失效）；下一轮快照已更新，提醒消失，merged
//! 字节回到稳定形态。重放面安全：提醒随 merged snapshot 逐 build 重渲
//! 染、不进历史，与 time 节同一「per-build 态」语义（台账回放不重跑
//! build_messages 比字节——time 每轮都变，先例已定）。

use std::collections::HashMap;

use parking_lot::RwLock;
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

/// P2-1 变更提醒行渲染（纯函数，便于单测）。`added`/`removed` 不会同时
/// 为空（调用方只在集合变化时构造）；全部卸载时 removed 覆盖整个旧集，
/// 此时 Available Skills 节本身也会消失（infos 空 → 不推目录节），文案
/// 不依赖该节在场。
pub fn render_change_reminder(added: &[&str], removed: &[&str]) -> String {
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!("+{}", added.join("、+")));
    }
    if !removed.is_empty() {
        parts.push(format!("-{}", removed.join("、-")));
    }
    format!(
        "# Skills Changed\n技能集已变化：{}（用 skills_list 查看当前完整目录）",
        parts.join(" ")
    )
}

/// Digest emission state handle.
///
/// P2-1（2026-11-19）：持有 **per-session 技能名集快照**，
/// 供 [`DigestState::change_reminder`] 产出「技能集已变化：+x/-y」一行
/// 提醒。目录渲染本体不变——merged snapshot 每 build 重渲（见模块文档），
/// 快照只服务提醒行：
/// - 会话首见 = 落基线，None（首启不提醒，目录节首次出现已是足量信号）；
/// - 集相同 = None（零注入字节——cache 纪律的核心不变量）；
/// - 集变化 = Some(提醒行) + 更新快照。提醒只在变化轮出现，而那一轮目录
///   字节本来就变（零额外 cache 失效）；下一轮快照已更新，提醒消失，
///   merged 字节回到稳定形态。
///
/// `clear_all()` 清空快照 = 基线重锚（touch-driven 稀有事件；重锚窗口内
/// 的技能变更少一次提醒，可接受——指令链变更远比技能装卸频繁，不能为
/// 提醒保真反过来放大失效面）。
pub struct DigestState {
    seen: RwLock<HashMap<String, Vec<String>>>,
}

impl Default for DigestState {
    fn default() -> Self {
        Self::new()
    }
}

impl DigestState {
    pub fn new() -> Self {
        Self {
            seen: RwLock::new(HashMap::new()),
        }
    }

    /// Decide the context-digest message for this build. Under
    /// merged-snapshot semantics this ALWAYS re-emits (the injection is not
    /// persisted in history; every build must carry it, byte-identically
    /// when nothing changed — deterministic rendering preserves the provider
    /// prefix). Change REMINDERS live in [`DigestState::change_reminder`]
    /// (P2-1) — this method itself stays a passthrough.
    pub fn should_inject(&self, _session_key: &str, rendered: &str) -> Option<String> {
        Some(rendered.to_string())
    }

    /// P2-1 变更提醒判定（详见类型文档）。`names` 无需预排序——内部
    /// sort+dedup，调用方直接传 `list_skills()` 的名字序列即可。
    pub fn change_reminder(&self, session_key: &str, names: &[String]) -> Option<String> {
        let mut current = names.to_vec();
        current.sort();
        current.dedup();
        let mut seen = self.seen.write();
        match seen.get(session_key) {
            None => {
                // 首见：落基线，不提醒。
                seen.insert(session_key.to_string(), current);
                None
            }
            Some(prev) if prev == &current => None,
            Some(prev) => {
                let added: Vec<&str> = current
                    .iter()
                    .filter(|n| !prev.contains(*n))
                    .map(String::as_str)
                    .collect();
                let removed: Vec<&str> = prev
                    .iter()
                    .filter(|n| !current.contains(*n))
                    .map(String::as_str)
                    .collect();
                let msg = render_change_reminder(&added, &removed);
                seen.insert(session_key.to_string(), current);
                Some(msg)
            }
        }
    }

    /// touch-driven invalidation. The per-build re-render of section content
    /// needs no invalidation; this clears the P2-1 name-set snapshots
    /// (= baseline re-anchor — the next build after an instruction-chain
    /// touch re-baselines instead of diffing, see type doc). Kept for the
    /// `invalidate_context_digests` call-chain shape.
    pub fn clear_all(&self) {
        self.seen.write().clear();
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

#[cfg(test)]
mod tests;
