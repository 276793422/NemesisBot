//! Full Access 编辑器放行开关 WSAPI(2026-09-20 用户裁决)——
//! `editor.get` / `editor.set`。
//!
//! 依赖方向(与 projects.rs 同款):`EditorAccessState` 本体在
//! nemesis-security,nemesisbot `security_setup` 装配期创建一个 Arc 同时
//! 注入 auditor(`evaluate_request` 判定热路径短路)与本模块级
//! [`EDITOR_ACCESS`] 槽(WSAPI 读写)——单一真相源,聊天框旁按钮与设置页
//! 【编辑器】TAB 都经这里读写同一实例。未装配(security 未启用/headless)
//! 时诚实报错。
//!
//! `set` 成功后向 EventHub 发布 `editor-mode` 事件(SSE 广播所有窗口——
//! 不走 AgentEvent,避免补穷举 match;前端 useSSE 订阅该事件名)。

use std::sync::Arc;

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_security::editor_access::EditorAccessState;

/// 进程级状态槽(`RwLock<Option<_>>` = PROJECTS_BRIDGE 同款:生产装配期
/// 安装一次、handler 只读;latest-wins;测试可置换)。
static EDITOR_ACCESS: std::sync::RwLock<Option<Arc<EditorAccessState>>> =
    std::sync::RwLock::new(None);

/// 装配期注入(nemesisbot security_setup:与 auditor 持同一 Arc)。
pub fn install_editor_access(state: Arc<EditorAccessState>) {
    *EDITOR_ACCESS.write().expect("editor access lock") = Some(state);
}

/// 取当前状态(clone Arc 出来,不持锁返回)。
fn editor_access() -> Option<Arc<EditorAccessState>> {
    EDITOR_ACCESS.read().expect("editor access lock").clone()
}

/// 槽置换口(仅测试):None = 恢复未装配形态。
#[cfg(test)]
pub(crate) fn set_editor_access_for_test(state: Option<Arc<EditorAccessState>>) {
    *EDITOR_ACCESS.write().expect("editor access lock") = state;
}

/// 槽触碰类测试的共享串行锁(进程级槽是共享态——并行测试互相置换会
/// flake;PROJECTS_BRIDGE 的 BRIDGE_TEST_LOCK 同款纪律)。只在 tests 里
/// acquire。
#[cfg(test)]
pub(crate) static EDITOR_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 刷新 workspace roots(裁决语义边界 6:运行期漂移由 get/set 每次刷新
/// 纠正,新建项目下次触碰开关即生效)。roots = 主 workspace + 项目
/// registry 全部项目根;无 workspace 且 bridge 未装配时保留旧 roots
/// (best-effort 降级,不使 get/set 失败——fail-safe)。
fn refresh_roots(ctx: &RequestContext, state: &EditorAccessState) {
    let mut roots: Vec<String> = Vec::new();
    if let Ok(ws) = crate::handlers::require_workspace(ctx) {
        roots.push(ws.to_string());
    }
    if let Some(bridge) = crate::handlers::projects::projects_bridge() {
        for p in bridge.list() {
            roots.push(p.path);
        }
    }
    if !roots.is_empty() {
        state.set_workspace_roots(roots);
    }
}

pub struct EditorHandler;

#[async_trait::async_trait]
impl ModuleHandler for EditorHandler {
    fn module_name(&self) -> &str {
        "editor"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["get", "set"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let state = editor_access()
            .ok_or_else(|| "编辑器放行未装配(security 未启用或旧装配)".to_string())?;
        match cmd {
            "get" => {
                refresh_roots(ctx, &state);
                let (full_access, external_write) = state.snapshot();
                Ok(Some(serde_json::json!({
                    "full_access": full_access,
                    "external_write": external_write,
                })))
            }
            "set" => {
                let data = data.ok_or("missing data")?;
                // 部分更新契约:缺键保持当前值;键存在但非 bool 显式报错
                // (get_opt_bool_loud——前端 bug 同轮暴露,不静默吞)。
                let (cur_full, cur_ext) = state.snapshot();
                let full_access =
                    crate::handlers::get_opt_bool_loud(&data, "full_access")?.unwrap_or(cur_full);
                let external_write =
                    crate::handlers::get_opt_bool_loud(&data, "external_write")?.unwrap_or(cur_ext);
                // 联动收口在 set_flags 内(ext=true ⇒ full=true;UI 禁用
                // 联动只是体验,不变量在服务端保证)。
                state.set_flags(full_access, external_write);
                refresh_roots(ctx, &state);
                let (full_access, external_write) = state.snapshot();
                tracing::info!(
                    "[Web/WSAPI] editor.set full_access={} external_write={} by session {}",
                    full_access,
                    external_write,
                    ctx.session_id
                );
                // SSE 广播所有窗口(生效值以服务端 snapshot 为准)。
                ctx.state.event_hub.publish(
                    "editor-mode",
                    serde_json::json!({
                        "full_access": full_access,
                        "external_write": external_write,
                    }),
                );
                Ok(Some(serde_json::json!({
                    "full_access": full_access,
                    "external_write": external_write,
                })))
            }
            _ => Err(format!("unknown command: editor.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
