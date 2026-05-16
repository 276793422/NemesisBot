//! Desktop automation tool for Windows.
//!
//! Provides desktop UI automation through two backends:
//! 1. MCP backend: delegates to a window-mcp server (recommended)
//! 2. Standalone backend: uses PowerShell for basic operations (fallback)
//!
//! Port of Go's `module/tools/desktop_automation.go`.

use crate::browser::MCPToolCaller;
use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::Mutex;

/// Default per-operation timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Desktop action types.
#[derive(Debug, Clone, PartialEq)]
pub enum DesktopAction {
    FindWindow,
    ListWindows,
    ClickAt,
    TypeText,
    Screenshot,
    GetWindowText,
}

impl std::fmt::Display for DesktopAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DesktopAction::FindWindow => write!(f, "find_window"),
            DesktopAction::ListWindows => write!(f, "list_windows"),
            DesktopAction::ClickAt => write!(f, "click_at"),
            DesktopAction::TypeText => write!(f, "type_text"),
            DesktopAction::Screenshot => write!(f, "take_screenshot"),
            DesktopAction::GetWindowText => write!(f, "get_window_text"),
        }
    }
}

impl std::str::FromStr for DesktopAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "find_window" => Ok(DesktopAction::FindWindow),
            "list_windows" => Ok(DesktopAction::ListWindows),
            "click_at" => Ok(DesktopAction::ClickAt),
            "type_text" => Ok(DesktopAction::TypeText),
            "take_screenshot" => Ok(DesktopAction::Screenshot),
            "get_window_text" => Ok(DesktopAction::GetWindowText),
            _ => Err(format!("unknown desktop action: {}", s)),
        }
    }
}

/// Simplified window info for JSON deserialization from PowerShell output.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WindowInfo {
    hwnd: String,
    title: String,
    #[serde(rename = "class_name", default)]
    class_name: String,
    #[serde(default)]
    left: i64,
    #[serde(default)]
    top: i64,
    #[serde(default)]
    width: i64,
    #[serde(default)]
    height: i64,
}

/// Desktop tool provides desktop UI automation for Windows.
///
/// Supports two execution backends:
/// - MCP backend: delegates to a window-mcp server for full native Windows API support
/// - Standalone backend: uses PowerShell for basic operations (fallback)
pub struct DesktopTool {
    mcp_caller: Option<Arc<dyn MCPToolCaller>>,
    workspace: PathBuf,
    timeout: Arc<Mutex<Duration>>,
}

impl DesktopTool {
    /// Create a new DesktopTool.
    ///
    /// The `mcp_caller` may be `None`, in which case only standalone
    /// (PowerShell-based) operations will be available.
    pub fn new(workspace: PathBuf, mcp_caller: Option<Arc<dyn MCPToolCaller>>) -> Self {
        Self {
            mcp_caller,
            workspace,
            timeout: Arc::new(Mutex::new(DEFAULT_TIMEOUT)),
        }
    }

    /// Set the per-operation timeout (default 30s).
    pub async fn set_timeout(&self, d: Duration) {
        let mut timeout = self.timeout.lock().await;
        *timeout = d;
    }

    /// Check if MCP backend is available and connected.
    fn has_mcp(&self) -> bool {
        self.mcp_caller
            .as_ref()
            .map(|c| c.is_connected())
            .unwrap_or(false)
    }

    // --------------- action implementations ---------------

    /// Execute find_window action.
    async fn execute_find_window(&self, args: &serde_json::Value) -> ToolResult {
        let title = match args["title"].as_str() {
            Some(t) if !t.is_empty() => t,
            _ => return ToolResult::error("parameter 'title' is required for find_window action"),
        };

        if self.has_mcp() {
            let mcp_args = serde_json::json!({
                "title_contains": title
            });
            match self
                .mcp_caller
                .as_ref()
                .unwrap()
                .call_tool("find_window_by_title", &mcp_args)
                .await
            {
                Ok(result) => return ToolResult::success(&result),
                Err(e) => return ToolResult::error(&format!("MCP find_window failed: {}", e)),
            }
        }

        // Standalone fallback
        self.standalone_find_window(title).await
    }

    /// Execute list_windows action.
    async fn execute_list_windows(&self, args: &serde_json::Value) -> ToolResult {
        if self.has_mcp() {
            let mut mcp_args = serde_json::json!({
                "filter_visible": true
            });
            if let Some(title) = args["title"].as_str() {
                mcp_args["title_contains"] = serde_json::Value::String(title.to_string());
            }
            match self
                .mcp_caller
                .as_ref()
                .unwrap()
                .call_tool("enumerate_windows", &mcp_args)
                .await
            {
                Ok(result) => return ToolResult::success(&result),
                Err(e) => return ToolResult::error(&format!("MCP list_windows failed: {}", e)),
            }
        }

        self.standalone_list_windows().await
    }

    /// Execute click_at action.
    async fn execute_click_at(&self, args: &serde_json::Value) -> ToolResult {
        let x = match args["x"].as_i64() {
            Some(v) => v,
            None => return ToolResult::error("parameters 'x' and 'y' are required for click_at action"),
        };
        let y = match args["y"].as_i64() {
            Some(v) => v,
            None => return ToolResult::error("parameters 'x' and 'y' are required for click_at action"),
        };

        let button = args["button"].as_str().unwrap_or("left").to_string();

        if self.has_mcp() {
            let mut mcp_args = serde_json::json!({
                "x": x,
                "y": y,
                "button": button
            });
            if let Some(hwnd) = args["hwnd"].as_str() {
                mcp_args["hwnd"] = serde_json::Value::String(hwnd.to_string());
            }
            match self
                .mcp_caller
                .as_ref()
                .unwrap()
                .call_tool("click_window", &mcp_args)
                .await
            {
                Ok(result) => {
                    return ToolResult::silent(&format!(
                        "Clicked at ({}, {}) with {} button.\n{}",
                        x, y, button, result
                    ))
                }
                Err(e) => return ToolResult::error(&format!("MCP click_at failed: {}", e)),
            }
        }

        self.standalone_click_at(x, y, &button).await
    }

    /// Execute type_text action.
    async fn execute_type_text(&self, args: &serde_json::Value) -> ToolResult {
        let text = match args["text"].as_str() {
            Some(t) if !t.is_empty() => t,
            _ => return ToolResult::error("parameter 'text' is required for type_text action"),
        };

        if self.has_mcp() {
            let mut mcp_args = serde_json::json!({
                "key": text
            });
            if let Some(hwnd) = args["hwnd"].as_str() {
                mcp_args["hwnd"] = serde_json::Value::String(hwnd.to_string());
            }
            match self
                .mcp_caller
                .as_ref()
                .unwrap()
                .call_tool("send_key_to_window", &mcp_args)
                .await
            {
                Ok(result) => return ToolResult::silent(&format!("Typed text.\n{}", result)),
                Err(e) => return ToolResult::error(&format!("MCP type_text failed: {}", e)),
            }
        }

        self.standalone_type_text(text).await
    }

    /// Execute screenshot action.
    async fn execute_screenshot(&self, args: &serde_json::Value) -> ToolResult {
        let x = args["x"].as_i64();
        let y = args["y"].as_i64();
        let w = args["width"].as_i64();
        let h = args["height"].as_i64();

        if self.has_mcp() {
            let mut mcp_args = serde_json::json!({});
            if let Some(xv) = x {
                mcp_args["x"] = serde_json::Value::Number(xv.into());
            }
            if let Some(yv) = y {
                mcp_args["y"] = serde_json::Value::Number(yv.into());
            }
            if let Some(wv) = w {
                mcp_args["width"] = serde_json::Value::Number(wv.into());
            }
            if let Some(hv) = h {
                mcp_args["height"] = serde_json::Value::Number(hv.into());
            }
            if let Some(hwnd) = args["hwnd"].as_str() {
                mcp_args["hwnd"] = serde_json::Value::String(hwnd.to_string());
            }
            match self
                .mcp_caller
                .as_ref()
                .unwrap()
                .call_tool("capture_screenshot_to_file", &mcp_args)
                .await
            {
                Ok(result) => return ToolResult::silent(&format!("Screenshot captured.\n{}", result)),
                Err(e) => return ToolResult::error(&format!("MCP screenshot failed: {}", e)),
            }
        }

        self.standalone_screenshot(x, y, w, h).await
    }

    /// Execute get_window_text action.
    async fn execute_get_window_text(&self, args: &serde_json::Value) -> ToolResult {
        let hwnd = args["hwnd"].as_str().unwrap_or("");
        let title = args["title"].as_str().unwrap_or("");

        if hwnd.is_empty() && title.is_empty() {
            return ToolResult::error(
                "parameter 'hwnd' or 'title' is required for get_window_text action",
            );
        }

        if self.has_mcp() {
            let resolved_hwnd = if hwnd.is_empty() {
                // Find the window first by title
                let find_args = serde_json::json!({
                    "title_contains": title
                });
                match self
                    .mcp_caller
                    .as_ref()
                    .unwrap()
                    .call_tool("find_window_by_title", &find_args)
                    .await
                {
                    Ok(find_result) => {
                        // Parse hwnd from find result
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&find_result)
                        {
                            parsed["hwnd"].as_str().unwrap_or("").to_string()
                        } else {
                            String::new()
                        }
                    }
                    Err(e) => {
                        return ToolResult::error(&format!(
                            "MCP find_window for get_window_text failed: {}",
                            e
                        ))
                    }
                }
            } else {
                hwnd.to_string()
            };

            if resolved_hwnd.is_empty() {
                return ToolResult::error(
                    "could not resolve window handle for get_window_text",
                );
            }

            let text_args = serde_json::json!({
                "hwnd": resolved_hwnd
            });
            match self
                .mcp_caller
                .as_ref()
                .unwrap()
                .call_tool("get_window_text", &text_args)
                .await
            {
                Ok(result) => ToolResult::success(&result),
                Err(e) => ToolResult::error(&format!("MCP get_window_text failed: {}", e)),
            }
        } else {
            ToolResult::error(
                "get_window_text requires a window-mcp server (no standalone fallback available)",
            )
        }
    }

    // --------------- standalone (PowerShell) fallback implementations ---------------

    /// Find window by title using PowerShell.
    async fn standalone_find_window(&self, title: &str) -> ToolResult {
        match self.enumerate_windows_ps().await {
            Ok(windows) => {
                let title_lower = title.to_lowercase();
                let matches: Vec<&WindowInfo> = windows
                    .iter()
                    .filter(|w| w.title.to_lowercase().contains(&title_lower))
                    .collect();

                if matches.is_empty() {
                    return ToolResult::success(&format!(
                        "No windows found matching title: {}",
                        title
                    ));
                }

                match serde_json::to_string_pretty(&matches) {
                    Ok(data) => ToolResult::success(&format!(
                        "Found {} window(s) matching '{}':\n{}",
                        matches.len(),
                        title,
                        data
                    )),
                    Err(e) => ToolResult::error(&format!("failed to serialize windows: {}", e)),
                }
            }
            Err(e) => ToolResult::error(&format!("failed to enumerate windows: {}", e)),
        }
    }

    /// List visible windows using PowerShell.
    async fn standalone_list_windows(&self) -> ToolResult {
        match self.enumerate_windows_ps().await {
            Ok(windows) => match serde_json::to_string_pretty(&windows) {
                Ok(data) => ToolResult::success(&format!(
                    "Found {} visible window(s):\n{}",
                    windows.len(),
                    data
                )),
                Err(e) => ToolResult::error(&format!("failed to serialize windows: {}", e)),
            },
            Err(e) => ToolResult::error(&format!("failed to enumerate windows: {}", e)),
        }
    }

    /// Click at screen coordinates using PowerShell.
    async fn standalone_click_at(&self, x: i64, y: i64, button: &str) -> ToolResult {
        let mouse_down;
        let mouse_up;
        match button {
            "right" => {
                mouse_down = "0x0008";
                mouse_up = "0x0010";
            }
            "middle" => {
                mouse_down = "0x0020";
                mouse_up = "0x0040";
            }
            _ => {
                // left
                mouse_down = "0x0002";
                mouse_up = "0x0004";
            }
        }

        let script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point({}, {})
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Mouse {{
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, IntPtr dwExtraInfo);
}}
"@
[Mouse]::mouse_event({}, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 50
[Mouse]::mouse_event({}, 0, 0, 0, [IntPtr]::Zero)
"#,
            x, y, mouse_down, mouse_up
        );

        let timeout = *self.timeout.lock().await;
        match self.run_powershell(&script, timeout).await {
            Ok(_) => ToolResult::silent(&format!(
                "Clicked at ({}, {}) with {} button",
                x, y, button
            )),
            Err(e) => ToolResult::error(&format!("click_at failed: {}", e)),
        }
    }

    /// Type text using PowerShell SendKeys.
    async fn standalone_type_text(&self, text: &str) -> ToolResult {
        // Escape single quotes for PowerShell
        let escaped = text.replace('\'', "''");

        let script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Start-Sleep -Milliseconds 100
[System.Windows.Forms.SendKeys]::SendWait('{}')
"#,
            escaped
        );

        let timeout = *self.timeout.lock().await;
        match self.run_powershell(&script, timeout).await {
            Ok(_) => ToolResult::silent(&format!("Typed {} characters", text.len())),
            Err(e) => ToolResult::error(&format!("type_text failed: {}", e)),
        }
    }

    /// Take a screenshot using PowerShell System.Drawing.
    async fn standalone_screenshot(
        &self,
        x: Option<i64>,
        y: Option<i64>,
        w: Option<i64>,
        h: Option<i64>,
    ) -> ToolResult {
        // Default to full primary screen if no region specified
        let (final_x, final_y, final_w, final_h) = if x.is_none() || y.is_none() || w.is_none() || h.is_none() {
            let screen_script = r#"
Add-Type -AssemblyName System.Windows.Forms
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
Write-Output "$($screen.X),$($screen.Y),$($screen.Width),$($screen.Height)"
"#;
            let timeout = Duration::from_secs(10);
            match self.run_powershell(screen_script, timeout).await {
                Ok(out) => {
                    let parts: Vec<&str> = out.trim().split(',').collect();
                    if parts.len() == 4 {
                        let sx = x.unwrap_or_else(|| parts[0].parse().unwrap_or(0));
                        let sy = y.unwrap_or_else(|| parts[1].parse().unwrap_or(0));
                        let sw = w.unwrap_or_else(|| parts[2].parse().unwrap_or(0));
                        let sh = h.unwrap_or_else(|| parts[3].parse().unwrap_or(0));
                        (sx, sy, sw, sh)
                    } else {
                        (0, 0, 1920, 1080) // fallback defaults
                    }
                }
                Err(_) => (0, 0, 1920, 1080),
            }
        } else {
            (x.unwrap(), y.unwrap(), w.unwrap(), h.unwrap())
        };

        // Ensure temp directory exists
        let temp_dir = self.workspace.join("temp");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return ToolResult::error(&format!("failed to create temp directory: {}", e));
        }

        let timestamp = chrono::Utc::now().timestamp_millis();
        let filename = format!("desktop_screenshot_{}.png", timestamp);
        let output_path = temp_dir.join(&filename);
        let output_str = output_path.to_string_lossy().to_string().replace('\'', "''");

        let script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bounds = New-Object System.Drawing.Rectangle({}, {}, {}, {})
$bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('{}', [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "OK"
"#,
            final_x, final_y, final_w, final_h, output_str
        );

        let timeout = *self.timeout.lock().await;
        match self.run_powershell(&script, timeout).await {
            Ok(_) => ToolResult::silent(&format!(
                "Screenshot saved to {} ({}x{} at {},{})",
                output_path.display(),
                final_w,
                final_h,
                final_x,
                final_y
            )),
            Err(e) => ToolResult::error(&format!("screenshot failed: {}", e)),
        }
    }

    /// Enumerate visible windows via PowerShell.
    async fn enumerate_windows_ps(&self) -> Result<Vec<WindowInfo>, String> {
        let script = r#"
$procs = Get-Process | Where-Object { $_.MainWindowTitle -ne '' -and $_.MainWindowHandle -ne 0 }
$results = @()
foreach ($p in $procs) {
    $results += @{
        hwnd       = ('HWND(0x{0:X})' -f [int]$p.MainWindowHandle)
        title      = $p.MainWindowTitle
        class_name = ''
        left       = 0
        top        = 0
        width      = 0
        height     = 0
    }
}
$results | ConvertTo-Json -Compress
"#;

        let timeout = Duration::from_secs(15);
        let out = self.run_powershell(script, timeout).await.map_err(|e| e)?;

        let trimmed = out.trim();
        if trimmed.is_empty() || trimmed == "null" {
            return Ok(Vec::new());
        }

        // Try parsing as array first, then as single object
        if let Ok(windows) = serde_json::from_str::<Vec<WindowInfo>>(trimmed) {
            Ok(windows)
        } else if let Ok(single) = serde_json::from_str::<WindowInfo>(trimmed) {
            Ok(vec![single])
        } else {
            Err("failed to parse window list from PowerShell output".to_string())
        }
    }

    /// Execute a PowerShell script and return its stdout.
    async fn run_powershell(&self, script: &str, timeout: Duration) -> Result<String, String> {
        tracing::debug!(
            script_length = script.len(),
            "Running PowerShell script"
        );

        let output = tokio::time::timeout(
            timeout,
            tokio::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .output(),
        )
        .await
        .map_err(|_| "PowerShell execution timed out".to_string())?
        .map_err(|e| format!("failed to execute PowerShell: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let mut err_detail = format!("PowerShell exited with status {}", output.status);
            if !stderr.is_empty() {
                err_detail.push_str(&format!("\nSTDERR: {}", stderr));
            }
            if !stdout.is_empty() {
                err_detail.push_str(&format!("\nSTDOUT: {}", stdout));
            }
            return Err(err_detail);
        }

        Ok(stdout)
    }
}

#[async_trait]
impl Tool for DesktopTool {
    fn name(&self) -> &str {
        "desktop"
    }

    fn description(&self) -> &str {
        "Automate desktop windows on the current machine (Windows only).\n\
         \n\
         Supported actions:\n\
         - find_window:     Find a window by title (partial match) and return its handle and position\n\
         - list_windows:    List all visible windows with title, class, position, and size\n\
         - click_at:        Perform a mouse click at screen coordinates (x, y)\n\
         - type_text:       Send keyboard text input to the foreground window\n\
         - take_screenshot: Capture a screenshot of a window or screen region (saves to workspace/temp/)\n\
         - get_window_text: Retrieve the text content of a window\n\
         \n\
         When a window-mcp server is configured, operations are delegated to it for full \
         native Windows API support. Otherwise a PowerShell-based fallback is used for \
         basic operations."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Desktop action to perform",
                    "enum": [
                        "find_window",
                        "list_windows",
                        "click_at",
                        "type_text",
                        "take_screenshot",
                        "get_window_text"
                    ]
                },
                "title": {
                    "type": "string",
                    "description": "Window title (partial match, used by find_window, get_window_text)"
                },
                "hwnd": {
                    "type": "string",
                    "description": "Window handle, e.g. 'HWND(0x12345)' (used by screenshot, get_window_text)"
                },
                "x": {
                    "type": "integer",
                    "description": "Screen X coordinate for click_at or screenshot region"
                },
                "y": {
                    "type": "integer",
                    "description": "Screen Y coordinate for click_at or screenshot region"
                },
                "width": {
                    "type": "integer",
                    "description": "Width for screenshot region"
                },
                "height": {
                    "type": "integer",
                    "description": "Height for screenshot region"
                },
                "text": {
                    "type": "string",
                    "description": "Text to type into the foreground window (used by type_text)"
                },
                "button": {
                    "type": "string",
                    "description": "Mouse button for click_at: left (default), right, middle",
                    "enum": ["left", "right", "middle"]
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        // Platform check
        if cfg!(not(target_os = "windows")) {
            return ToolResult::error("desktop automation is only supported on Windows");
        }

        let action_str = match args["action"].as_str() {
            Some(a) => a,
            None => return ToolResult::error("parameter 'action' is required"),
        };

        let action = match DesktopAction::from_str(action_str) {
            Ok(a) => a,
            Err(ref e) => return ToolResult::error(e.as_str()),
        };

        match action {
            DesktopAction::FindWindow => self.execute_find_window(args).await,
            DesktopAction::ListWindows => self.execute_list_windows(args).await,
            DesktopAction::ClickAt => self.execute_click_at(args).await,
            DesktopAction::TypeText => self.execute_type_text(args).await,
            DesktopAction::Screenshot => self.execute_screenshot(args).await,
            DesktopAction::GetWindowText => self.execute_get_window_text(args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_desktop_action_from_str() {
        assert_eq!(
            DesktopAction::from_str("find_window").unwrap(),
            DesktopAction::FindWindow
        );
        assert_eq!(
            DesktopAction::from_str("list_windows").unwrap(),
            DesktopAction::ListWindows
        );
        assert_eq!(
            DesktopAction::from_str("click_at").unwrap(),
            DesktopAction::ClickAt
        );
        assert_eq!(
            DesktopAction::from_str("type_text").unwrap(),
            DesktopAction::TypeText
        );
        assert_eq!(
            DesktopAction::from_str("take_screenshot").unwrap(),
            DesktopAction::Screenshot
        );
        assert_eq!(
            DesktopAction::from_str("get_window_text").unwrap(),
            DesktopAction::GetWindowText
        );
        assert!(DesktopAction::from_str("invalid").is_err());
    }

    #[test]
    fn test_desktop_action_display() {
        assert_eq!(DesktopAction::FindWindow.to_string(), "find_window");
        assert_eq!(DesktopAction::ListWindows.to_string(), "list_windows");
        assert_eq!(DesktopAction::ClickAt.to_string(), "click_at");
        assert_eq!(DesktopAction::TypeText.to_string(), "type_text");
        assert_eq!(DesktopAction::Screenshot.to_string(), "take_screenshot");
        assert_eq!(DesktopAction::GetWindowText.to_string(), "get_window_text");
    }

    #[tokio::test]
    async fn test_desktop_tool_metadata() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        assert_eq!(tool.name(), "desktop");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_desktop_tool_missing_action() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'action' is required"));
    }

    #[tokio::test]
    async fn test_desktop_tool_unknown_action() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"action": "unknown_action"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("unknown desktop action"));
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_missing_title() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"action": "find_window"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'title' is required"));
    }

    #[tokio::test]
    async fn test_desktop_tool_click_at_missing_coords() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"action": "click_at"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'x' and 'y' are required"));
    }

    #[tokio::test]
    async fn test_desktop_tool_type_text_missing_text() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"action": "type_text"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'text' is required"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_missing_params() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"action": "get_window_text"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'hwnd' or 'title' is required"));
    }

    #[tokio::test]
    async fn test_desktop_tool_parameters_schema() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let params = tool.parameters();

        // Verify required fields
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("action")));

        // Verify action enum values
        let action_enum = params["properties"]["action"]["enum"].as_array().unwrap();
        assert_eq!(action_enum.len(), 6);
        assert!(action_enum.iter().any(|v| v.as_str() == Some("find_window")));
        assert!(action_enum.iter().any(|v| v.as_str() == Some("list_windows")));
        assert!(action_enum.iter().any(|v| v.as_str() == Some("click_at")));
        assert!(action_enum.iter().any(|v| v.as_str() == Some("type_text")));
        assert!(action_enum.iter().any(|v| v.as_str() == Some("take_screenshot")));
        assert!(action_enum.iter().any(|v| v.as_str() == Some("get_window_text")));
    }

    #[tokio::test]
    async fn test_desktop_tool_no_mcp() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        assert!(!tool.has_mcp());
    }

    // ============================================================
    // Additional desktop automation tests
    // ============================================================

    #[test]
    fn test_desktop_action_all_variants() {
        assert!(DesktopAction::from_str("find_window").is_ok());
        assert!(DesktopAction::from_str("list_windows").is_ok());
        assert!(DesktopAction::from_str("click_at").is_ok());
        assert!(DesktopAction::from_str("type_text").is_ok());
        assert!(DesktopAction::from_str("take_screenshot").is_ok());
        assert!(DesktopAction::from_str("get_window_text").is_ok());
    }

    #[test]
    fn test_desktop_action_display_roundtrip() {
        let actions = [
            DesktopAction::FindWindow,
            DesktopAction::ListWindows,
            DesktopAction::ClickAt,
            DesktopAction::TypeText,
            DesktopAction::Screenshot,
            DesktopAction::GetWindowText,
        ];
        for action in &actions {
            let s = action.to_string();
            let parsed = DesktopAction::from_str(&s);
            assert_eq!(parsed.unwrap(), *action);
        }
    }

    #[test]
    fn test_window_info_serialization() {
        let info = WindowInfo {
            hwnd: "HWND(0x12345)".to_string(),
            title: "Test Window".to_string(),
            class_name: "TestClass".to_string(),
            left: 100,
            top: 200,
            width: 800,
            height: 600,
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: WindowInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hwnd, "HWND(0x12345)");
        assert_eq!(restored.title, "Test Window");
        assert_eq!(restored.width, 800);
    }

    #[test]
    fn test_window_info_deserialization_from_json() {
        let json = r#"{"hwnd":"HWND(0xABCD)","title":"MyApp","class_name":"Chrome","left":0,"top":0,"width":1920,"height":1080}"#;
        let info: WindowInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.hwnd, "HWND(0xABCD)");
        assert_eq!(info.title, "MyApp");
        assert_eq!(info.width, 1920);
    }

    #[tokio::test]
    async fn test_desktop_tool_description_not_empty() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        assert!(tool.description().len() > 50);
    }

    #[tokio::test]
    async fn test_desktop_tool_parameters_valid() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert!(params["required"].is_array());
    }

    #[tokio::test]
    async fn test_desktop_tool_missing_action_returns_error() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("action"));
    }

    #[tokio::test]
    async fn test_desktop_click_at_with_mcp_and_button() {
        // Use MCP mock by reusing the test module's mock
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("clicked".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool
            .execute(&serde_json::json!({"action": "click_at", "x": 50, "y": 100, "button": "right"}))
            .await;
        assert!(!result.is_error);
    }

    // ============================================================
    // Additional tests for coverage improvement
    // ============================================================

    #[test]
    fn test_desktop_action_equality() {
        assert_eq!(DesktopAction::FindWindow, DesktopAction::FindWindow);
        assert_ne!(DesktopAction::FindWindow, DesktopAction::ListWindows);
        assert_ne!(DesktopAction::ClickAt, DesktopAction::TypeText);
    }

    #[test]
    fn test_desktop_action_debug_format() {
        assert!(format!("{:?}", DesktopAction::Screenshot).contains("Screenshot"));
        assert!(format!("{:?}", DesktopAction::GetWindowText).contains("GetWindowText"));
    }

    #[test]
    fn test_window_info_default_values() {
        let json = r#"{"hwnd": "1", "title": "w"}"#;
        let info: WindowInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.class_name, "");
        assert_eq!(info.left, 0);
        assert_eq!(info.top, 0);
        assert_eq!(info.width, 0);
        assert_eq!(info.height, 0);
    }

    #[tokio::test]
    async fn test_desktop_tool_set_timeout_value() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let custom = Duration::from_secs(120);
        tool.set_timeout(custom).await;
        let t = *tool.timeout.lock().await;
        assert_eq!(t, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn test_desktop_tool_has_mcp_false_without_caller() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        assert!(!tool.has_mcp());
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_empty_title() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": ""})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_click_at_only_x() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 50})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_click_at_only_y() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({"action": "click_at", "y": 50})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_type_text_empty() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({"action": "type_text", "text": ""})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_with_hwnd_no_mcp() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "12345"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("window-mcp"));
    }

    #[tokio::test]
    async fn test_desktop_tool_list_windows_no_mcp() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        // This should try PowerShell and either succeed or fail - just verify no panic
        let _ = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_find_window() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x999)","title":"Test"}"#.to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Test"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_list_windows() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "enumerate_windows" => Ok(r#"[{"hwnd":"1","title":"Win1"}]"#.to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_screenshot() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "capture_screenshot_to_file" => Ok("screenshot saved".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_type_text() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "send_key_to_window" => Ok("keys sent".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
        assert!(!result.is_error);
    }

    #[test]
    fn test_window_info_serialize_deserialize_large_values() {
        let info = WindowInfo {
            hwnd: "HWND(0xFFFFFFFF)".to_string(),
            title: "A".repeat(500),
            class_name: "BigClass".to_string(),
            left: i64::MAX,
            top: i64::MIN,
            width: 7680,
            height: 4320,
        };
        let json = serde_json::to_string(&info).unwrap();
        let restored: WindowInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hwnd, info.hwnd);
        assert_eq!(restored.title.len(), 500);
        assert_eq!(restored.left, i64::MAX);
        assert_eq!(restored.width, 7680);
    }

    // ============================================================
    // MCP error path coverage
    // ============================================================

    #[tokio::test]
    async fn test_desktop_tool_mcp_find_window_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("connection lost".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Test"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("MCP find_window failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_list_windows_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("connection lost".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("MCP list_windows failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_click_at_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("click failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 50, "y": 100})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("MCP click_at failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_type_text_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("type failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("MCP type_text failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_screenshot_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("screenshot failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("MCP screenshot failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_get_window_text_with_hwnd() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "get_window_text" => Ok("Window text content".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "HWND(0x123)"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Window text content"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_get_window_text_by_title() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x456)"}"#.to_string()),
                        "get_window_text" => Ok("Found window text".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "MyApp"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Found window text"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_get_window_text_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("get_text failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "12345"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("MCP get_window_text failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_get_window_text_by_title_find_fails() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "find_window_by_title" => Err("not found".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "Nonexistent"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("find_window"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_get_window_text_by_title_find_returns_no_hwnd() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "find_window_by_title" => Ok(r#"{"title":"MyApp"}"#.to_string()), // No hwnd field
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "MyApp"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("resolve window handle"));
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_click_at_with_hwnd() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("clicked".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({
            "action": "click_at", "x": 50, "y": 100, "hwnd": "HWND(0x999)"
        })).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_type_text_with_hwnd() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("typed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({
            "action": "type_text", "text": "hello", "hwnd": "HWND(0x999)"
        })).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_screenshot_with_region() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("screenshot saved".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({
            "action": "take_screenshot", "x": 0, "y": 0, "width": 800, "height": 600
        })).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_screenshot_with_hwnd() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("screenshot saved".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({
            "action": "take_screenshot", "hwnd": "HWND(0x123)"
        })).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_list_windows_with_title_filter() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                let has_title = args.get("title_contains").is_some();
                Box::pin(async move {
                    match name.as_str() {
                        "enumerate_windows" => {
                            if has_title {
                                Ok(r#"[{"hwnd":"1","title":"Chrome"}]"#.to_string())
                            } else {
                                Ok(r#"[{"hwnd":"1","title":"Win1"}]"#.to_string())
                            }
                        }
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "list_windows", "title": "Chrome"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_mcp_disconnected() {
        struct DisconnectedMCPCaller;
        impl crate::browser::MCPToolCaller for DisconnectedMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("should not be called".to_string()) })
            }
            fn is_connected(&self) -> bool { false }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(DisconnectedMCPCaller)));
        assert!(!tool.has_mcp());
        // Should fall back to standalone PowerShell (or error on non-Windows)
        let _ = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
    }

    // ============================================================
    // Additional coverage tests for 95%+ target
    // ============================================================

    #[test]
    fn test_desktop_tool_new_no_mcp() {
        let tool = DesktopTool::new(PathBuf::from("/tmp"), None);
        assert_eq!(tool.name(), "desktop");
        assert!(!tool.has_mcp());
    }

    #[test]
    fn test_desktop_tool_new_with_mcp() {
        struct MockMCP;
        impl crate::browser::MCPToolCaller for MockMCP {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("ok".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCP)));
        assert!(tool.has_mcp());
    }

    #[test]
    fn test_window_info_deserialize() {
        let json = r#"{"hwnd":"HWND(0x123)","title":"Test Window","class_name":"TestClass","left":10,"top":20,"width":800,"height":600}"#;
        let info: WindowInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.hwnd, "HWND(0x123)");
        assert_eq!(info.title, "Test Window");
        assert_eq!(info.class_name, "TestClass");
        assert_eq!(info.left, 10);
        assert_eq!(info.top, 20);
        assert_eq!(info.width, 800);
        assert_eq!(info.height, 600);
    }

    #[test]
    fn test_window_info_deserialize_defaults() {
        let json = r#"{"hwnd":"HWND(0x1)","title":"Min"}"#;
        let info: WindowInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.hwnd, "HWND(0x1)");
        assert_eq!(info.title, "Min");
        assert_eq!(info.class_name, "");
        assert_eq!(info.left, 0);
        assert_eq!(info.top, 0);
        assert_eq!(info.width, 0);
        assert_eq!(info.height, 0);
    }

    #[test]
    fn test_window_info_serialize_roundtrip() {
        let info = WindowInfo {
            hwnd: "HWND(0x999)".to_string(),
            title: "Roundtrip".to_string(),
            class_name: "RTClass".to_string(),
            left: 100,
            top: 200,
            width: 1024,
            height: 768,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: WindowInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hwnd, info.hwnd);
        assert_eq!(back.title, info.title);
        assert_eq!(back.left, info.left);
    }

    #[test]
    fn test_desktop_tool_parameters_structure() {
        let tool = DesktopTool::new(PathBuf::from("."), None);
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
        assert!(params["required"].as_array().unwrap().contains(&serde_json::json!("action")));
    }

    #[tokio::test]
    async fn test_desktop_tool_missing_action_v2() {
        let tool = DesktopTool::new(PathBuf::from("."), None);
        let result = tool.execute(&serde_json::json!({"action": "nonexistent"})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_mcp_success() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok(r#"{"hwnd":"HWND(0x100)","title":"MyApp"}"#.to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "MyApp"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("HWND(0x100)"));
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("find failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Test"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("find_window failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_missing_title_v2() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("ok".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("title"));
    }

    #[tokio::test]
    async fn test_desktop_tool_list_windows_mcp_error_v2() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("list failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("list_windows failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_click_at_missing_y() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("ok".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 10})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("x") || result.for_llm.contains("y"));
    }

    #[tokio::test]
    async fn test_desktop_tool_click_at_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("click failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 10, "y": 20})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("click_at failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_type_text_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("type failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("type_text failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_type_text_empty_text() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("ok".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "type_text", "text": ""})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("text"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_no_params() {
        struct MockMCPCaller;
        impl crate::browser::MCPToolCaller for MockMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("ok".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(MockMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("hwnd") || result.for_llm.contains("title"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_no_mcp() {
        let tool = DesktopTool::new(PathBuf::from("."), None);
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "123"})).await;
        // No MCP backend - should error about needing MCP
        // On non-Windows, platform check fires first
        if cfg!(target_os = "windows") {
            assert!(result.is_error);
            assert!(result.for_llm.contains("window-mcp") || result.for_llm.contains("requires"));
        }
    }

    #[tokio::test]
    async fn test_desktop_tool_set_timeout() {
        let tool = DesktopTool::new(PathBuf::from("."), None);
        tool.set_timeout(Duration::from_secs(60)).await;
        let timeout = tool.timeout.lock().await;
        assert_eq!(*timeout, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_desktop_tool_screenshot_mcp_success() {
        struct ScreenshotMCPCaller;
        impl crate::browser::MCPToolCaller for ScreenshotMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("screenshot_saved.png".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ScreenshotMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Screenshot"));
    }

    #[tokio::test]
    async fn test_desktop_tool_screenshot_mcp_with_region() {
        struct ScreenshotMCPCaller;
        impl crate::browser::MCPToolCaller for ScreenshotMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let tool_name = tool_name.to_string();
                let args = args.clone();
                Box::pin(async move {
                    assert_eq!(tool_name, "capture_screenshot_to_file");
                    assert_eq!(args["x"], 10);
                    assert_eq!(args["y"], 20);
                    assert_eq!(args["width"], 100);
                    assert_eq!(args["height"], 200);
                    Ok("region_screenshot.png".to_string())
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ScreenshotMCPCaller)));
        let result = tool.execute(&serde_json::json!({
            "action": "take_screenshot", "x": 10, "y": 20, "width": 100, "height": 200
        })).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_desktop_tool_screenshot_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("screenshot failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "take_screenshot"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("screenshot failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_list_windows_mcp_success() {
        struct ListMCPCaller;
        impl crate::browser::MCPToolCaller for ListMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("[{\"hwnd\":\"123\",\"title\":\"Test\"}]".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ListMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Test"));
    }

    #[tokio::test]
    async fn test_desktop_tool_list_windows_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("list failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "list_windows"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("list_windows failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_with_mcp_success() {
        struct FindMCPCaller;
        impl crate::browser::MCPToolCaller for FindMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("{\"hwnd\":\"0x123\",\"title\":\"Chrome\"}".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FindMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Chrome"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Chrome"));
    }

    #[tokio::test]
    async fn test_desktop_tool_find_window_with_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("find failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "find_window", "title": "Chrome"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("find_window failed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_type_text_mcp_success() {
        struct TypeMCPCaller;
        impl crate::browser::MCPToolCaller for TypeMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("typed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(TypeMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "type_text", "text": "hello"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Typed"));
    }

    #[tokio::test]
    async fn test_desktop_tool_click_at_mcp_success() {
        struct ClickMCPCaller;
        impl crate::browser::MCPToolCaller for ClickMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("clicked".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(ClickMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "click_at", "x": 100, "y": 200})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Clicked"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_with_hwnd_mcp() {
        struct TextMCPCaller;
        impl crate::browser::MCPToolCaller for TextMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    if name == "get_window_text" {
                        Ok("Window Title Text".to_string())
                    } else {
                        Ok("{}".to_string())
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(TextMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "0x123"})).await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Window Title Text"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_with_title_mcp_find_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("find failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "Chrome"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("find_window"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_with_title_mcp_find_returns_empty_hwnd() {
        struct EmptyHwndMCPCaller;
        impl crate::browser::MCPToolCaller for EmptyHwndMCPCaller {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    if name == "find_window_by_title" {
                        Ok("{\"hwnd\":\"\"}".to_string())
                    } else {
                        Ok("{}".to_string())
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(EmptyHwndMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "title": "Chrome"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("could not resolve"));
    }

    #[tokio::test]
    async fn test_desktop_tool_get_window_text_mcp_error() {
        struct FailingMCPCaller;
        impl crate::browser::MCPToolCaller for FailingMCPCaller {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("get_text failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }
        let tool = DesktopTool::new(PathBuf::from("."), Some(Arc::new(FailingMCPCaller)));
        let result = tool.execute(&serde_json::json!({"action": "get_window_text", "hwnd": "0x123"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("get_window_text failed"));
    }

    #[test]
    fn test_desktop_action_from_str_edge_cases() {
        assert!(DesktopAction::from_str("").is_err());
        assert!(DesktopAction::from_str("FIND_WINDOW").is_err());
        assert!(DesktopAction::from_str("find_window ").is_err());
    }

    #[test]
    fn test_window_info_default_fields() {
        let info = WindowInfo {
            hwnd: String::new(),
            title: String::new(),
            class_name: String::new(),
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        };
        assert!(info.hwnd.is_empty());
        assert_eq!(info.width, 0);
    }
}
