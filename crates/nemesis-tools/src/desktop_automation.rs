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
            "[Tools] Running PowerShell script"
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
mod tests;
