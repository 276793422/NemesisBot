//! 集群专业职能框架 M4：`generate_image` 工具——images-openai lane 的唯一
//! agent 消费点。
//!
//! 职责：把 [`nemesis_providers::images::generate_image`] 的 b64 结果落盘到
//! 工作区 `images/` 目录，返回**路径 + 元数据**（不回传字节——LLM 上下文
//! 塞不进图像字节；M5 验收侧经资产通道按路径取用）。
//!
//! 安全形态：
//! - 端点固定（装配期 [`super::agent_factory::resolve_image_model`] 解析自
//!   config，不接受 URL 入参）——SSRF 面为零；
//! - 输出路径只接受相对文件名/相对子路径，`..` / 绝对路径 / 盘符 / 冒号
//!   一律拒绝——写入被构造性钉死在 output_dir 内（不依赖运行期围栏）；
//! - 8 层管线照常生效（`tool_to_operation` → NetworkRequest，注入检测/
//!   凭据扫描/DLP/审计链全部跑）；不做 executor 隔离（不出 MOVE_TOOLS
//!   ——沙盒断网反而打不通端点）。
//!
//! 注册面：`agent_factory.rs::register_tools_and_mcp`（主/项目/集群三 loop
//! 统一）；注册闸 = 图像模型可解析，未配置 = 不注册（诚实缺省）。

use base64::Engine as _;
use nemesis_agent::context::RequestContext;
use std::path::{Component, Path, PathBuf};

/// 工具注册名（loop::Tool 无 name()；注册键即 LLM 可见名）。
pub const TOOL_NAME: &str = "generate_image";

/// generate_image 工具。
pub struct GenerateImageTool {
    /// 图像端点 base（含 `/v1` 等版本段，与模型条目 api_base 同源）。
    api_base: String,
    api_key: String,
    /// wire 模型名（model_list 条目的 `model` 字段）。
    model: String,
    /// 单请求墙钟预算秒数（`tools.image_gen.timeout_secs`）。
    timeout_secs: u64,
    /// 输出根目录（通常 `<workspace>/images`；绝对路径，装配期定死）。
    output_dir: PathBuf,
}

impl GenerateImageTool {
    pub fn new(
        api_base: String,
        api_key: String,
        model: String,
        timeout_secs: u64,
        output_dir: PathBuf,
    ) -> Self {
        Self {
            api_base,
            api_key,
            model,
            timeout_secs,
            output_dir,
        }
    }

    /// 把用户给的 `output` 相对路径钉死在 `output_dir` 内。
    ///
    /// 拒绝：空/纯目录形态、绝对路径、盘符/根前缀、任何 `..` 分量。
    /// 反斜杠先归一为分隔符（Windows 习惯输入兼容 + Linux 上把含反斜杠
    /// 的怪文件名一并按路径语义拒绝，跨平台行为一致）。
    /// 返回 `None` = 非法（调用方诚实报错，不猜测修正）。
    fn sanitize_rel(output: &str) -> Option<PathBuf> {
        let normalized = output.trim().replace('\\', "/");
        if normalized.is_empty() {
            return None;
        }
        let p = Path::new(&normalized);
        if p.is_absolute() {
            return None;
        }
        let mut safe = PathBuf::new();
        for comp in p.components() {
            match comp {
                Component::Normal(seg) => {
                    let seg_str = seg.to_string_lossy().to_string();
                    let stem = seg_str
                        .split_once('.')
                        .map(|(s, _)| s.to_string())
                        .unwrap_or_else(|| seg_str.clone());
                    // Windows 保留设备名（aux/con/nul 等）在部分 API 上有
                    // 特殊语义，宁可错杀——生成物命名不值得冒险。
                    if ["aux", "con", "prn", "nul", "com1", "lpt1"]
                        .iter()
                        .any(|r| stem.eq_ignore_ascii_case(r))
                        // 盘符/NTFS ADS 形态（Linux 上 "C:/..." 不是绝对路径，
                        // 冒号防御补上跨平台一致性）。
                        || seg_str.contains(':')
                    {
                        return None;
                    }
                    safe.push(seg);
                }
                Component::ParentDir
                | Component::RootDir
                | Component::Prefix(_)
                | Component::CurDir => {
                    return None;
                }
            }
        }
        // 必须最终落在文件名上（纯目录形态无意义）。
        if safe.as_os_str().is_empty() || safe.extension().is_none() {
            return None;
        }
        Some(safe)
    }

    /// 默认产物名：`img_{hash16}.png`（DefaultHasher 混入毫秒时间戳——
    /// 只做命名去碰，不做安全承诺）。
    fn default_filename(prompt: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        prompt.hash(&mut h);
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
            .hash(&mut h);
        format!("img_{:x}.png", h.finish())
    }

    /// 请求 + 解码 + 落盘的公共执行体（execute 与测试共用）。
    async fn run(
        &self,
        prompt: &str,
        size: Option<String>,
        rel: PathBuf,
    ) -> Result<(PathBuf, usize), String> {
        let request = nemesis_providers::images::ImageRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            size,
        };
        let result = nemesis_providers::images::generate_image(
            &self.api_base,
            &self.api_key,
            &request,
            self.timeout_secs,
        )
        .await?;

        let bytes = base64::engine::general_purpose::STANDARD
            .decode(result.b64_json.trim())
            .map_err(|e| format!("b64 解码失败: {e}"))?;
        if bytes.is_empty() {
            return Err("图像字节为空".to_string());
        }
        let out_path = self.output_dir.join(&rel);
        if let Some(parent) = out_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Err(format!("创建输出目录失败: {e}"));
        }
        std::fs::write(&out_path, &bytes).map_err(|e| format!("写入图像文件失败: {e}"))?;
        Ok((out_path, bytes.len()))
    }
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::Tool for GenerateImageTool {
    fn description(&self) -> String {
        "Generate an image from a text prompt via the configured image model \
         (OpenAI-compatible images/generations endpoint). Saves the PNG under \
         the workspace images/ directory and returns its path and size — use \
         that path for later reference (do not expect image bytes back)."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What to draw (text description of the image)"
                },
                "size": {
                    "type": "string",
                    "description": "Image size like \"1024x1024\" (optional; endpoint default if omitted)"
                },
                "output": {
                    "type": "string",
                    "description": "Relative filename under the workspace images/ directory, e.g. \"mockups/login.png\" (optional; auto name if omitted). Absolute paths and '..' are rejected."
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let parsed: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("args 非 JSON: {e}"))?;
        let Some(prompt) = parsed.get("prompt").and_then(|v| v.as_str()) else {
            return Err("missing 'prompt' argument".to_string());
        };
        let size = parsed
            .get("size")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let rel = match parsed.get("output").and_then(|v| v.as_str()) {
            Some(o) => Self::sanitize_rel(o).ok_or_else(|| {
                "非法 output 路径：只接受 images/ 目录内的相对文件路径（拒绝绝对路径与 '..'）"
                    .to_string()
            })?,
            None => PathBuf::from(Self::default_filename(prompt)),
        };

        let (out_path, n_bytes) = self.run(prompt, size, rel).await?;

        // 用量日志（D15：不接计价——图像端点计价口径与 LLM 价目表不同源，
        // 只记次数 + 模型 + 字节数，供日志侧聚合）。
        tracing::info!(
            tool = "generate_image",
            model = %self.model,
            bytes = n_bytes,
            path = %out_path.to_string_lossy(),
            "image generated"
        );

        // 结果只回路径 + 元数据，不回传字节（LLM 上下文承载不了图像字节；
        // 多模态验收走 M5 资产通道按路径读）。
        let meta = serde_json::json!({
            "path": out_path.to_string_lossy(),
            "bytes": n_bytes,
            "model": self.model,
            "size": parsed.get("size").and_then(|v| v.as_str()).unwrap_or("endpoint-default"),
        });
        Ok(meta.to_string())
    }
}

#[cfg(test)]
mod tests;
