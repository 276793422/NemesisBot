//! L9 selfupdate WSAPI — `selfupdate.check` / `selfupdate.apply` /
//! `selfupdate.status`。
//!
//! manager 槽（`set_manager`，gateway run_runtime 审批装配点注入）与启动身份
//! 槽（`set_boot_identity`：version + 短 commit + workspace）都是 OnceLock
//! 静态——handlers 模块无状态、槽未装配时命令诚实报「未装配」（plugins_wasm
//! installer 槽同款模式）。check/status/journal 只依赖身份槽（无 feature）；
//! apply 需要 `selfupdate` feature（manager 类型本体）+ gateway 装配，双闸
//! 诚实降级。

use std::sync::OnceLock;

use crate::ws_router::{ModuleHandler, RequestContext};

/// 启动身份槽（version, 短 commit, workspace 绝对路径）——gateway 启动注入。
/// 空槽 = headless/早期，命令诚实报身份未知。
struct BootIdentity {
    version: String,
    commit: String,
    workspace: std::path::PathBuf,
}

static BOOT_IDENTITY: OnceLock<BootIdentity> = OnceLock::new();

/// gateway 装配点：注入启动身份。
pub fn set_boot_identity(version: &str, commit: &str, workspace: &std::path::Path) {
    let _ = BOOT_IDENTITY.set(BootIdentity {
        version: version.to_string(),
        commit: commit.to_string(),
        workspace: workspace.to_path_buf(),
    });
}

fn boot_identity() -> Option<&'static BootIdentity> {
    BOOT_IDENTITY.get()
}

// manager 槽（selfupdate feature 门控——类型本体随 feature 进出）。
#[cfg(feature = "selfupdate")]
static MANAGER: OnceLock<std::sync::Arc<crate::selfupdate::SelfUpdateManager>> = OnceLock::new();

/// gateway 装配点：注入 apply 编排器（首个生效——agent 重启不重装）。
#[cfg(feature = "selfupdate")]
pub fn set_manager(m: std::sync::Arc<crate::selfupdate::SelfUpdateManager>) {
    let _ = MANAGER.set(m);
}

/// 槽读取（gateway 内部命令监听臂与 WSAPI apply 共用；None = 未装配）。
#[cfg(feature = "selfupdate")]
pub fn manager() -> Option<std::sync::Arc<crate::selfupdate::SelfUpdateManager>> {
    MANAGER.get().cloned()
}

pub struct SelfupdateHandler;

#[async_trait::async_trait]
impl ModuleHandler for SelfupdateHandler {
    fn module_name(&self) -> &str {
        "selfupdate"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["check", "apply", "status"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            // 只读检查：Release 最新产物 vs 当前 commit 身份。网络失败诚实报。
            "check" => {
                let ident = boot_identity()
                    .map(|b| (b.version.clone(), b.commit.clone()))
                    .unwrap_or_default();
                let c = crate::selfupdate::check(&ident.0, &ident.1).await?;
                Ok(Some(serde_json::to_value(&c).map_err(|e| e.to_string())?))
            }
            // 九步 apply：受理即回（下载 + 审批卡 + staging 全程分钟级，不能
            // 占住 WSAPI 响应）；进度经审批 SSE / tracing / 审计链可见。
            "apply" => {
                let allow_downgrade = data
                    .as_ref()
                    .and_then(|d| d.get("allow_downgrade"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                #[cfg(feature = "selfupdate")]
                {
                    let mgr = manager().ok_or_else(|| {
                        "selfupdate 子系统未装配（gateway 装配未完成）".to_string()
                    })?;
                    let (version, commit) = boot_identity()
                        .map(|b| (b.version.clone(), b.commit.clone()))
                        .unwrap_or_default();
                    tokio::spawn(async move {
                        match mgr.apply(&version, &commit, allow_downgrade).await {
                            Ok(msg) => tracing::info!("[SelfUpdate] {msg}"),
                            Err(e) => tracing::warn!("[SelfUpdate] apply 失败：{e}"),
                        }
                    });
                    Ok(Some(serde_json::json!({
                        "accepted": true,
                        "note": "apply 已受理：审批卡经 SSE 下发；结果看 gateway 日志与审计链（决策流）"
                    })))
                }
                #[cfg(not(feature = "selfupdate"))]
                {
                    let _ = allow_downgrade;
                    Err("selfupdate 通道未编译（构建未带 selfupdate feature）；\
                         请手动下载 Release 包替换"
                        .to_string())
                }
            }
            // 状态查询：启动身份 + journal 相位 + 保险丝用时 + manager 在场。
            "status" => {
                let ident = boot_identity();
                let (version, commit) = ident
                    .map(|b| (b.version.clone(), b.commit.clone()))
                    .unwrap_or_default();
                let (journal_phase, fuse_elapsed_ms) = match ident {
                    Some(b) => (
                        crate::selfupdate::load_journal(&b.workspace).map(|j| j.phase),
                        crate::selfupdate::last_attempt_ms(&b.workspace)
                            .map(|last| crate::selfupdate::now_ms().saturating_sub(last)),
                    ),
                    None => (None, None),
                };
                #[cfg(feature = "selfupdate")]
                let wired = manager().is_some();
                #[cfg(not(feature = "selfupdate"))]
                let wired = false;
                Ok(Some(serde_json::json!({
                    "version": version,
                    "commit": commit,
                    "commit_known": !commit.is_empty(),
                    "asset": crate::selfupdate::TARGET_ASSET,
                    "wired": wired,
                    "journal_phase": journal_phase,
                    "fuse_elapsed_ms": fuse_elapsed_ms,
                    "fuse_window_ms": crate::selfupdate::APPLY_FUSE_MILLIS,
                })))
            }
            _ => Err(format!("unknown command: selfupdate.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
