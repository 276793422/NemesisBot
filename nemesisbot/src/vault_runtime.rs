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
fn resolve_alias(path: &Path, alias: &str) -> Result<String, String> {
    let store = open_unlocked(path)?;
    store.get(alias).map_err(|e| {
        format!(
            "vault 别名 '{alias}' 解析失败: {e}——用 `nemesisbot vault set {alias}` 写入；\
             argon2 模式需 NEMESISBOT_VAULT_PASSPHRASE"
        )
    })
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
