//! 进程级 vault 运行时（P0 vault 计划 B1，2026-09-22 计划 §2.2）。
//!
//! 依赖环约束：nemesis-config 不能依赖 nemesis-security，所以 `vault:<alias>`
//! 解析器由本模块在 nemesisbot 启动路径注入 `set_global_vault_resolver`。
//! 安装点与 `set_global_credentials_path` 一一对应（gateway / run / agent /
//! acp / eval-worker / skills / credentials CLI），保证任意入口的消费点
//! （provider api_key、channel token、cluster token、MCP env/headers）
//! 都能解析。
//!
//! 语义与 `env:` / `yaml:` 对齐：**每次解析现开 vault 文件、无缓存**——
//! `vault set`/轮换在进程运行中经 CLI 落盘后，下一次解析即生效。文件是
//! KB 级、AES-GCM 解锁微秒级，相比每次解析所服务的 LLM/网络调用可忽略。
//! DPAPI 模式开箱即解（当前用户）；argon2 模式从 `NEMESISBOT_VAULT_PASSPHRASE`
//! 取口令（与 `vault` CLI 同源），缺失则报锁定。vault 缺失/锁定不让进程
//! 起不来，只让**引用它的那一次解析** fail loud。

use std::path::Path;

/// 在 nemesisbot 各启动路径调用（与 set_global_credentials_path 同点）。
/// 只注册解析器闭包（捕获 vault 路径），不做 IO；后装覆盖先装。
pub fn install(home: &Path) {
    let ws = crate::common::workspace_path(home);
    let path = nemesis_path::resolve_vault_path_in_workspace(&ws);
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(move |alias| {
        resolve_alias(&path, alias)
    }));
}

/// `vault:<alias>` → secret。错误带补救指引（vault_ref 约定：fail loud，
/// 绝不静默降级）。
///
/// pub 供同 crate 内非 `vault:` 标准形态的别名消费方复用（WASM 插件桥
/// 的 x-secret 解析——到达时已剥 `vault:` 前缀）。
pub fn resolve_alias_at(path: &Path, alias: &str) -> Result<String, String> {
    let store = open_unlocked(path)?;
    store.get(alias).map_err(|e| {
        format!(
            "vault 别名 '{alias}' 解析失败: {e}——用 `nemesisbot vault set {alias}` 写入；\
             argon2 模式需 NEMESISBOT_VAULT_PASSPHRASE"
        )
    })
}

/// 全局解析器闭包入口（`vault:<alias>` 全前缀形态）。
fn resolve_alias(path: &Path, alias: &str) -> Result<String, String> {
    resolve_alias_at(path, alias.strip_prefix("vault:").unwrap_or(alias))
}

/// 打开并解锁 vault 的 pub 变体（P6-R2 agent 迁移的凭据导出用——迁移
/// 编排需要拿整个 [`VaultStore`] 做 export_entries，不是单别名读取）。
/// 语义与私有 [`open_unlocked`] 完全同源（现开现解锁、argon2 走
/// `NEMESISBOT_VAULT_PASSPHRASE`）。
pub fn open_unlocked_at(path: &Path) -> Result<nemesis_security::vault::VaultStore, String> {
    open_unlocked(path)
}

/// 打开（缺文件时按平台默认模式创建）并解锁 vault——自持写通道（WASM
/// 插件面板表单 secret-set）用。与 CLI `open_or_create`（commands/vault.rs）
/// 建库语义同源，但**不交互 prompt**（调用方在常驻进程内，读 stdin 是错的）：
/// DPAPI 免口令直接建；argon2id 只认 `NEMESISBOT_VAULT_PASSPHRASE`，缺失 =
/// 诚实 Err 带 CLI 一次性补救指引（`nemesisbot vault set <alias>` 建库）。
pub fn open_or_create_unlocked_at(
    path: &Path,
) -> Result<nemesis_security::vault::VaultStore, String> {
    use nemesis_security::vault::VaultMode;
    if path.exists() {
        return open_unlocked_at(path);
    }
    let mode = nemesis_security::vault::VaultStore::default_mode();
    let pw = match mode {
        VaultMode::Dpapi => None,
        VaultMode::Argon2id => match std::env::var("NEMESISBOT_VAULT_PASSPHRASE") {
            Ok(p) if !p.is_empty() => Some(p),
            _ => {
                return Err(format!(
                    "vault 文件不存在（{}）且 argon2id 模式无 NEMESISBOT_VAULT_PASSPHRASE——\
                     headless 进程不能交互输入口令；先跑一次 `nemesisbot vault set <alias>` 建库",
                    path.display()
                ));
            }
        },
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("vault 目录创建失败: {e}（{}）", dir.display()))?;
    }
    nemesis_security::vault::VaultStore::create(path, mode, pw.as_deref())
        .map_err(|e| format!("vault 创建失败: {e}（文件: {}）", path.display()))
}

/// 打开并解锁 vault（每次解析现开）。文件不存在是常见态（还没人 set 过），
/// 报错带创建指引。
fn open_unlocked(path: &Path) -> Result<nemesis_security::vault::VaultStore, String> {
    if !path.exists() {
        return Err(format!(
            "vault 文件不存在（{}）——用 `nemesisbot vault set <alias>` 创建",
            path.display()
        ));
    }
    let mut store = nemesis_security::vault::VaultStore::open(path)
        .map_err(|e| format!("vault 打开失败: {e}（文件: {}）", path.display()))?;
    if !store.is_unlocked() {
        let pw = std::env::var("NEMESISBOT_VAULT_PASSPHRASE")
            .ok()
            .filter(|p| !p.is_empty());
        match pw {
            Some(p) => store
                .unlock(&p)
                .map_err(|e| format!("vault 解锁失败: {e}（检查 NEMESISBOT_VAULT_PASSPHRASE）"))?,
            None => {
                return Err(format!(
                    "vault 已锁定（argon2 模式）——设置 NEMESISBOT_VAULT_PASSPHRASE 后重启进程\
                    （文件: {}）",
                    path.display()
                ));
            }
        }
    }
    Ok(store)
}

#[cfg(test)]
mod tests;
