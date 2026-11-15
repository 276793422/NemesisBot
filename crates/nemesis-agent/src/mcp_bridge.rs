//! Bridge between `nemesis_mcp::adapter::Tool` and the agent's `Tool` trait.

/// Wrapper that adapts an `adapter::Tool` into the agent's `Tool` trait.
pub(crate) struct McpToolBridge {
    inner: Box<dyn nemesis_mcp::adapter::Tool>,
}

/// 解析 adapter 文本中的 `[Image saved: <path>]` 标记（nemesis-mcp
/// `image_content_note` 产出，Phase 5 F1），返回全部落盘路径。
/// 防御性验真（存在 + 图片 magic）后由调用方推 `tool_images` 槽；验不过
/// 的标记静默跳过（文本照常回灌，模型仍能 read_image 重读）。
fn extract_saved_image_paths(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(idx) = rest.find("[Image saved: ") {
        let after = &rest[idx + "[Image saved: ".len()..];
        let Some(end) = after.find(']') else { break };
        let candidate = after[..end].trim();
        if !candidate.is_empty() {
            let path = std::path::PathBuf::from(candidate);
            if path.is_file() {
                let ok = std::fs::File::open(&path).map(|mut f| {
                    use std::io::Read;
                    let mut buf = [0u8; 12];
                    f.read(&mut buf)
                        .map(|n| crate::image_path_detector::sniff_magic(&buf[..n]))
                        .unwrap_or(false)
                });
                if matches!(ok, Ok(true)) {
                    out.push(candidate.to_string());
                }
            }
        }
        rest = &after[end + 1..];
    }
    out
}

impl McpToolBridge {
    pub fn new(tool: Box<dyn nemesis_mcp::adapter::Tool>) -> Self {
        Self { inner: tool }
    }
}

#[async_trait::async_trait]
impl crate::r#loop::Tool for McpToolBridge {
    async fn execute(
        &self,
        args: &str,
        context: &crate::context::RequestContext,
    ) -> Result<String, String> {
        let args_value = serde_json::from_str(args).unwrap_or(serde_json::json!({}));
        let result = self.inner.execute(args_value).await;
        if result.is_error {
            let msg = if result.content.is_empty() {
                "MCP tool returned an error".to_string()
            } else {
                result.content
            };
            Err(msg)
        } else {
            // MCP 落盘图像（Phase 5 F1）：文本里的 `[Image saved: <path>]`
            // 标记 → 路径推 tool_images 槽，随 tool 轮落史（apply_tool_guards
            // take）→ 模型下轮经 vision 通路看见图像。
            for path in extract_saved_image_paths(&result.content) {
                context.push_tool_image(path);
            }
            Ok(result.content)
        }
    }

    fn description(&self) -> String {
        self.inner.definition().description.clone()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.definition().parameters.clone()
    }

    // （治理补全）：MCP annotations 透传（快照/镜像基线消费）。
    fn read_only_hint(&self) -> Option<bool> {
        self.inner
            .definition()
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
    }

    fn destructive_hint(&self) -> Option<bool> {
        self.inner
            .definition()
            .annotations
            .as_ref()
            .and_then(|a| a.destructive_hint)
    }
}

#[cfg(test)]
mod tests;
