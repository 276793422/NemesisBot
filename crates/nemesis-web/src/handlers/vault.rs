//! Vault（凭据 vault）WSAPI — `vault.list` / `vault.set` / `vault.remove` /
//! `vault.status` / `vault.unlock`（S3c，2026-10-09 高优差距批次；配套 S3a
//! 的 `vault:` 别名注入，让 Dashboard 能管理模型/通道凭据的别名）。
//!
//! 语义与 `nemesisbot vault` CLI（nemesisbot/src/commands/vault.rs）同源：
//! - **永不显示值**——list/status 只含别名与元数据；刻意不提供 `vault.get`
//!   （CLI 同样不提供）。secret 只经 `vault.set` 单向写入。
//! - **每次请求现开 vault 文件**（与网关解析器 vault_runtime 同模式，无
//!   长驻句柄）——CLI `vault set`/轮换落盘后下一次请求即见新状态。
//! - argon2 模式解锁口令取 `NEMESISBOT_VAULT_PASSPHRASE`（与 CLI/解析器
//!   同源）；缺失/错误 = 诚实 locked 错误，不猜不崩。
//! - 覆盖（轮换）需显式 `force:true`——WS 无交互终端，前端先弹确认框再
//!   带参重发（镜像 CLI 的 confirm/--force 纪律）。
//!
//! 值的传输面：`vault.set` 的 value 走 WS 请求帧（Dashboard 是登录后受信
//! 级面，PTY 先例）。handler 侧**绝不把 value 写进日志/错误**——错误文案
//! 只回别名；响应 JSON 也只回别名与 rotated 标记。
//!
//! vault 文件缺失的语义：`list`/`status` 诚实报 `exists:false`（不创建）；
//! `set` 允许创建（与 CLI open_or_create 同）；`remove` 诚实报错（对不存在
//! 的文件做删除是调用方 bug，不悄悄造一个空 vault）。
//!
//! `vault.unlock` 是**解锁测试**：每次请求现开 vault（无长驻句柄），解锁
//! 态不跨请求存留——本命令的意义是让 Dashboard 在 gateway 未带口令环境
//! 变量启动时，仍能用用户显式输入的口令验证能否解锁（argon2 模式）。
//! 口令只在本请求内使用，不落日志、不写盘、不进响应。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_path::resolve_vault_path_in_workspace;
use nemesis_security::vault::{VaultMode, VaultStore};
use std::path::{Path, PathBuf};

pub struct VaultHandler;

/// vault 写路径互斥（RMW 串行锁，2026-10-24）：`set`/`remove` 是
/// 「open→内存改→save 整文件」的读改写序列，而 WSAPI 每条消息独立
/// `tokio::spawn` 派发（websocket_handler dispatch）——并发 set∥set /
/// set∥remove 各持旧快照互相整文件覆盖，先落盘的条目**静默丢失**（无任何
/// 报错）。与 share.rs `SHARE_LOCK`（2026-10-13 RMW 横扫同款）同一缺陷类，
/// vault 当时漏网。只罩进程内并发面；跨进程（CLI ∥ gateway）仍可能竞——
/// 与 share 面同一诚实边界（单用户操作速率，现实触发面即 Dashboard 双发/
/// 双标签页）。`list`/`status`/`unlock` 不落盘，不入锁。
static VAULT_IO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[async_trait::async_trait]
impl ModuleHandler for VaultHandler {
    fn module_name(&self) -> &str {
        "vault"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "set", "remove", "status", "unlock"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // 口令来源与 CLI/网关解析器同源（进程 env）；显式传参进各辅助函数，
        // 测试可注入固定口令不碰进程 env。
        let passphrase = std::env::var("NEMESISBOT_VAULT_PASSPHRASE")
            .ok()
            .filter(|p| !p.is_empty());
        match cmd {
            "status" => {
                let path = vault_path(ctx)?;
                if !path.exists() {
                    return Ok(Some(serde_json::json!({ "exists": false })));
                }
                let mut store = open_store(&path)?;
                let unlocked = try_unlock(&mut store, passphrase.as_deref()).is_ok();
                Ok(Some(serde_json::json!({
                    "exists": true,
                    "mode": store.mode().to_string(),
                    "unlocked": unlocked,
                    "alias_count": store.aliases().len(),
                    "path": path.display().to_string(),
                })))
            }
            "list" => {
                let path = vault_path(ctx)?;
                if !path.exists() {
                    return Ok(Some(
                        serde_json::json!({ "exists": false, "unlocked": false, "entries": [] }),
                    ));
                }
                let mut store = open_store(&path)?;
                // 元数据是明文，锁定也能列；仍尽量解锁，把真实解锁状态报给
                // 前端（锁定提示横幅用）。
                let unlocked = try_unlock(&mut store, passphrase.as_deref()).is_ok();
                let entries: Vec<serde_json::Value> = store
                    .list()
                    .into_iter()
                    .map(|l| {
                        serde_json::json!({
                            "alias": l.alias,
                            "domain": l.meta.domain,
                            "description": l.meta.description,
                            "created_at": l.meta.created_at,
                            "rotated_at": l.meta.rotated_at,
                        })
                    })
                    .collect();
                Ok(Some(serde_json::json!({
                    "exists": true,
                    "unlocked": unlocked,
                    "entries": entries,
                })))
            }
            "set" => {
                let data = data.ok_or("vault.set requires data")?;
                let alias = str_field(&data, "alias")?;
                let value = str_field(&data, "value")?;
                if value.is_empty() {
                    return Err("secret 为空，已取消".to_string());
                }
                let domain = data
                    .get("domain")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let description = data
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let force = data.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

                let path = vault_path(ctx)?;
                // RMW 串行锁：罩 open→set→save 全序列（VAULT_IO_LOCK 注）。
                let _io = VAULT_IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let mut store = if path.exists() {
                    // 写路径必须真解锁——set 需要 DEK；锁死 = 诚实报错带指引。
                    let mut s = open_store(&path)?;
                    try_unlock(&mut s, passphrase.as_deref())?;
                    s
                } else {
                    create_fresh(&path, passphrase.as_deref())?
                };
                let existed = store.aliases().iter().any(|a| a == &alias);
                if existed && !force {
                    return Err(format!(
                        "别名「{alias}」已存在。覆盖（轮换）不可撤销，请确认后带 force:true 重发"
                    ));
                }
                store
                    .set(&alias, &value, &domain, &description)
                    .map_err(|e| format!("vault 写入失败: {e}"))?;
                store
                    .save()
                    .map_err(|e| format!("vault 落盘失败: {e}（文件: {}）", path.display()))?;
                tracing::info!(
                    "[Web/WSAPI] vault.set alias={alias} rotated={existed}（值不落日志）"
                );
                Ok(Some(
                    serde_json::json!({ "alias": alias, "rotated": existed }),
                ))
            }
            "remove" => {
                let data = data.ok_or("vault.remove requires data")?;
                let alias = str_field(&data, "alias")?;
                let path = vault_path(ctx)?;
                if !path.exists() {
                    return Err(format!(
                        "vault 文件不存在（{}）——没有可删除的条目",
                        path.display()
                    ));
                }
                // RMW 串行锁：罩 open→remove→save 全序列（VAULT_IO_LOCK 注）。
                let _io = VAULT_IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let mut store = open_store(&path)?;
                // remove 只动 BTreeMap + 落盘，不需要 DEK；锁定也可删。
                let removed = store
                    .remove(&alias)
                    .map_err(|e| format!("vault 删除失败: {e}"))?;
                if !removed {
                    return Err(format!("别名不存在: {alias}"));
                }
                store
                    .save()
                    .map_err(|e| format!("vault 落盘失败: {e}（文件: {}）", path.display()))?;
                tracing::info!("[Web/WSAPI] vault.remove alias={alias}");
                Ok(Some(serde_json::json!({ "removed": true, "alias": alias })))
            }
            "unlock" => {
                // 解锁测试：显式口令优先（Dashboard 输入框），缺省回退进程
                // env。解锁态不跨请求存留（现开现抛），这里只回答「这把口
                // 令能不能开」——所以成功与否都只回布尔/错误，口令不落日志。
                // 无口令请求：dpapi 模式（Windows 默认，口令不参与）open 即
                // 解锁 = 恒真；argon2 模式没口令就无从验证 = 诚实报错
                //（2026-10-13 修复：原实现把口令检查放在开店之前，dpapi 下
                // 无口令 unlock 永远报「未提供口令」，与模式语义矛盾）。
                let pw = data
                    .as_ref()
                    .and_then(|d| d.get("passphrase"))
                    .and_then(|v| v.as_str())
                    .filter(|p| !p.is_empty())
                    .map(str::to_string)
                    .or(passphrase);
                let path = vault_path(ctx)?;
                let mut store = open_store(&path)?;
                let mode = store.mode().to_string();
                if pw.is_none() && !store.is_unlocked() {
                    return Err(
                        "未提供口令：请求带 passphrase，或设置 NEMESISBOT_VAULT_PASSPHRASE"
                            .to_string(),
                    );
                }
                try_unlock(&mut store, pw.as_deref())?;
                tracing::info!("[Web/WSAPI] vault.unlock ok（口令不落日志）");
                Ok(Some(serde_json::json!({ "unlocked": true, "mode": mode })))
            }
            _ => Err(format!("unknown command: vault.{cmd}")),
        }
    }
}

// ---------------------------------------------------------------------------
// 共享辅助（供 handler 与测试复用；passphrase 显式传参，测试不碰进程 env）
// ---------------------------------------------------------------------------

/// vault 文件路径 = `<workspace>/config/vault.enc`（nemesis-path 单一真相源，
/// 与 CLI / 网关解析器同一条）。
fn vault_path(ctx: &RequestContext) -> Result<PathBuf, String> {
    let ws = ctx
        .workspace
        .as_deref()
        .ok_or("workspace not configured")?
        .to_string();
    Ok(resolve_vault_path_in_workspace(Path::new(&ws)))
}

fn str_field(data: &serde_json::Value, key: &str) -> Result<String, String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| format!("vault.{key} 缺失或非字符串（{key} 为必填）"))
}

/// 打开已有 vault（不创建）。文件缺失 = 带创建指引的诚实错误。
fn open_store(path: &Path) -> Result<VaultStore, String> {
    if !path.exists() {
        return Err(format!(
            "vault 文件不存在（{}）——写入一条即可创建",
            path.display()
        ));
    }
    VaultStore::open(path).map_err(|e| format!("vault 打开失败: {e}（文件: {}）", path.display()))
}

/// 尽量解锁（dpapi 模式 open 即解锁；argon2 模式用给定口令）。失败 = 诚实
/// 错误（含 env 变量名指引），由调用方决定是否致命。
fn try_unlock(store: &mut VaultStore, passphrase: Option<&str>) -> Result<(), String> {
    if store.is_unlocked() {
        return Ok(());
    }
    match passphrase {
        Some(pw) => store
            .unlock(pw)
            .map_err(|e| format!("vault 解锁失败: {e}（检查 NEMESISBOT_VAULT_PASSPHRASE）")),
        None => Err(
            "vault 已锁定（argon2 模式）——设置 NEMESISBOT_VAULT_PASSPHRASE 后重启进程".to_string(),
        ),
    }
}

/// 新建 vault（指定模式；argon2id 需要口令在场，缺失 = 诚实报错不造一个
/// 永远解不开的空文件）。
fn create_fresh(path: &Path, passphrase: Option<&str>) -> Result<VaultStore, String> {
    let mode = VaultStore::default_mode();
    if mode == VaultMode::Argon2id && passphrase.is_none() {
        return Err(format!(
            "vault 文件不存在（{}）且平台默认模式为 argon2id：创建需要 \
             NEMESISBOT_VAULT_PASSPHRASE 在场",
            path.display()
        ));
    }
    VaultStore::create(path, mode, passphrase)
        .map_err(|e| format!("创建 vault 失败: {e}（文件: {}）", path.display()))
}

#[cfg(test)]
mod tests;
