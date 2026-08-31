//! 插件状态总览（`plugins.list`，只读）。
//!
//! Dashboard「插件」页（PluginsView）phase 1 的数据源：枚举已知插件库
//! （探测 exe 旁 `plugins/`）与当前构建的子系统 feature 状态。
//! 只读、无副作用；安装/启停（需要 effect/disposer 配对的注册机制）见
//! plugins-page-goal 扩展项。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::hooks::ToolHook;
// `Path` 本体仅在 memory feature 块内使用（no-feature 下避免 unused import 警告）。
#[cfg(feature = "memory")]
use std::path::Path;
use std::path::PathBuf;

pub struct PluginsHandler;

impl PluginsHandler {
    pub fn new() -> Self {
        Self
    }

    fn workspace(&self, ctx: &RequestContext) -> Result<String, String> {
        crate::handlers::require_workspace(ctx).map(|s| s.to_string())
    }

    /// 单个插件库的探测条目。
    fn detect_plugin(&self, id: &str, label: &str, used_by: &str) -> serde_json::Value {
        let path: Option<PathBuf> = nemesis_utils::find_plugin_library(id);
        let mut obj = serde_json::json!({
            "id": id,
            "label": label,
            "used_by": used_by,
            "found": path.is_some(),
            "filename": nemesis_utils::plugin_library_filename(id),
        });
        if let Some(p) = path {
            obj["path"] = serde_json::json!(p.display().to_string());
        }
        obj
    }

    fn plugins_list(&self, workspace: &str) -> Result<serde_json::Value, String> {
        let _ = workspace; // 预留：后续按 workspace 差异化（多实例/集群）时使用

        // plugin_onnx：能力状态取自 embedding 配置（active tier 模型就绪与否）。
        // nemesis-memory 是可选依赖：feature off 时仅报告文件探测结果。
        let mut onnx = self.detect_plugin(
            "plugin_onnx",
            "ONNX 嵌入推理",
            "强化记忆 / 自动记忆注入",
        );
        onnx["capabilities"] = serde_json::json!(["embedding 推理（tokenizer + model.onnx）"]);
        #[cfg(feature = "memory")]
        {
            let workspace_path = Path::new(workspace);
            let config_dir = nemesis_path::workspace_config_dir(workspace_path);
            let emb =
                nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir);
            let emb_data_dir =
                nemesis_memory::vector::embedding_config::embedding_data_dir(&config_dir);
            let active = &emb.active;
            let model_ready = emb.models.get(active).map(|mc| {
                (!mc.local_model_path.is_empty()
                    && Path::new(&mc.local_model_path).exists())
                    || emb_data_dir.join(&mc.name).join("model.onnx").exists()
            });
            onnx["detail"] = serde_json::json!({
                "enhanced_memory_enabled": emb.enabled,
                "active_tier": active,
                "active_model": emb.models.get(active).map(|m| m.name.clone()),
                "model_ready": model_ready,
            });
        }
        #[cfg(not(feature = "memory"))]
        {
            onnx["detail"] = serde_json::json!({ "note": "memory feature 未编译" });
        }

        // plugin_ui：desktop 子系统消费（WebView UI + Linux 系统托盘）。
        let mut ui = self.detect_plugin(
            "plugin_ui",
            "WebView UI / 系统托盘",
            "desktop 集成（Linux 托盘经 plugin-ui.so 运行时加载）",
        );
        ui["capabilities"] = serde_json::json!(["webview 宿主", "系统托盘（Linux）"]);

        // 编译期子系统 feature 状态（cfg! 在编译期固化，展示当前构建形态）。
        let features = serde_json::json!([
            { "id": "memory", "label": "强化记忆", "enabled": cfg!(feature = "memory") },
            { "id": "workflow", "label": "工作流", "enabled": cfg!(feature = "workflow") },
            { "id": "cluster", "label": "集群", "enabled": cfg!(feature = "cluster") },
            { "id": "security", "label": "安全", "enabled": cfg!(feature = "security") },
            { "id": "forge", "label": "Forge", "enabled": cfg!(feature = "forge") },
            { "id": "voice", "label": "语音", "enabled": cfg!(feature = "voice") },
            { "id": "sandbox", "label": "沙盒", "enabled": cfg!(feature = "sandbox") },
        ]);

        // 管线插件（T2 三段化的进程内插件；启停经 set_metrics_enabled）。
        let metrics = nemesis_agent::hooks::metrics_plugin_slot();
        let pipeline_plugins = serde_json::json!([{
            "name": metrics.name(),
            "scope": serde_json::Value::Null,
            "enabled": metrics.is_enabled(),
            "description": "每工具调用计时（around 段参考实现）",
        }]);

        Ok(serde_json::json!({
            "plugins": [onnx, ui],
            "features": features,
            "pipeline_plugins": pipeline_plugins,
        }))
    }
}

impl Default for PluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ModuleHandler for PluginsHandler {
    fn module_name(&self) -> &str {
        "plugins"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = self.workspace(ctx)?;
        // 路由按 module_name 分发后 cmd 是裸名——不要带模块前缀
        // （commands.list 事故：臂写全名导致 100% unknown command）。
        let _ = data;
        match cmd {
            "list" => self.plugins_list(&workspace).map(Some),
            "set_metrics_enabled" => {
                let data = data.ok_or("missing data")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("enabled (bool) is required")?;
                nemesis_agent::hooks::metrics_plugin_slot().set_enabled(enabled);
                Ok(Some(serde_json::json!({
                    "name": "metrics-pipeline",
                    "enabled": enabled,
                })))
            }
            _ => Err(format!("unknown command: plugins.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
