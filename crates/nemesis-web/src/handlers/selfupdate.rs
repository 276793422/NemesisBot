//! L9 selfupdate WSAPI — `selfupdate.check` / `selfupdate.download` /
//! `selfupdate.apply` / `selfupdate.status` / `selfupdate.cancel_download` /
//! `selfupdate.integrity`。
//!
//! download/apply 的 data 可带 `force: true`（2026-10-30 强制更新）：只豁免
//! 新鲜度闸（同版重下重装、存量 pending 重拉）；estop / 单飞锁 / 跨进程
//! 安装锁 / journal 闸 / sha256 / v4 验签**全部照常**——force 是「重装当前
//! 版本」的意志，不是安全旁路。`selfupdate.integrity` = 完整性检查（对
//! applied_manifest.json 已验签基线逐一比对活 exe 与 plugins/ 的 sha256；
//! 只读不处置，修复路径 = 强制更新）。
//!
//! manager 槽（`set_manager`，gateway run_runtime 装配点注入）与启动身份
//! 槽（`set_boot_identity`：version + 短 commit + workspace）都是静态槽
//!（身份槽 OnceLock；manager 槽 RwLock<Option>——测试进程需可清，装配语义
//! 仍「首个生效」）——handlers 模块无状态、槽未装配时命令诚实报「未装配」
//!（plugins_wasm
//! installer 槽同款模式）。check/status/journal 只依赖身份槽（无 feature）；
//! download/apply/cancel 需要 `selfupdate` feature（manager 类型本体）+
//! gateway 装配，双闸诚实降级。
//!
//! 交互形态（2026-10-21 用户拍板）：download/apply 都是**受理即回**的后台
//! 任务——进度走 SSE `selfupdate-progress`（易逝事件，不进断线补拉）+
//! status 轮询快照；确认在 Dashboard 侧完成（MessageBox 式确认框），后端
//! 无审批卡（v4 签名验签是安全闸）。

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
// RwLock<Option> 而非 OnceLock：装配语义仍是「首个生效」，但测试进程需要
// 可清——gateway full_assembly 冒烟在同进程跑生产 run() 会占槽，OnceLock
// 时代断言「未装配」的测试只能靠执行顺序，跨测试污染偶发假红（2026-10-13）。
#[cfg(feature = "selfupdate")]
static MANAGER: std::sync::RwLock<Option<std::sync::Arc<crate::selfupdate::SelfUpdateManager>>> =
    std::sync::RwLock::new(None);

/// gateway 装配点：注入 apply 编排器（首个生效——agent 重启不重装）。
#[cfg(feature = "selfupdate")]
pub fn set_manager(m: std::sync::Arc<crate::selfupdate::SelfUpdateManager>) {
    let mut slot = MANAGER.write().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(m);
    }
}

/// 槽读取（gateway 内部命令监听臂与 WSAPI apply 共用；None = 未装配）。
#[cfg(feature = "selfupdate")]
pub fn manager() -> Option<std::sync::Arc<crate::selfupdate::SelfUpdateManager>> {
    MANAGER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 测试隔离钩子（生产禁用——唯一合法调用方 = 测试 arrange 段）：清空
/// manager 槽，让「未装配诚实报错」类断言不依赖同进程测试执行顺序。
#[doc(hidden)]
#[cfg(feature = "selfupdate")]
pub fn clear_manager_for_tests() {
    *MANAGER.write().unwrap_or_else(|e| e.into_inner()) = None;
}

/// 本地待装包状态（无 updates/ 或元数据损坏 = Null——load_pending 内部
/// 已对坏 meta/坏 zip 做整清，这里只投影）。
#[cfg(feature = "selfupdate")]
fn pending_value() -> serde_json::Value {
    std::env::current_exe()
        .ok()
        .and_then(|exe| crate::selfupdate::load_pending(&exe))
        .and_then(|p| serde_json::to_value(&p).ok())
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(not(feature = "selfupdate"))]
fn pending_value() -> serde_json::Value {
    serde_json::Value::Null
}

pub struct SelfupdateHandler;

#[async_trait::async_trait]
impl ModuleHandler for SelfupdateHandler {
    fn module_name(&self) -> &str {
        "selfupdate"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "check",
            "download",
            "apply",
            "status",
            "cancel_download",
            "integrity",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            // 只读检查：Release 最新产物 vs 当前 commit 身份。网络失败诚实报。
            // 成功结果缓存进 manager（status 面供 Dashboard 免重查）；返回值
            // 合并本地待装包态（pending 非空 = ready_to_apply）。下载代理
            // 调用时读盘（config.proxy.json selfupdate 段——保存即对下次
            // 检查生效，免重启）。
            "check" => {
                let ident = boot_identity()
                    .map(|b| (b.version.clone(), b.commit.clone()))
                    .unwrap_or_default();
                let proxy = ctx.workspace.as_deref().and_then(|ws| {
                    nemesis_config::proxy_file::load_selfupdate_proxy(
                        &std::path::Path::new(ws).join("config"),
                    )
                });
                let c = crate::selfupdate::check(&ident.0, &ident.1, proxy.as_deref()).await?;
                let mut v = serde_json::to_value(&c).map_err(|e| e.to_string())?;
                #[cfg(feature = "selfupdate")]
                if let Some(mgr) = manager() {
                    mgr.store_check(c.clone());
                }
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("pending".to_string(), pending_value());
                }
                Ok(Some(v))
            }
            // 两段式第一步：只下载 + 落盘 + sha 校验。受理即回（后台任务），
            // 进度走 SSE `selfupdate-progress` + status 轮询；重复调用幂等
            // （同目标已在下/已就绪直接返回成功）。data.force=true（强制
            // 更新）跳过「pending 已就绪」幂等早退——存量 pending 可能就是
            // 残包，强制更新必须重拉。
            "download" => {
                let force = data
                    .as_ref()
                    .and_then(|d| d.get("force"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                #[cfg(feature = "selfupdate")]
                {
                    let mgr = manager().ok_or_else(|| {
                        "selfupdate 子系统未装配（gateway 装配未完成）".to_string()
                    })?;
                    let commit = boot_identity()
                        .map(|b| b.commit.clone())
                        .unwrap_or_default();
                    tokio::spawn(async move {
                        match mgr.download(&commit, force).await {
                            Ok(msg) => tracing::info!("[SelfUpdate] {msg}"),
                            Err(e) => tracing::warn!("[SelfUpdate] download 失败：{e}"),
                        }
                    });
                    Ok(Some(serde_json::json!({
                        "accepted": true,
                        "note": "下载已受理：进度走 SSE selfupdate-progress 与 selfupdate.status"
                    })))
                }
                #[cfg(not(feature = "selfupdate"))]
                {
                    let _ = force;
                    Err("selfupdate 通道未编译（构建未带 selfupdate feature）；\
                         请手动下载 Release 包替换"
                        .to_string())
                }
            }
            // 两段式第二步（或一步式直达）：staging + 受控重启。受理即回；
            // 无待装包自动先下载（CLI 兼容旧一步式调用）；执行进度走 SSE
            // 与 gateway 日志；成功后自动重启（确认在 Dashboard 侧完成，
            // 后端无审批卡——v4 验签是安全闸）。
            "apply" => {
                let allow_downgrade = data
                    .as_ref()
                    .and_then(|d| d.get("allow_downgrade"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let force = data
                    .as_ref()
                    .and_then(|d| d.get("force"))
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
                        match mgr.apply(&version, &commit, allow_downgrade, force).await {
                            Ok(msg) => tracing::info!("[SelfUpdate] {msg}"),
                            Err(e) => tracing::warn!("[SelfUpdate] apply 失败：{e}"),
                        }
                    });
                    Ok(Some(serde_json::json!({
                        "accepted": true,
                        "note": "apply 已受理：无待装包会自动先下载；执行进度走 SSE \
                                 （selfupdate-progress）与 gateway 日志；成功后自动重启"
                    })))
                }
                #[cfg(not(feature = "selfupdate"))]
                {
                    let _ = (allow_downgrade, force);
                    Err("selfupdate 通道未编译（构建未带 selfupdate feature）；\
                         请手动下载 Release 包替换"
                        .to_string())
                }
            }
            // 状态查询：启动身份 + journal 相位 + 平台支持 + 待装包 + 下载
            // 进度快照 + check 缓存（Dashboard 启动恢复态一次拉全）。
            "status" => {
                let ident = boot_identity();
                let (version, commit) = ident
                    .map(|b| (b.version.clone(), b.commit.clone()))
                    .unwrap_or_default();
                let journal_phase =
                    ident.map(|b| crate::selfupdate::load_journal(&b.workspace).map(|j| j.phase));
                #[cfg(feature = "selfupdate")]
                let (wired, download_v, cached_v) = {
                    let mgr = manager();
                    let download_v = mgr
                        .as_ref()
                        .map(|m| serde_json::to_value(m.progress_snapshot()).unwrap_or_default())
                        .unwrap_or_else(|| serde_json::json!({"active": false, "phase": "idle"}));
                    let cached_v = mgr
                        .as_ref()
                        .and_then(|m| m.cached_check())
                        .map(|(c, at)| {
                            let mut cv =
                                serde_json::to_value(&c).unwrap_or(serde_json::Value::Null);
                            if let Some(obj) = cv.as_object_mut() {
                                obj.insert("checked_at_ms".to_string(), serde_json::json!(at));
                            }
                            cv
                        })
                        .unwrap_or(serde_json::Value::Null);
                    (mgr.is_some(), download_v, cached_v)
                };
                #[cfg(not(feature = "selfupdate"))]
                let (wired, download_v, cached_v) = (
                    false,
                    serde_json::json!({"active": false, "phase": "idle"}),
                    serde_json::Value::Null,
                );
                // apply 失败反馈面（2026-10-23 挂死修复）：粘滞态解挂依据
                // （last_apply_error）+ 首更形态提前告知依据
                // （sequence_baseline_present=false = 无 applied 基准，顺序
                // 闸默认 Unknown 拒绝）。基准只看文件在场性，权威判定在闸里。
                #[cfg(feature = "selfupdate")]
                let (apply_err_v, baseline_v) = {
                    let mgr = manager();
                    let apply_err_v = mgr
                        .as_ref()
                        .and_then(|m| m.last_apply_error_snapshot())
                        .map(|r| serde_json::to_value(&r).unwrap_or(serde_json::Value::Null))
                        .unwrap_or(serde_json::Value::Null);
                    let baseline_v = std::env::current_exe()
                        .ok()
                        .map(|exe| crate::selfupdate::sequence_baseline_present(&exe))
                        .unwrap_or(false);
                    (apply_err_v, baseline_v)
                };
                #[cfg(not(feature = "selfupdate"))]
                let (apply_err_v, baseline_v) = (serde_json::Value::Null, false);
                Ok(Some(serde_json::json!({
                    "version": version,
                    "commit": commit,
                    "commit_known": !commit.is_empty(),
                    "asset": crate::selfupdate::TARGET_ASSET,
                    "wired": wired,
                    "journal_phase": journal_phase,
                    "platform_supported": crate::selfupdate::apply_supported(),
                    "pending": pending_value(),
                    "download": download_v,
                    "cached_check": cached_v,
                    "last_apply_error": apply_err_v,
                    "sequence_baseline_present": baseline_v,
                })))
            }
            // 取消在途下载：旗标置位，下载回调下一个数据块即中止并整清
            // updates/——回 [可下载] 态。
            "cancel_download" => {
                #[cfg(feature = "selfupdate")]
                {
                    let mgr = manager().ok_or_else(|| {
                        "selfupdate 子系统未装配（gateway 装配未完成）".to_string()
                    })?;
                    mgr.cancel_download();
                    Ok(Some(serde_json::json!({
                        "accepted": true,
                        "note": "取消请求已受理：下载中止后 updates/ 将被清空"
                    })))
                }
                #[cfg(not(feature = "selfupdate"))]
                {
                    Err("selfupdate 通道未编译（构建未带 selfupdate feature）".to_string())
                }
            }
            // 完整性检查（2026-10-30）：活 exe + plugins/ 平面文件逐一对
            // applied_manifest.json 已验签基线做 sha256 比对。只读不处置——
            // 修复路径 = 强制更新（download/apply 带 force）。同步返回
            //（本地哈希，秒级；无网络依赖）。
            "integrity" => {
                #[cfg(feature = "selfupdate")]
                {
                    let mgr = manager().ok_or_else(|| {
                        "selfupdate 子系统未装配（gateway 装配未完成）".to_string()
                    })?;
                    let report = mgr.integrity_snapshot();
                    let v = serde_json::to_value(&report).map_err(|e| e.to_string())?;
                    Ok(Some(v))
                }
                #[cfg(not(feature = "selfupdate"))]
                {
                    Err("selfupdate 通道未编译（构建未带 selfupdate feature）".to_string())
                }
            }
            _ => Err(format!("unknown command: selfupdate.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
