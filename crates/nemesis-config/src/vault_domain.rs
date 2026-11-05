//! vault 别名域绑定（②，2026-11-02 三期）：alias → (工具,槽位) 白名单。
//!
//! 绑定配置落 `<workspace>/config/config.vault.json`（配置落 vault
//! 配置文件；config.cluster.json / config.forge.json 同族 sidecar 形态）。
//! 绑定是**策略元数据不是秘密**：明文 JSON，不进 vault.enc——可读、可手
//! 编、可 version control，泄密面为零（不含值）。
//!
//! 判定语义（dispatch 凭据闸 v2 臂，见 nemesis-agent tool_dispatch）：
//! - 域内（全部 ref 命中各自绑定）→ 免卡直过（绑定即预授权）；
//! - 出域（任一 ref 的别名有绑定但 (tool, field) 不命中）→ 审计拒绝 loud；
//! - 无绑定（任一 ref 的别名没配域）→ v1 审批卡路径（绑定 opt-in，存量零回归）。
//!
//! crate 归位：判定臂 always-on（不依赖 security feature / 审批通道），
//! 所以 loader 住本 crate（nemesis-agent 的非 cfg 依赖）；nemesis-security
//! 在 agent 是 optional，落那边会把策略绑死在审批面上。
//!
//! 损坏配置 fail-secure：load Err → 调用方 warn 后按无绑定兜底——绑定是
//! **松弛**机制（免卡），坏配置不得松弛任何调用（收紧方向）。

use std::collections::BTreeMap;
use std::path::Path;

/// 绑定配置文件相对 `<workspace>/config/` 的文件名。
pub const VAULT_DOMAIN_CONFIG_FILE: &str = "config.vault.json";

/// 单条绑定：允许别名在 `tool` 工具的 `slot` 槽位使用。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomainBinding {
    pub tool: String,
    pub slot: String,
}

/// 绑定配置（文件唯一顶层键 `bindings`；缺席 = 空绑定）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VaultDomainConfig {
    #[serde(default)]
    pub bindings: BTreeMap<String, Vec<DomainBinding>>,
}

/// 出域明细（拒绝串与审计 reason 的素材；值零参与——绑定与引用都是
/// 别名级元数据，不触 vault.enc）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainViolation {
    pub alias: String,
    pub field: String,
    /// 该别名已配置的允许域摘要（`tool:slot` 形态列表）。
    pub allowed: Vec<String>,
}

/// dispatch 凭据闸 v2 臂的三态判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainVerdict {
    /// refs 中没有任何别名配置了绑定 → v1 审批卡路径（存量行为）。
    NoBindings,
    /// 全部 ref 域内 → 免卡直过。
    AllInDomain,
    /// 任一 ref 出域 → 拒绝（硬策略优先于预授权优先于人工裁决）。
    OutOfDomain(Vec<DomainViolation>),
}

impl VaultDomainConfig {
    /// 从磁盘加载。文件缺席 → `Ok(None)`；存在但损坏/非法 → `Err`（调用方
    /// warn + 按 None 兜底，fail-secure）。
    pub fn load_at(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("绑定配置读取失败（{}）: {e}", path.display()))?;
        let cfg: VaultDomainConfig = serde_json::from_str(&raw)
            .map_err(|e| format!("绑定配置解析失败（{}）: {e}", path.display()))?;
        Ok(Some(cfg))
    }

    /// 判定（纯函数）。`refs` = `(字段路径, 别名)` 列表
    /// （`find_declared_vault_references` 产出形态）。
    pub fn evaluate(&self, tool: &str, refs: &[(String, String)]) -> DomainVerdict {
        // 空 refs = 无引用可判（闸对空 refs 本就直落）——契约恒 NoBindings。
        if refs.is_empty() {
            return DomainVerdict::NoBindings;
        }
        let mut saw_unbound = false;
        let mut violations = Vec::new();
        for (field, alias) in refs {
            match self.bindings.get(alias) {
                None => saw_unbound = true,
                Some(entries) if entries.is_empty() => saw_unbound = true,
                Some(entries) => {
                    let hit = entries
                        .iter()
                        .any(|b| b.tool == tool && slot_covers(&b.slot, field));
                    if !hit {
                        violations.push(DomainViolation {
                            alias: alias.clone(),
                            field: field.clone(),
                            allowed: entries
                                .iter()
                                .map(|b| format!("{}:{}", b.tool, b.slot))
                                .collect(),
                        });
                    }
                }
            }
        }
        if !violations.is_empty() {
            return DomainVerdict::OutOfDomain(violations);
        }
        if saw_unbound {
            DomainVerdict::NoBindings
        } else {
            DomainVerdict::AllInDomain
        }
    }
}

/// slot 层级匹配（设计）：绑定 `slot` 覆盖字段路径 `field` 当且仅当
/// `field == slot`（顶槽本体 / 精确叶）或 `field` 是 `slot` 的直接层级后代
/// （`{slot}.叶`——顶槽绑定盖全部叶子）。不做通配符语法（层级语义已覆盖）。
fn slot_covers(slot: &str, field: &str) -> bool {
    if slot.is_empty() {
        return false;
    }
    field == slot
        || field
            .strip_prefix(slot)
            .and_then(|r| r.strip_prefix('.'))
            .is_some()
}

/// 绑定文件在 config 目录下的标准路径（`config_path.parent` 消费形态）。
pub fn vault_domain_config_path_in(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(VAULT_DOMAIN_CONFIG_FILE)
}

#[cfg(test)]
mod tests;
