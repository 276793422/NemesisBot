//! `vault:<alias>` 引用解析钩子（P0 vault 计划 B1，2026-09-22 计划 §2.2）。
//!
//! 依赖环约束：vault 的加密实现住在 nemesis-security，而 nemesis-security
//! 依赖本 crate（nemesis-config）——所以本 crate **不能**反向依赖它。约定：
//!
//! - 本模块只持有前缀常量 + 全局解析器槽位（`set_global_vault_resolver`）；
//! - 上层装配（nemesisbot 启动路径，security feature）把"别名 → secret"
//!   的解析闭包注入进来；
//! - 所有 `vault:` 消费点（provider api_key、channel token、cluster token、
//!   MCP env/headers）经 [`resolve_vault_reference`] / [`resolve_secret_field`]
//!   走同一槽位。
//!
//! 语义与 `env:` / `yaml:` 一致：引用解析**每次现查、无缓存**（vault 轮换
//! 下次解析即生效）；解析失败 fail loud 带补救指引，绝不静默降级为空值。

use crate::{ConfigError, Result};
use parking_lot::RwLock;
use std::sync::Arc;

/// `vault:` 引用前缀。配置中写作 `vault:<alias>`（如 `vault:openai-main`）。
pub const VAULT_PREFIX: &str = "vault:";

/// vault 别名解析函数：alias -> secret，或人类可读错误（别名不存在 /
/// vault 锁定 / 打不开）。由上层装配注入；实现须无阻塞交互（无 TTY 提示）。
pub type VaultResolver = Arc<dyn Fn(&str) -> std::result::Result<String, String> + Send + Sync>;

static GLOBAL_VAULT_RESOLVER: RwLock<Option<VaultResolver>> = RwLock::new(None);

/// 安装全局 vault 解析器（幂等：后装覆盖先装）。测试用
/// [`clear_global_vault_resolver`] 复位。
pub fn set_global_vault_resolver(resolver: VaultResolver) {
    *GLOBAL_VAULT_RESOLVER.write() = Some(resolver);
}

/// Reset the global resolver (test hygiene, mirrors credentials module).
pub fn clear_global_vault_resolver() {
    *GLOBAL_VAULT_RESOLVER.write() = None;
}

/// 当前全局解析器（诊断/测试用）。
pub fn global_vault_resolver() -> Option<VaultResolver> {
    GLOBAL_VAULT_RESOLVER.read().clone()
}

/// 若 `value` 是 `vault:` 引用则解析之。
///
/// 返回 `None` = 不是 vault 引用（前缀不相交，调用方继续自己的链路）；
/// `Some(Err)` = 是引用但解析失败（解析器未安装 / 别名不存在 / vault
/// 锁定）——调用方必须把错误向上传播，不得静默当字面量处理。
pub fn resolve_vault_reference(value: &str) -> Option<std::result::Result<String, String>> {
    let alias = value.strip_prefix(VAULT_PREFIX)?;
    if alias.is_empty() {
        return Some(Err(
            "vault: 引用别名为空——写作 vault:<alias>（如 vault:openai-main），\
             或用 `nemesisbot vault set <alias>` 先写入"
                .to_string(),
        ));
    }
    let resolver = global_vault_resolver();
    match resolver {
        Some(r) => Some(r(alias)),
        None => Some(Err(format!(
            "检测到 vault:{alias} 引用，但本进程未安装 vault 解析器\
             （security feature 未启用，或非完整 nemesisbot 构建）"
        ))),
    }
}

/// `value` 是否为整值秘密引用（`env:` / `yaml:` / `vault:` 前缀）。
///
/// 前缀清单的唯一判定点：新增前缀时只改这里与 [`resolve_secret_field`]，
/// 消费方（如工作流模板层，2026-09-23 计划类 B）以本函数做整值识别，
/// 不自行维护前缀列表。
pub fn is_secret_reference(value: &str) -> bool {
    value.starts_with("env:") || value.starts_with("yaml:") || value.starts_with(VAULT_PREFIX)
}

/// 通用秘密字段解析（P0 B3）：channel token / cluster token / MCP
/// env、headers 等字段共用。前缀链与 `resolve_api_key_value` 对齐：
/// `env:VAR` > `yaml:<alias>` > `vault:<alias>` > 字面量（向后兼容）。
///
/// `field_for_error` 是字段上下文（如 "channels.telegram.token"），仅进
/// 错误消息，不参与解析。
pub fn resolve_secret_field(raw: &str, field_for_error: &str) -> Result<String> {
    if let Some(var) = raw.strip_prefix("env:") {
        if var.is_empty() {
            return Err(ConfigError::Validation(format!(
                "{field_for_error}: env: 引用变量名为空——写作 env:VAR_NAME 或直接填字面量"
            )));
        }
        return match std::env::var(var) {
            Ok(v) if !v.is_empty() => Ok(v),
            Ok(_) => Err(ConfigError::Validation(format!(
                "{field_for_error}: 环境变量 '{var}' 已设置但为空"
            ))),
            Err(_) => Err(ConfigError::Validation(format!(
                "{field_for_error}: 环境变量 '{var}' 未设置"
            ))),
        };
    }
    if let Some(alias) = raw.strip_prefix("yaml:") {
        return crate::credentials::resolve_yaml_reference(alias, field_for_error);
    }
    if let Some(resolved) = resolve_vault_reference(raw) {
        return resolved
            .map_err(|msg| ConfigError::Validation(format!("{field_for_error}: {msg}")));
    }
    // 字面量原样通过（向后兼容：现有明文配置继续工作）。
    Ok(raw.to_string())
}

#[cfg(test)]
mod tests;
