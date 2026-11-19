// ---------------------------------------------------------------------------
// (Phase 5 图像工具层) screen_capture / desktop_automation 薄适配器——
// 委托 nemesis-tools 真实实现（I2CTool 同款模式）。
// ---------------------------------------------------------------------------

use super::*;

// nemesis-tools 侧 Tool trait 以 `_` 导入（方法解析用，与 loop 侧 Tool 不
// 冲突——两个 trait 的实现类型不相交）。
use nemesis_tools::registry::Tool as _;

/// 从工具结果文本提取截图落盘路径（"Screenshot saved to <path>" 形态——
/// ScreenCaptureTool / DesktopTool 两实现的成功文案同前缀）。
/// 提取后验存在 + magic byte，验不过 = 不入图槽（文本照常回灌，模型仍能
/// 用 read_image 重读该路径）。返回 None = 结果无截图产物。
fn extract_saved_screenshot_path(result: &str) -> Option<String> {
    let idx = result.find("saved to ")?;
    let rest = &result[idx + "saved to ".len()..];
    let end = rest
        .find('\n')
        .or_else(|| rest.find(" ("))
        .unwrap_or(rest.len());
    let candidate = rest[..end].trim().trim_matches('"');
    if candidate.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(candidate);
    // 验真：存在 + 图片 magic（防文案解析误捕非图片路径）。
    if !path.is_file() {
        return None;
    }
    match std::fs::File::open(&path) {
        Ok(mut f) => {
            use std::io::Read;
            let mut buf = [0u8; 12];
            match f.read(&mut buf) {
                Ok(n) if crate::image_path_detector::sniff_magic(&buf[..n]) => {
                    Some(candidate.to_string())
                }
                _ => None,
            }
        }
        Err(_) => None,
    }
}

/// `screen_capture` 薄适配器：委托 [`nemesis_tools::screen_capture::ScreenCaptureTool`]
/// （PowerShell 后端 + window-mcp 委托，截图落 `{workspace}/temp/`）。
/// 成功且提取到截图路径 → push 进 `RequestContext.tool_images` 槽（随
/// tool 轮落史 → 模型下轮看见截图）。安全分级 FileWrite（HIGH，写 temp/
/// 文件 + 隐私敏感面）走静态表既有条目。
pub struct ScreenCaptureToolAdapter {
    inner: nemesis_tools::screen_capture::ScreenCaptureTool,
}

impl ScreenCaptureToolAdapter {
    /// 生产构造：workspace 根决定截图落盘目录；MCP caller 恒 None（loop
    /// 侧无 window-mcp 装配面，PowerShell 后端兜底）。
    pub fn new(workspace: std::path::PathBuf) -> Self {
        Self {
            inner: nemesis_tools::screen_capture::ScreenCaptureTool::new(workspace, None),
        }
    }
}

#[async_trait]
impl Tool for ScreenCaptureToolAdapter {
    fn description(&self) -> String {
        self.inner.description().to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
        let result = self.inner.execute(&val).await;
        if result.is_error {
            return Err(result.for_llm);
        }
        // 截图产物入图槽（提取失败 = 纯文本结果，不影响主链路）。
        if let Some(path) = extract_saved_screenshot_path(&result.for_llm) {
            context.push_tool_image(path);
        }
        Ok(result.for_llm)
    }
}

/// `desktop_automation` 薄适配器：委托 [`nemesis_tools::desktop_automation::DesktopTool`]
/// （find_window/list_windows/click_at/type_text/take_screenshot/
/// get_window_text 六动作，Windows PowerShell 后端，非 Windows 运行期
/// 诚实报错）。take_screenshot 成功产物同 [`ScreenCaptureToolAdapter`] 入图槽。
/// 安全分级 ProcessSpawn（HIGH，键鼠注入 = 操纵 GUI 程序）走静态表。
pub struct DesktopAutomationToolAdapter {
    inner: nemesis_tools::desktop_automation::DesktopTool,
}

impl DesktopAutomationToolAdapter {
    /// 生产构造：workspace 根决定截图落盘目录。
    pub fn new(workspace: std::path::PathBuf) -> Self {
        Self {
            inner: nemesis_tools::desktop_automation::DesktopTool::new(workspace, None),
        }
    }
}

#[async_trait]
impl Tool for DesktopAutomationToolAdapter {
    fn description(&self) -> String {
        self.inner.description().to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let val: serde_json::Value =
            serde_json::from_str(args).map_err(|_| "Invalid JSON arguments".to_string())?;
        let result = self.inner.execute(&val).await;
        if result.is_error {
            return Err(result.for_llm);
        }
        if let Some(path) = extract_saved_screenshot_path(&result.for_llm) {
            context.push_tool_image(path);
        }
        Ok(result.for_llm)
    }
}

#[cfg(test)]
mod tests;
