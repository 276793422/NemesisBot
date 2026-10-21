//! 插件状态总览（`plugins.list`）+ 开关（`plugins.native_set_enabled`）+
//! WASM 插件开发包下载（`wasm.devkit_download`）。
//!
//! Dashboard「插件」页（PluginsView）的数据源：`plugins.list` 枚举原生插件
//! 全目录（nemesis-config `plugins_native::NativePluginId` 四成员——目录
//! 真相源）+ 管线插件；每个原生条目带文件探测 / 用户开关（`<workspace>/
//! config/plugins_native.json`，缺省启用）/ 签名状态（gateway 装配期经
//! [`set_native_status_probe`] 注入——验签执行体在 nemesisbot 侧，本 crate
//! 不依赖 nemesis-verify；槽空 = dormant 诚实降级）/ 未知库清单（只列出，
//! 不装载）。`wasm.*` 系列代理到 handlers/plugins_wasm.rs（宿主运行时管理
//! 面），其中 `wasm.devkit_download` 自处理。编译期子系统 feature 状态不在
//! 此处（→ system.features）。开关写有副作用（长期记忆文件），其余只读。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::hooks::ToolHook;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// 原生插件签名/未知库状态 probe（gateway 装配期注入；闭包 =
/// nemesisbot `native_plugins::native_payload_json` 捕获 config_dir/exe_dir）。
/// 本 crate 无 nemesis-verify 依赖——验签面只存在于注入后。
static NATIVE_STATUS_PROBE: OnceLock<Arc<dyn Fn() -> serde_json::Value + Send + Sync>> =
    OnceLock::new();

/// gateway 装配期注入（agent_build，与 plugins_wasm::set_installer 同批）。
pub fn set_native_status_probe(probe: Arc<dyn Fn() -> serde_json::Value + Send + Sync>) {
    let _ = NATIVE_STATUS_PROBE.set(probe);
}

fn signature_dormant_fallback() -> serde_json::Value {
    serde_json::json!({
        "mode": "dormant",
        "state": null,
        "detail": "签名状态探测未接入（gateway 未注入 probe）",
    })
}

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

    /// 原生插件目录条目：文件探测 + 用户开关 + 签名状态（probe 合并）+ 生效语义。
    fn native_entry(
        &self,
        id: &str,
        label: &str,
        used_by: &str,
        config_dir: &Path,
        signature: serde_json::Value,
    ) -> serde_json::Value {
        let mut obj = self.detect_plugin(id, label, used_by);
        let enabled = match nemesis_config::plugins_native::NativePluginId::from_id(id) {
            Some(pid) => nemesis_config::plugins_native::load_enabled(config_dir, pid),
            None => true,
        };
        obj["enabled"] = serde_json::json!(enabled);
        obj["immediate_effect"] = serde_json::json!(
            nemesis_config::plugins_native::NativePluginId::from_id(id)
                .map(|p| p.immediate_effect())
                .unwrap_or(false)
        );
        obj["signature"] = signature;
        obj
    }

    fn plugins_list(&self, workspace: &str) -> Result<serde_json::Value, String> {
        let workspace_path = Path::new(workspace);
        let config_dir = nemesis_path::workspace_config_dir(workspace_path);

        // 签名/未知库状态：probe 注入（gateway 装配期）优先；槽空 = dormant
        // 降级 + 空未知库（开关/探测不受影响——它们走 nemesis-config 直读）。
        let probe = NATIVE_STATUS_PROBE.get().map(|f| f());
        let sig_of = |id: &str| -> serde_json::Value {
            probe
                .as_ref()
                .and_then(|p| p.get("status"))
                .and_then(|s| s.as_array())
                .and_then(|arr| {
                    arr.iter()
                        .find(|e| e.get("id").and_then(|i| i.as_str()) == Some(id))
                })
                .and_then(|e| e.get("signature"))
                .cloned()
                .unwrap_or_else(signature_dormant_fallback)
        };
        let unrecognized = probe
            .as_ref()
            .and_then(|p| p.get("unrecognized"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));

        // plugin_onnx：能力状态取自 embedding 配置（active tier 模型就绪与否）。
        // nemesis-memory 是可选依赖：feature off 时仅报告文件探测结果。
        let mut onnx = self.native_entry(
            "plugin_onnx",
            "ONNX 嵌入推理",
            "强化记忆 / 自动记忆注入",
            &config_dir,
            sig_of("plugin_onnx"),
        );
        onnx["capabilities"] = serde_json::json!(["embedding 推理（tokenizer + model.onnx）"]);
        #[cfg(feature = "memory")]
        {
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
        let mut ui = self.native_entry(
            "plugin_ui",
            "WebView UI / 系统托盘",
            "desktop 集成（Linux 托盘经 plugin-ui.so 运行时加载）",
            &config_dir,
            sig_of("plugin_ui"),
        );
        ui["capabilities"] = serde_json::json!(["webview 宿主", "系统托盘（Linux）"]);

        // eval 监控 DLL：eval 沙盒事件采集（唯一事件驱动装载点 = 开关即时生效）。
        let mut eval_monitor = self.native_entry(
            "eval_monitor_dll",
            "eval 沙盒监控",
            "eval 行为评估（监控事件采集；开关即时生效）",
            &config_dir,
            sig_of("eval_monitor_dll"),
        );
        eval_monitor["capabilities"] =
            serde_json::json!(["沙盒驱动事件采集（注入宿主 + SbieAPI）"]);

        // WASM 宿主壳：wasmtime 运行时所在（2026-10-21 宿主剥离）。
        let mut wasm_shell = self.native_entry(
            "plugin_wasm",
            "WASM 插件宿主壳",
            "WASM 组件插件子系统（plugin.<slug> 工具/观察者）",
            &config_dir,
            sig_of("plugin_wasm"),
        );
        wasm_shell["capabilities"] = serde_json::json!(["wasmtime 组件运行时（独立壳 DLL）"]);

        // 管线插件（T2 三段化的进程内插件；启停经 set_metrics_enabled）。
        let metrics = nemesis_agent::hooks::metrics_plugin_slot();
        let pipeline_plugins = serde_json::json!([{
            "name": metrics.name(),
            "scope": serde_json::Value::Null,
            "enabled": metrics.is_enabled(),
            "description": "每工具调用计时（around 段参考实现）",
        }]);

        Ok(serde_json::json!({
            "plugins": [onnx, ui, eval_monitor, wasm_shell],
            "pipeline_plugins": pipeline_plugins,
            "unrecognized_libraries": unrecognized,
        }))
    }

    /// `plugins.native_set_enabled {id, enabled}`：写开关文件（长期记忆，
    /// 读-改-写保留他条目；写锁与 config 家族同源）。返回生效语义提示。
    fn native_set_enabled(
        &self,
        workspace: &str,
        data: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let data = data.ok_or("missing data")?;
        let id = data
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("id (string) is required")?;
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or("enabled (bool) is required")?;
        let pid = nemesis_config::plugins_native::NativePluginId::from_id(id)
            .ok_or_else(|| format!("未知原生插件 id：{id}"))?;
        let config_dir = nemesis_path::workspace_config_dir(Path::new(workspace));
        nemesis_config::plugins_native::set_enabled(&config_dir, pid, enabled)?;
        Ok(serde_json::json!({
            "id": pid.id(),
            "enabled": enabled,
            "immediate_effect": pid.immediate_effect(),
            "note": if pid.immediate_effect() {
                "开关即时生效（事件驱动装载）"
            } else {
                "重启 gateway 后生效"
            },
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
        // zip 9：条目名解码可失败——诚实报错拒绝（zip-slip 检查前不允许含糊名字）。
        let raw = entry
            .name()
            .map_err(|e| format!("ZIP 条目 {i} 名字不可解码：{e}"))?
            .to_string();
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
            "native_set_enabled",
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
            "native_set_enabled" => self.native_set_enabled(&workspace, data).map(Some),
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
