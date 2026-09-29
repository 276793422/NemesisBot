//! System handler — version and status commands.

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct SystemHandler;

/// 构建形态清单槽：gateway 注入 nemesisbot build.rs 生成的 features.json
///（include_str! 嵌入的 `'static` 字符串直接存引用）。测试装配不注入 →
/// `system.features` 回空数组（诚实：无清单=无形态数据）。
static FEATURES_MANIFEST: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// gateway 装配点：注入构建形态清单 JSON 文本。
pub fn set_features_manifest(json: &'static str) {
    let _ = FEATURES_MANIFEST.set(json);
}

#[async_trait::async_trait]
impl ModuleHandler for SystemHandler {
    fn module_name(&self) -> &str {
        "system"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["version", "status", "commands", "lease_status", "features"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "version" => self.version(ctx),
            "status" => self.status(ctx),
            // WS9/P22：workspace 写租约现状探针（无副作用——试取锁立即释
            // 放 + 读持有者 sidecar）。设置页网关卡片展示当前持有者。
            "lease_status" => Ok(Some(nemesis_agent::workspace_lease::WorkspaceLease::probe(
                ctx.workspace.as_deref(),
            ))),
            // 构建形态清单（feature 裁剪体系）：gateway 注入的 features.json
            //（单一真相源 scripts/customize/features.toml + 本构建 CARGO_FEATURE_*
            // 真实编译态）。未注入（测试装配）= 空数组。
            "features" => {
                let features = FEATURES_MANIFEST
                    .get()
                    .copied()
                    .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                    .unwrap_or_else(|| serde_json::Value::Array(vec![]));
                Ok(Some(serde_json::json!({ "features": features })))
            }
            // L1（devtool-upgrade 阶段 6）：全量 WSAPI 命令注册表——
            // register_all 发布的 OnceLock 快照（module → 静态清单）。
            "commands" => {
                let modules: Vec<serde_json::Value> = crate::handlers::commands_registry()
                    .iter()
                    .map(|(module, cmds)| serde_json::json!({ "module": module, "commands": cmds }))
                    .collect();
                let total_cmds: usize = crate::handlers::commands_registry()
                    .iter()
                    .map(|(_, cmds)| cmds.len())
                    .sum();
                Ok(Some(serde_json::json!({
                    "modules": modules,
                    "total_modules": modules.len(),
                    "total_cmds": total_cmds,
                })))
            }
            _ => Err(format!("unknown command: system.{}", cmd)),
        }
    }
}

impl SystemHandler {
    fn version(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let uptime = ctx.state.start_time.elapsed().as_secs();
        Ok(Some(serde_json::json!({
            "version": ctx.state.version,
            "uptime_seconds": uptime,
        })))
    }

    fn status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let uptime = ctx.state.start_time.elapsed().as_secs();
        let session_count = ctx
            .state
            .session_count
            .load(std::sync::atomic::Ordering::SeqCst);
        let running = ctx.state.running.load(std::sync::atomic::Ordering::SeqCst);
        let model_name = ctx.state.model_name.lock().clone();

        let mut status = serde_json::json!({
            "version": ctx.state.version,
            "uptime_seconds": uptime,
            "running": running,
            "session_count": session_count,
            "model_name": model_name,
        });

        if let Some(ref ws) = ctx.workspace {
            status
                .as_object_mut()
                .unwrap()
                .insert("workspace".to_string(), serde_json::json!(ws));
        }

        Ok(Some(status))
    }
}
