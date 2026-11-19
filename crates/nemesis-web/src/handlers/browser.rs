//! 浏览器接管 WSAPI handler（2026-11-15，计划 D15 管理面）。
//!
//! 命令清单（l1 注册表全量登记）：
//! - `browser.detect`：枚举系统已装浏览器（DriverKind + 路径 + phase2 标）
//! - `browser.spawn`：启动会话（可选 kind/executable 覆盖；解析序见
//!   BrowserManager::resolve_browser——配置 > Chrome 偏好 > 平台保底）
//! - `browser.sessions`：会话列表快照（实例卡数据源）
//! - `browser.tabs`：页签枚举（session_id 缺省第一个会话）
//! - `browser.attach`：设 working tab（session_id + target_id）
//! - `browser.release`：释放 working tab（回到「打开页签寻路」缺省语义）
//! - `browser.close`：关闭会话（进程 kill + 注册表摘除；estop 不走此门）
//! - `browser.logs`：编排操作路日志（环形缓冲，session_id 可选过滤）
//!
//! 闸面：cargo feature `browser`（关 = 本 handler 整体不注册，命令
//! unknown）；manager 未注入（headless/测试装配）= 诚实报错。estop 不拦
//! spawn/act（工具 dispatch 已闸；管理面的 close 是降险动作恒放行）。

use std::sync::{Arc, OnceLock};

use crate::ws_router::{ModuleHandler, RequestContext};

// 管理面单例走模块级 OnceLock 而非 AppState 字段（同 BOARD_ESTOP/
// BASELINE_PUSHER 裁决：AppState 手写字面量构造测试遍布全库，加字段是
// 断点级改动）。gateway 装配点（post_agent）注入；OnceLock 语义 = 进程
// 生命周期单次装配，与 BrowserManager 本身的长寿命一致。
static BROWSER_MANAGER: OnceLock<Arc<nemesis_browser::BrowserManager>> = OnceLock::new();

/// gateway 装配点注入（`nemesis_web::handlers::browser::set_browser_manager`）。
pub fn set_browser_manager(manager: Arc<nemesis_browser::BrowserManager>) {
    let _ = BROWSER_MANAGER.set(manager);
}

fn browser_manager() -> Result<Arc<nemesis_browser::BrowserManager>, String> {
    BROWSER_MANAGER
        .get()
        .cloned()
        .ok_or_else(|| "浏览器管理面未装配（gateway 未注入或 browser feature 关闭）".to_string())
}

pub struct BrowserHandler;

/// 会话选择（显式 session_id 优先；缺省 = 第一个**活**会话——F28，
/// 2026-11-15 五审：统一走 manager.pick_default_session 与工具侧同源同
/// 语义，旧实现取注册表第一个、死活不论，手关窗口后缺省臂撞死会话）。
async fn pick_session(
    manager: &nemesis_browser::BrowserManager,
    data: Option<&serde_json::Value>,
) -> Result<Arc<nemesis_browser::BrowserSession>, String> {
    let id = data
        .and_then(|d| d.get("session_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match id {
        Some(id) => {
            let s = manager
                .session(&id)
                .ok_or_else(|| format!("会话 {id} 不存在"))?;
            // F45（2026-11-16 九审）：显式臂同样验活——与工具侧 F40 同源
            // 同语义（竞窗死亡/手动直调时给可理解文案，而非底层 ws 拒连
            // 的传输层错误）；close 不走此处（关死会话是合法清卡动作）。
            if !s.is_alive().await {
                return Err(format!("会话 {id} 已退出（窗口已关闭）"));
            }
            Ok(s)
        }
        None => manager.pick_default_session().await,
    }
}

fn opt_str(data: Option<&serde_json::Value>, key: &str) -> Option<String> {
    data.and_then(|d| d.get(key))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[async_trait::async_trait]
impl ModuleHandler for BrowserHandler {
    fn module_name(&self) -> &str {
        "browser"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "detect", "spawn", "sessions", "tabs", "attach", "release", "close", "logs",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        if !self.commands().contains(&cmd) {
            return Err(format!("unknown command: browser.{cmd}"));
        }
        let manager = browser_manager()?;

        match cmd {
            "detect" => {
                let browsers: Vec<serde_json::Value> = manager
                    .detect()
                    .iter()
                    .map(|b| {
                        serde_json::json!({
                            "kind": b.kind.slug(),
                            "name": b.kind.display_name(),
                            "path": b.path.as_ref().map(|p| p.to_string_lossy()),
                            "phase2": b.phase2,
                            "source": b.source,
                        })
                    })
                    .collect();
                Ok(Some(serde_json::json!({ "browsers": browsers })))
            }
            "spawn" => {
                let kind = opt_str(data.as_ref(), "kind");
                let executable = opt_str(data.as_ref(), "executable");
                let session = manager
                    .spawn(kind.as_deref(), executable.as_deref())
                    .await?;
                Ok(Some(serde_json::json!({ "session": session.summary() })))
            }
            "sessions" => Ok(Some(serde_json::json!({
                "sessions": manager.sessions_snapshot(),
            }))),
            "tabs" => {
                let session = pick_session(&manager, data.as_ref()).await?;
                let tabs = session.tabs_snapshot().await?;
                Ok(Some(serde_json::json!({
                    "session_id": session.id,
                    "working_tab_id": session.working_tab_id(),
                    "tabs": tabs,
                })))
            }
            "attach" => {
                let session = pick_session(&manager, data.as_ref()).await?;
                let target_id = data
                    .as_ref()
                    .and_then(|d| d.get("target_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 target_id")?
                    .to_string();
                // manager 层包装（非直调 session）：attach/release 编排留痕
                // 进环形缓冲 + SSE——设置页 hint「attach 全留痕」的兑现。
                Ok(Some(manager.attach_tab(&session.id, &target_id).await?))
            }
            "release" => {
                let session = pick_session(&manager, data.as_ref()).await?;
                Ok(Some(manager.release_tab(&session.id)?))
            }
            "close" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or("缺 session_id")?
                    .to_string();
                let result = manager.close_session(&session_id).await?;
                Ok(Some(result))
            }
            "logs" => {
                let session_id = opt_str(data.as_ref(), "session_id");
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(nemesis_browser::DEFAULT_LOG_LIMIT);
                Ok(Some(serde_json::json!({
                    "logs": manager.logs(session_id.as_deref(), limit),
                })))
            }
            _ => unreachable!("commands() 清单已兜住"),
        }
    }
}
