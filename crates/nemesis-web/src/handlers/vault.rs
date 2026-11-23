//! Vault（凭据 vault）WSAPI — `vault.list` / `vault.set` / `vault.remove` /
//! `vault.status` / `vault.unlock`（2026-10-09 高优差距批次；配套
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
use std::sync::Arc;

pub struct VaultHandler;

/// vault 写路径互斥（RMW 串行锁）：`set`/`remove` 是
/// 「open→内存改→save 整文件」的读改写序列，而 WSAPI 每条消息独立
/// `tokio::spawn` 派发（websocket_handler dispatch）——并发 set∥set /
/// set∥remove 各持旧快照互相整文件覆盖，先落盘的条目**静默丢失**（无任何
/// 报错）。与 share.rs `SHARE_LOCK`（RMW 横扫同款）同一缺陷类，
/// vault 当时漏网。只罩进程内并发面；跨进程（CLI ∥ gateway）仍可能竞——
/// 与 share 面同一诚实边界（单用户操作速率，现实触发面即 Dashboard 双发/
/// 双标签页）。`list`/`status`/`unlock` 不落盘，不入锁。
pub(crate) static VAULT_IO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ── 凭据免打扰运行时态槽（2026-11-23 ABCD 批 A/B；security handler
// APPROVAL_AUTO 同款依赖方向：`CredentialAutoApproveState` 本体在
// nemesis-security，gateway 装配处（security_setup）注入，本 handler
// `credential_status`/`credential_set`/`credential_stats` 读写 +
// AgentLoop 侧经 getter 读回（agent_factory 三装配点）——统一定义 Arc；
// config.security.json 是持久真相源，本态是运行期投影）。
static CREDENTIAL_AUTO: std::sync::RwLock<
    Option<Arc<nemesis_security::credential_auto::CredentialAutoApproveState>>,
> = std::sync::RwLock::new(None);

/// gateway 装配处调用；未注入 = headless/降级形态，
/// `credential_status` 返回 `injected: false`（前端展示「未装配」不误判）。
pub fn install_credential_auto(
    state: Arc<nemesis_security::credential_auto::CredentialAutoApproveState>,
) {
    *CREDENTIAL_AUTO
        .write()
        .expect("credential auto state lock poisoned") = Some(state);
}

/// 全局槽只读访问（agent_factory 挂 AgentLoop 槽复用同一 Arc；
/// security handler `approval_auto_state` 同款）。未注入 = `None`。
pub fn credential_auto_state()
-> Option<Arc<nemesis_security::credential_auto::CredentialAutoApproveState>> {
    CREDENTIAL_AUTO
        .read()
        .expect("credential auto state lock poisoned")
        .clone()
}

#[async_trait::async_trait]
impl ModuleHandler for VaultHandler {
    fn module_name(&self) -> &str {
        "vault"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "set",
            "remove",
            "status",
            "unlock",
            "credential_status",
            "credential_set",
            "credential_stats",
            "bindings",
            "bind",
            "unbind",
        ]
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
                //（修复：原实现把口令检查放在开店之前，dpapi 下
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
            "credential_status" => match credential_auto_state() {
                Some(st) => {
                    let (auto, preauth, denied) = st.counts();
                    Ok(Some(serde_json::json!({
                        "injected": true,
                        "enabled": st.enabled(),
                        "auto_count": auto,
                        "preauth_count": preauth,
                        "denied_count": denied,
                    })))
                }
                None => Ok(Some(serde_json::json!({ "injected": false }))),
            },
            "credential_set" => {
                let data = data.ok_or("vault.credential_set requires data")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("vault.credential_set 缺 enabled 布尔")?;
                let ws = ctx
                    .workspace
                    .as_deref()
                    .ok_or("workspace not configured")?
                    .to_string();
                // 持久真相源写盘（security handler update_security_config
                // 同一事务入口；config.security.json approvals 段）。
                crate::handlers::security::update_security_config(&ws, |cfg| {
                    cfg.approvals.credential_auto_approve = enabled;
                    Ok(())
                })?;
                // 运行态热翻（BUG-20261120-027 模式：config 是持久真相源，
                // 这里只翻投影——闸下一次判定即读新值）。
                if let Some(st) = credential_auto_state() {
                    st.set_enabled(enabled);
                }
                tracing::info!("[Web/WSAPI] vault.credential_set enabled={enabled}");
                Ok(Some(
                    serde_json::json!({ "saved": true, "enabled": enabled }),
                ))
            }
            "credential_stats" => match credential_auto_state() {
                Some(st) => {
                    let (auto, preauth, denied) = st.counts();
                    let records: Vec<serde_json::Value> = st
                        .records_snapshot()
                        .into_iter()
                        .filter_map(|r| serde_json::to_value(&r).ok())
                        .collect();
                    Ok(Some(serde_json::json!({
                        "injected": true,
                        "enabled": st.enabled(),
                        "counters": {
                            "auto": auto,
                            "preauth": preauth,
                            "denied": denied,
                        },
                        "records": records,
                    })))
                }
                None => Ok(Some(serde_json::json!({ "injected": false }))),
            },
            "bindings" => {
                let path = vault_domain_config_path(ctx)?;
                let cfg = load_domain_cfg(&path)?;
                let bindings: serde_json::Map<String, serde_json::Value> = cfg
                    .bindings
                    .iter()
                    .map(|(alias, entries)| {
                        (
                            alias.clone(),
                            serde_json::json!(
                                entries
                                    .iter()
                                    .map(|b| serde_json::json!({"tool": b.tool, "slot": b.slot}))
                                    .collect::<Vec<_>>()
                            ),
                        )
                    })
                    .collect();
                Ok(Some(serde_json::json!({
                    "bindings": bindings,
                    "path": path.display().to_string(),
                })))
            }
            "bind" => {
                let data = data.ok_or("vault.bind requires data")?;
                let alias = str_field(&data, "alias")?;
                let tool = str_field(&data, "tool")?;
                let slot = str_field(&data, "slot")?;
                if alias.is_empty() || tool.is_empty() || slot.is_empty() {
                    return Err("vault.bind 的 alias/tool/slot 均为必填且非空".to_string());
                }
                let path = vault_domain_config_path(ctx)?;
                // RMW 串行锁（VAULT_IO_LOCK 注：config.vault.json 与
                // vault.enc 同目录同族 RMW 面，一并串行防并发覆盖）。
                let _io = VAULT_IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let mut cfg = load_domain_cfg(&path)?;
                let entries = cfg.bindings.entry(alias.clone()).or_default();
                let binding = nemesis_config::DomainBinding {
                    tool: tool.clone(),
                    slot: slot.clone(),
                };
                if entries.contains(&binding) {
                    return Ok(Some(serde_json::json!({
                        "bound": true, "idempotent": true, "alias": alias,
                        "tool": tool, "slot": slot, "count": entries.len(),
                    })));
                }
                entries.push(binding);
                let count = entries.len();
                save_domain_cfg(&path, &cfg)?;
                tracing::info!(
                    "[Web/WSAPI] vault.bind alias={alias} tool={tool} slot={slot} count={count}（值不落日志）"
                );
                Ok(Some(serde_json::json!({
                    "bound": true, "alias": alias, "tool": tool, "slot": slot, "count": count,
                })))
            }
            "unbind" => {
                let data = data.ok_or("vault.unbind requires data")?;
                let alias = str_field(&data, "alias")?;
                let tool = str_field(&data, "tool")?;
                let slot = str_field(&data, "slot")?;
                let path = vault_domain_config_path(ctx)?;
                let _io = VAULT_IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let mut cfg = load_domain_cfg(&path)?;
                let Some(entries) = cfg.bindings.get_mut(&alias) else {
                    return Err(format!("别名「{alias}」无任何绑定"));
                };
                let before = entries.len();
                entries.retain(|b| !(b.tool == tool && b.slot == slot));
                if entries.len() == before {
                    return Err(format!("绑定不存在: {alias} -> {tool}:{slot}"));
                }
                let count = entries.len();
                if count == 0 {
                    cfg.bindings.remove(&alias);
                }
                save_domain_cfg(&path, &cfg)?;
                tracing::info!(
                    "[Web/WSAPI] vault.unbind alias={alias} tool={tool} slot={slot} count={count}（值不落日志）"
                );
                Ok(Some(serde_json::json!({
                    "unbound": true, "alias": alias, "tool": tool, "slot": slot, "count": count,
                })))
            }
            _ => Err(format!("unknown command: vault.{cmd}")),
        }
    }
}

// 共享辅助（供 handler 与测试复用；passphrase 显式传参，测试不碰进程 env）

/// vault 文件路径 = `<workspace>/config/vault.enc`（nemesis-path 统一定义，
/// 与 CLI / 网关解析器同一条）。
pub(crate) fn vault_path(ctx: &RequestContext) -> Result<PathBuf, String> {
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

/// 域绑定配置路径 = `<workspace>/config/config.vault.json`（nemesis-path
/// workspace_config_dir + nemesis-config 文件名常量同一拼装公式——CLI
/// `vault bind` 写入侧同款；与 dispatch 闸读取侧（装配点注入路径）同点）。
fn vault_domain_config_path(ctx: &RequestContext) -> Result<PathBuf, String> {
    let ws = ctx
        .workspace
        .as_deref()
        .ok_or("workspace not configured")?
        .to_string();
    Ok(nemesis_path::workspace_config_dir(Path::new(&ws))
        .join(nemesis_config::VAULT_DOMAIN_CONFIG_FILE))
}

/// 读绑定配置（不存在 = 空配置）。损坏 loud 拒改（CLI 同口径：用户可手编
/// 资产，不静默重置——修复或删除后重试）。
fn load_domain_cfg(path: &Path) -> Result<nemesis_config::VaultDomainConfig, String> {
    match nemesis_config::VaultDomainConfig::load_at(path) {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Ok(Default::default()),
        Err(e) => Err(e),
    }
}

/// 写绑定配置（CLI save_domain_config 同款：pretty JSON + 原子写 0o600）。
fn save_domain_cfg(path: &Path, cfg: &nemesis_config::VaultDomainConfig) -> Result<(), String> {
    let out = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化绑定配置失败: {e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("创建目录失败: {}: {e}", dir.display()))?;
    }
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("写绑定配置失败（{}）: {e}", path.display()))
}

/// 打开已有 vault（不创建）。文件缺失 = 带创建指引的诚实错误。
pub(crate) fn open_store(path: &Path) -> Result<VaultStore, String> {
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
pub(crate) fn try_unlock(store: &mut VaultStore, passphrase: Option<&str>) -> Result<(), String> {
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
pub(crate) fn create_fresh(path: &Path, passphrase: Option<&str>) -> Result<VaultStore, String> {
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
