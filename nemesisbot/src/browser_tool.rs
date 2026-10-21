//! P5-R1：`browser` 工具——VM 档 guest 内 headless chromium 的宿主侧入口。
//!
//! 执行链：LLM 调 `browser` → 本工具（标准 dispatch：8 层管线 + 审计链
//! 照常全跑）→ 解析闭包定位 agent 记录 → 档位校验（**仅 VM 档**）→
//! `VmSupervisor::browser_call` → guest 执行体（nemesis-guest src/browser.rs，
//! chromium + CDP）。
//!
//! 设计锚点（plan P5-R1）：
//! - **VM 档专用，不回落**：进程档没有浏览器执行体（guest 内 chromium 是
//!   唯一形态），非 VM 档诚实报错——与 ToolFailed「绝不回落」语义同源
//!   （宿主回落 = 环境切换的正确性事故，不是降级）。
//! - **审计链**：本工具走标准 dispatch（security 管线按 Tool 分类注入
//!   检测/审计链），guest 是哑执行体——plan 验收门「浏览器调用全程审计
//!   链留痕」由标准 dispatch 满足，不建旁路。
//! - **注册闸**：persistent 路由在场（gateway 形态）才注册——headless
//!   run / acp 无 agents 表解析面，注册了也永远解析失败，诚实缺省
//!   （generate_image 同款注册闸风格）。
//! - **profile 落 VM 盘**：`.browser-profile/` 在 guest `/workspace` 下、
//!   宿主 vm_sync 排除表内（cookies/缓存不进项目工作副本）；overlay 承载
//!   = 睡眠/唤醒/冷重启登录态保持（验收门）。

use std::sync::Arc;

use nemesis_agent::context::RequestContext;

/// 工具注册名（loop::Tool 无 name()；注册键即 LLM 可见名）。
pub const TOOL_NAME: &str = "browser";

/// guest 动作集（与 nemesis-guest src/browser.rs handle 的分派表同源镜像；
/// 两侧漂移由 guest tests + 本侧 schema 对账钉住）。
pub(crate) const BROWSER_ACTIONS: [&str; 7] = [
    "navigate",
    "click",
    "type",
    "fill_form",
    "extract_text",
    "screenshot",
    "wait_for",
];

/// VM 档浏览器工具。`resolve` 与 MOVE_TOOLS 常驻桥同源（persistent_resolve_by
/// 闭包——调用时现查 agents 表，静态绑定必空的时序纪律同款）。
pub struct VmBrowserTool {
    resolve: Arc<dyn Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync>,
}

impl VmBrowserTool {
    pub fn new(
        resolve: Arc<dyn Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync>,
    ) -> Self {
        Self { resolve }
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for VmBrowserTool {
    fn description(&self) -> String {
        "Drive a headless Chromium browser inside this agent's VM (isolated \
         desktop-grade sandbox). Actions: navigate (load URL and wait for \
         page ready), click (CSS selector), type (fill an input), fill_form \
         (multiple fields at once), extract_text (page or element text), \
         screenshot (PNG saved under the workspace uploads/ tree — reference \
         it by returned path for vision review), wait_for (poll until a \
         selector exists). Browser state (cookies/login) persists across VM \
         sleep/wake and restarts. Use VM-internal view: pages run in the VM's \
         network, not the host's."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": BROWSER_ACTIONS,
                    "description": "Browser action to perform"
                },
                "url": {
                    "type": "string",
                    "description": "navigate: URL to load (required for navigate)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector (click/type/extract_text/wait_for)"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type (type)"
                },
                "fields": {
                    "type": "array",
                    "description": "fill_form: [{selector, text}, ...] filled in order",
                    "items": {
                        "type": "object",
                        "properties": {
                            "selector": { "type": "string" },
                            "text": { "type": "string" }
                        },
                        "required": ["selector", "text"]
                    }
                },
                "path": {
                    "type": "string",
                    "description": "screenshot: relative save path under the workspace (default uploads/browser-<ts>.png)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "wait_for/navigate load budget in milliseconds (default 10000 / 30000)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 非 JSON: {e}"))?;
        let action = parsed
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing 'action' argument".to_string())?;
        if !BROWSER_ACTIONS.contains(&action) {
            return Err(format!(
                "unknown action {action:?} (allowed: {})",
                BROWSER_ACTIONS.join(", ")
            ));
        }

        // 身份解析（调用时现查——loop 建成早于实体 seed 的时序纪律）。
        let Some((agent_id, _exec_root)) = (self.resolve)() else {
            return Err(
                "browser 工具无持久 agent 路由（本形态无 agents 表解析面，浏览器执行体仅在 \
                 gateway 常驻 VM 档形态可用）"
                    .to_string(),
            );
        };

        // 档位校验：仅 VM 档（进程档没有浏览器执行体，不回落宿主）。
        let mgr = crate::agents::manager()
            .ok_or_else(|| "agent 管理器未装配（浏览器执行体依赖持久 agent 实体面）".to_string())?;
        let rec = mgr
            .get(&agent_id)
            .ok()
            .flatten()
            .ok_or_else(|| format!("agent {agent_id} 记录缺席（agents 表无此实体）"))?;
        if rec.runtime_backend != nemesis_taskstore::agents::runtime_backend::VM {
            return Err(format!(
                "browser 工具仅 VM 档 agent 可用：当前 agent 档位为 {}（guest 内 chromium 只在 \
                 VM 内运行，进程档无浏览器执行体，不做宿主回落）。可用 nemesisbot agents \
                 backend-set 切档后重试",
                rec.runtime_backend
            ));
        }

        // VM 依赖面（supervisor + 错误型）随 `vm` feature——裁剪形态诚实
        // 报「未编译」（同 voice-capture 拆分先例：设备路径运行期诚实报）。
        #[cfg(feature = "vm")]
        {
            let sup = mgr.vm_supervisor().ok_or_else(|| {
                "VM 后端未装配（executor.vm 关闭或 gateway 未装配 VM supervisor）".to_string()
            })?;
            sup.browser_call(&agent_id, parsed).await.map_err(|e| {
                // Unavailable = 请求从未发生（VM 未运行/镜像缺/满员）；Failed =
                // 副作用未知——两者对 LLM 都是诚实错误文本，不回落。
                match e {
                    crate::agents::vm_runtime::VmSupervisorError::Unavailable(m) => {
                        format!("browser 调用不可用（VM 未就绪）: {m}")
                    }
                    crate::agents::vm_runtime::VmSupervisorError::LifecycleRejected(m) => {
                        format!("browser 调用被生命周期拒绝（如 agent 删除在途）: {m}")
                    }
                    crate::agents::vm_runtime::VmSupervisorError::Failed(m) => {
                        format!("browser 调用失败: {m}")
                    }
                }
            })
        }
        #[cfg(not(feature = "vm"))]
        {
            let _ = mgr;
            Err("browser 工具依赖 VM 后端（vm feature），当前构建未编译该功能".to_string())
        }
    }
}

#[cfg(test)]
mod tests;
