//! 审批 pattern 记忆（F3，devtool-upgrade 阶段 5，「总是允许」语义）。
//!
//! 「总是允许」规则表：`{op, pattern, action: "allow"}` 持久化在
//! `<workspace>/config/approval_rules.json`（[`resolve_rules_path`]）。
//! auditor 命中 require_approval 时先查本表，命中（且层级安全门放行）则
//! 自动放行并在审计事件标注 `auto_by_rule`；M7 审批卡「总是允许」按钮
//! respond 时写入规则。读取走 `nemesis_config::HotReloader` 热载（模式照
//! commands）——本模块只提供加载/保存/匹配纯函数，装载与消费在
//! auditor（查）与 web_approval（写）。
//!
//! pattern 形态：exec 类操作（process_exec/process_spawn）用 B5
//! `reduce_command` 归约前缀（`cargo test *`），其余操作用完整 target
//! 精确匹配。层级安全：**CRITICAL 操作规则不生效**（永远人工）——唯一
//! 豁免是 `process_exec`（前缀 pattern 本身就是作用域，这正是本机制的
//! 存在意义；`cargo test` 批准 always → `cargo test --release` 自动放行、
//! `cargo publish` 不放行，即 F4 验收）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::command_arity::reduce_command;

/// 一条审批记忆规则。`action` 目前只有 `allow`（deny 没有记忆意义——
/// deny 的记忆就是再次弹卡）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApprovalRule {
    /// 操作类型（`process_exec` / `file_write` / ...，与
    /// `OperationRequest.op_type` 字符串一致）。
    pub op: String,
    /// 匹配 pattern：exec 类为 B5 归约前缀（`cargo test *`），其余为完整
    /// target 精确串。
    pub pattern: String,
    /// 动作，v1 恒为 `allow`。
    pub action: String,
    /// 写入时刻（RFC3339，诊断用）。
    pub created_at: String,
}

/// exec 类操作：target 是命令行，pattern 走 B5 归约。
fn is_command_op(op: &str) -> bool {
    op == "process_exec" || op == "process_spawn"
}

/// 由操作 + target 生成规则 pattern（单一真相源：写入侧与展示侧都用它）。
///
/// exec 类 → B5 归约前缀且**恒带 ` *`**（`cargo test --release` →
/// `cargo test *`；`cargo publish` 归约后无参数也补 ` *`——「总是允许」
/// 的意图是「这条命令 + 任意参数」，退化为精确串就丢了 F4 前缀粒度；
/// [`matches_rule`] 对带 ` *` 的 pattern 做 token 前缀匹配，裸命令照常
/// 命中，无回退损失）。归约结果为空（target 全是 env 前缀）→ 空串
/// （不构成规则——空 token 前缀若带 ` *` 会变成全放行，绝不产生）。
/// 其余 → 完整 target（精确匹配）；空 target → 空串（不构成规则）。
pub fn pattern_for(op: &str, target: &str) -> String {
    let target = target.trim();
    if target.is_empty() {
        return String::new();
    }
    if is_command_op(op) {
        let reduced = reduce_command(target);
        if reduced.is_empty() {
            return String::new();
        }
        if reduced.ends_with(" *") {
            reduced
        } else {
            format!("{} *", reduced)
        }
    } else {
        target.to_string()
    }
}

/// 层级安全门：该 (op, risk) 组合是否允许规则自动放行/写规则。
///
/// 非 CRITICAL 一律允许；CRITICAL 只有 `process_exec` 豁免（前缀 pattern
/// 自带作用域）。`risk` 是 `DangerLevel` 的字符串形态（`CRITICAL` 等），
/// 与审批卡/pending 条目里的字符串同源。
pub fn rule_permitted_for(op: &str, risk: &str) -> bool {
    risk != "CRITICAL" || is_command_op(op)
}

/// 规则是否命中 (op, target)。
///
/// - op 必须相等；
/// - pattern 以 ` *` 结尾 → token 前缀匹配（`cargo test *` 命中
///   `cargo test` 与 `cargo test --release`，不命中 `cargo publish`）；
/// - 其余 → target 精确相等；
/// - 手写裸 `*` pattern 永不命中（防规则文件被改成全放行）。
pub fn matches_rule(rule_op: &str, rule_pattern: &str, op: &str, target: &str) -> bool {
    if rule_op != op || rule_pattern.is_empty() || target.is_empty() {
        return false;
    }
    if rule_pattern == "*" {
        return false;
    }
    match rule_pattern.strip_suffix(" *") {
        Some(prefix) => {
            // 空 token 前缀（手写退化 pattern ` *`）= 全放行，与裸 `*` 同罪。
            let prefix_tokens = crate::command_arity::tokenize_command(prefix);
            if prefix_tokens.is_empty() {
                return false;
            }
            let target_tokens = crate::command_arity::tokenize_command(target);
            target_tokens.len() >= prefix_tokens.len()
                && target_tokens
                    .iter()
                    .zip(prefix_tokens.iter())
                    .all(|(t, p)| t == p)
        }
        None => rule_pattern == target,
    }
}

/// 查第一条可自动放行的命中规则（层级安全门不过 → None）。
pub fn find_auto_allow_rule<'a>(
    rules: &'a [ApprovalRule],
    op: &str,
    target: &str,
    risk: &str,
) -> Option<&'a ApprovalRule> {
    if !rule_permitted_for(op, risk) {
        return None;
    }
    rules
        .iter()
        .find(|r| r.action == "allow" && matches_rule(&r.op, &r.pattern, op, target))
}

/// upsert 一条规则：同 (op, pattern) 已存在则只刷新 created_at。
/// 返回是否发生变更（新插入或刷新）。
pub fn upsert_rule(rules: &mut Vec<ApprovalRule>, op: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    let now = chrono::Local::now().to_rfc3339();
    if let Some(existing) = rules
        .iter_mut()
        .find(|r| r.op == op && r.pattern == pattern)
    {
        if existing.created_at != now {
            existing.created_at = now;
        }
        return true;
    }
    rules.push(ApprovalRule {
        op: op.to_string(),
        pattern: pattern.to_string(),
        action: "allow".to_string(),
        created_at: now,
    });
    true
}

/// `<workspace>/config/approval_rules.json`——路径真相源在 nemesis-path
/// （[`nemesis_path::resolve_approval_rules_path_in_workspace`]，CLI/WSAPI
/// 消费方不经 security feature 也能解析同一路径）。
pub fn resolve_rules_path_in_workspace(workspace: &Path) -> std::path::PathBuf {
    nemesis_path::resolve_approval_rules_path_in_workspace(workspace)
}

/// 热载加载函数（HotReloader `load` 签名）：文件缺失 → 空表；解析失败 →
/// 空表 + warn（HotReloader 从不 surface 加载错误，坏文件不炸网关）。
pub fn load_rules(path: &Path) -> Vec<ApprovalRule> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(), // 缺失 = 从未写过，常态
    };
    match serde_json::from_str::<Vec<ApprovalRule>>(&raw) {
        Ok(rules) => rules,
        Err(e) => {
            tracing::warn!(
                "[ApprovalRules] malformed rules file {}: {} — treating as empty",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// 保存规则表到磁盘（pretty JSON；目录不存在自动创建）。
pub fn save_rules(path: &Path, rules: &[ApprovalRule]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create rules dir {}: {}", parent.display(), e))?;
    }
    let raw = serde_json::to_string_pretty(rules).map_err(|e| format!("serialize rules: {}", e))?;
    std::fs::write(path, raw).map_err(|e| format!("write rules {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests;
