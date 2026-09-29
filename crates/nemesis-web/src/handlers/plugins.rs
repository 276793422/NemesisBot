//! 插件状态总览（`plugins.list`）+ WASM 插件开发包下载（`wasm.devkit_download`）。
//!
//! Dashboard「插件」页（PluginsView，三 Tab）的数据源：`plugins.list` 枚举
//! 已知插件库（探测 exe 旁 `plugins/`）；`wasm.*` 系列代理到
//! handlers/plugins_wasm.rs（宿主运行时管理面），其中 `wasm.devkit_download`
//! 自处理——官方 Release 拉开发包（源码 zip，最小可编译集合）落 workspace
//! 并解包。编译期子系统 feature 状态不在此处（→ system.features，About 页
//! 「构建形态」tab 消费）。只读为主、无副作用（devkit 下载只落 workspace）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::hooks::ToolHook;
// `Path` 本体仅在 memory feature 块内使用（no-feature 下避免 unused import 警告）。
#[cfg(feature = "memory")]
use std::path::Path;
use std::path::PathBuf;

/// WASM 插件开发包（devkit）的 Release 附件名与落位子目录——CI
///（daily-release，仅 linux，平台无关源码包）打包链的固定产物。下载管线
/// 与 skins 同源（[`crate::handlers::release_fetch`]，SSRF 逐跳闸 + 上限）。
#[cfg(feature = "plugins-wasm")]
const DEVKIT_ZIP_NAME: &str = "nightly-wasm-devkit.zip";

pub struct PluginsHandler;

impl PluginsHandler {
    pub fn new() -> Self {
        Self
    }

    fn workspace(&self, ctx: &RequestContext) -> Result<String, String> {
        crate::handlers::require_workspace(ctx).map(|s| s.to_string())
    }

    /// `wasm.devkit_download`：官方 Release 拉 WASM 插件开发包（源码 zip，
    /// 最小可编译集合）落 `<workspace>/wasm-plugin-devkit/` 并解包到
    /// `devkit/`。`{overwrite}`（默认 false：zip 或 devkit/ 任一已存在即
    /// 拒绝）。返回 `{path, dir, size}`——前端指引用户在 devkit/ 内
    /// `cargo run -p pack`。下载管线与 skins 同源（SSRF 逐跳闸 + 上限）。
    #[cfg(feature = "plugins-wasm")]
    async fn devkit_download(
        &self,
        workspace: &str,
        data: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        use crate::handlers::release_fetch;
        let overwrite = data
            .as_ref()
            .and_then(|d| d.get("overwrite"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let base = std::path::Path::new(workspace).join("wasm-plugin-devkit");
        let zip_path = base.join(DEVKIT_ZIP_NAME);
        let devkit_dir = base.join("devkit");
        if !overwrite && (zip_path.exists() || devkit_dir.exists()) {
            return Err(format!(
                "开发包已存在：{}（传 overwrite=true 覆盖重下）",
                zip_path.display()
            ));
        }
        let bytes = release_fetch::fetch_release_asset(
            DEVKIT_ZIP_NAME,
            release_fetch::RELEASE_ASSET_MAX_BYTES,
        )
        .await?;
        std::fs::create_dir_all(&base).map_err(|e| format!("创建目录失败：{e}"))?;
        std::fs::write(&zip_path, &bytes).map_err(|e| format!("写入失败：{e}"))?;
        // 覆盖重下 = 先清旧解包目录（Windows AV/索引器瞬时句柄 → 韧性重试）。
        if devkit_dir.exists() && !release_fetch::remove_dir_all_resilient(&devkit_dir) {
            return Err(format!(
                "旧开发包目录清除失败（被占用？）：{}",
                devkit_dir.display()
            ));
        }
        let files = extract_zip_root_stripped(&bytes, &devkit_dir)?;
        Ok(serde_json::json!({
            "path": zip_path.display().to_string(),
            "dir": devkit_dir.display().to_string(),
            "size": bytes.len(),
            "files": files,
        }))
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
        let mut onnx =
            self.detect_plugin("plugin_onnx", "ONNX 嵌入推理", "强化记忆 / 自动记忆注入");
        onnx["capabilities"] = serde_json::json!(["embedding 推理（tokenizer + model.onnx）"]);
        #[cfg(feature = "memory")]
        {
            let workspace_path = Path::new(workspace);
            let config_dir = nemesis_path::workspace_config_dir(workspace_path);
            let emb = nemesis_memory::vector::embedding_config::load_embedding_config(&config_dir);
            let emb_data_dir =
                nemesis_memory::vector::embedding_config::embedding_data_dir(&config_dir);
            let active = &emb.active;
            let model_ready = emb.models.get(active).map(|mc| {
                (!mc.local_model_path.is_empty() && Path::new(&mc.local_model_path).exists())
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
            "pipeline_plugins": pipeline_plugins,
        }))
    }
}

impl Default for PluginsHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// 解包 devkit zip 到 `dest`，剥掉zip 内根目录段（`wasm-plugin-devkit/…`
/// → `dest/…`）。zip-slip 防护：路径段含 `..`、绝对形态（`/` 开头或
/// Windows 盘符前缀）一律诚实拒绝。目录条目懒建——只落文件，父目录
/// 逐级 create_dir_all。返回写入的文件数。
#[cfg(feature = "plugins-wasm")]
fn extract_zip_root_stripped(bytes: &[u8], dest: &std::path::Path) -> Result<usize, String> {
    use std::io::Read as _;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("ZIP 无法解析：{e}"))?;
    std::fs::create_dir_all(dest).map_err(|e| format!("创建目录失败：{e}"))?;
    let dest_canonical = dest
        .canonicalize()
        .map_err(|e| format!("目录解析失败：{e}"))?;
    let mut written = 0usize;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("ZIP 条目 {i} 不可读：{e}"))?;
        let raw = entry.name().to_string();
        // 剥首段根目录；无 '/' 的散条目（不存在于本包形态）按原名落。
        let rel = raw.split_once('/').map(|(_, r)| r).unwrap_or(&raw);
        if rel.is_empty() {
            continue; // 根目录条目本身
        }
        let rel_path = std::path::PathBuf::from(rel.replace('\\', "/"));
        // zip-slip：显式逐段检查（starts_with 词法比较之外再拦 `..` 与
        // 前缀/绝对形态——包名是攻击者可控的，不能只信词法前缀）。
        let mut suspect = rel_path.is_absolute() || has_drive_prefix(&rel_path);
        for comp in rel_path.components() {
            match comp {
                std::path::Component::ParentDir => suspect = true,
                std::path::Component::RootDir | std::path::Component::Prefix(_) => suspect = true,
                _ => {}
            }
        }
        if suspect {
            return Err(format!("ZIP 条目路径不合法（疑似路径穿越）：{raw}"));
        }
        let out = dest_canonical.join(&rel_path);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("创建目录失败：{e}"))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("条目 {raw} 不可读：{e}"))?;
        std::fs::write(&out, &buf).map_err(|e| format!("写入 {} 失败：{e}", out.display()))?;
        written += 1;
    }
    Ok(written)
}

/// Windows 盘符前缀检测（`C:` / `C:\` 形态——Path::is_absolute 在
/// Windows 对 `C:foo` 相对盘符路径返回 false，但落盘仍会跳出 dest）。
#[cfg(feature = "plugins-wasm")]
fn has_drive_prefix(p: &std::path::Path) -> bool {
    use std::path::Component::*;
    matches!(p.components().next(), Some(Prefix(_)))
}

#[async_trait::async_trait]
impl ModuleHandler for PluginsHandler {
    fn module_name(&self) -> &str {
        "plugins"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list",
            "set_metrics_enabled",
            #[cfg(feature = "plugins-wasm")]
            "wasm.list",
            #[cfg(feature = "plugins-wasm")]
            "wasm.install",
            #[cfg(feature = "plugins-wasm")]
            "wasm.enable",
            #[cfg(feature = "plugins-wasm")]
            "wasm.disable",
            #[cfg(feature = "plugins-wasm")]
            "wasm.uninstall",
            #[cfg(feature = "plugins-wasm")]
            "wasm.config.get",
            #[cfg(feature = "plugins-wasm")]
            "wasm.config.set",
            #[cfg(feature = "plugins-wasm")]
            "wasm.logs",
            #[cfg(feature = "plugins-wasm")]
            "wasm.devkit_download",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        // W6：`plugins.wasm.*` 命令代理到 plugins_wasm.rs（宿主运行时整槽
        // 注入；槽空/未编译均诚实报错）。devkit 下载不依赖宿主运行时槽，
        // 在代理前拦截自处理。
        if let Some(bare) = cmd.strip_prefix("wasm.") {
            #[cfg(feature = "plugins-wasm")]
            {
                let workspace = crate::handlers::require_workspace(ctx)?.to_string();
                if bare == "devkit_download" {
                    return self.devkit_download(&workspace, data).await.map(Some);
                }
                return crate::handlers::plugins_wasm::handle(bare, data).await;
            }
            #[cfg(not(feature = "plugins-wasm"))]
            {
                let _ = (bare, &data);
                return Err(format!(
                    "WASM 插件子系统未编译（plugins-wasm feature 关）：plugins.{cmd}"
                ));
            }
        }
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

// AGT 覆盖率批次（2026-09-25）：Default 转发体。豁免（exe 旁 plugins/
// 探测目录劫持并行断言）见 agt_tests 头注。
#[cfg(test)]
mod agt_tests;
