//! Screen capture tool for taking screenshots.
//!
//! Supports 3 capture modes:
//! - full_screen: Capture the entire primary display
//! - region: Capture a rectangular area specified by x, y, width, height
//! - window: Capture a specific window identified by title or handle
//!
//! Uses MCP backend when available, falls back to PowerShell on Windows.
//! Screenshots are saved as PNG/JPG files to workspace/temp/.
//!
//! Port of Go's `module/tools/screen_capture.go`.

use crate::browser::MCPToolCaller;
use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Default per-capture timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Capture mode types.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureMode {
    FullScreen,
    Region,
    Window,
}

impl std::fmt::Display for CaptureMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureMode::FullScreen => write!(f, "full_screen"),
            CaptureMode::Region => write!(f, "region"),
            CaptureMode::Window => write!(f, "window"),
        }
    }
}

impl std::str::FromStr for CaptureMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full_screen" => Ok(CaptureMode::FullScreen),
            "region" => Ok(CaptureMode::Region),
            "window" => Ok(CaptureMode::Window),
            _ => Err(format!(
                "unknown capture mode: {} (supported: full_screen, region, window)",
                s
            )),
        }
    }
}

/// Screen capture tool - captures screenshots of the desktop.
///
/// On Windows, uses PowerShell with System.Drawing for native capture.
/// When a window-mcp server is available, it delegates to that for more
/// accurate window-level captures (including off-screen content via PrintWindow).
///
/// Captured images are saved as PNG/JPG files under `{workspace}/temp/`.
pub struct ScreenCaptureTool {
    mcp_caller: Option<Arc<dyn MCPToolCaller>>,
    workspace: PathBuf,
    timeout: Arc<Mutex<Duration>>,
}

impl ScreenCaptureTool {
    /// Create a new ScreenCaptureTool.
    ///
    /// Screenshots are saved to `{workspace}/temp/`. The `mcp_caller` may be
    /// `None`; in that case the native PowerShell-based capture is used.
    pub fn new(workspace: PathBuf, mcp_caller: Option<Arc<dyn MCPToolCaller>>) -> Self {
        Self {
            mcp_caller,
            workspace,
            timeout: Arc::new(Mutex::new(DEFAULT_TIMEOUT)),
        }
    }

    /// Set the per-capture timeout (default 30s).
    pub async fn set_timeout(&self, d: Duration) {
        let mut timeout = self.timeout.lock().await;
        *timeout = d;
    }

    /// Determine the .NET ImageFormat enum name for the given format string.
    fn image_format_enum(format: &str) -> &'static str {
        match format.to_lowercase().as_str() {
            "jpg" | "jpeg" => "Jpeg",
            "png" => "Png",
            "bmp" => "Bmp",
            _ => "Png",
        }
    }

    /// Ensure temp directory exists and return the output path for a screenshot.
    fn prepare_output_path(&self, format: &str) -> Result<PathBuf, ToolResult> {
        let temp_dir = self.workspace.join("temp");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            return Err(ToolResult::error(&format!(
                "failed to create temp directory: {}",
                e
            )));
        }

        let timestamp = chrono::Local::now().timestamp_millis();
        let ext = format!(".{}", format);
        let filename = format!("screenshot_{}{}", timestamp, ext);
        Ok(temp_dir.join(filename))
    }

    /// Execute a PowerShell capture script and return the result.
    async fn execute_capture(&self, script: &str, output_path: &std::path::Path) -> ToolResult {
        let timeout = *self.timeout.lock().await;

        tracing::debug!(
            output_path = %output_path.display(),
            script_length = script.len(),
            timeout_ms = timeout.as_millis() as u64,
            "[Tools] Running capture script"
        );

        let output = match tokio::time::timeout(
            timeout,
            tokio::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .output(),
        )
        .await
        {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return ToolResult::error(&format!("screen capture failed: {}", e));
            }
            Err(_) => {
                return ToolResult::error("screen capture timed out");
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut detail = format!("screen capture failed with status {}", output.status);
            if !stderr.is_empty() {
                detail.push_str(&format!("\n{}", stderr));
            }
            return ToolResult::error(&detail);
        }

        // Verify the file was created
        let file_info = match std::fs::metadata(output_path) {
            Ok(m) => m,
            Err(e) => {
                return ToolResult::error(&format!(
                    "screenshot file not found after capture: {}",
                    e
                ));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let result = format!(
            "Screenshot saved to {} (size: {} bytes, info: {})",
            output_path.display(),
            file_info.len(),
            stdout
        );

        tracing::info!(
            path = %output_path.display(),
            size_bytes = file_info.len(),
            "[Tools] Screenshot captured successfully"
        );

        ToolResult::success(&result)
    }

    // --------------- capture implementations ---------------

    /// Capture the full screen.
    async fn capture_full_screen(&self, output_path: &std::path::Path, format: &str) -> ToolResult {
        // Use window-mcp if available
        if let Some(ref caller) = self.mcp_caller {
            if caller.is_connected() {
                let mcp_args = serde_json::json!({
                    "file_path": output_path.to_string_lossy().to_string()
                });
                match caller
                    .call_tool("capture_screenshot_to_file", &mcp_args)
                    .await
                {
                    Ok(result) => {
                        return ToolResult::success(&format!(
                            "Screenshot saved to {}\n{}",
                            output_path.display(),
                            result
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "[Tools] MCP capture failed, falling back to PowerShell"
                        );
                        // Fall through to PowerShell
                    }
                }
            }
        }

        let script = self.build_full_screen_script(output_path, format);
        self.execute_capture(&script, output_path).await
    }

    /// Capture a screen region.
    async fn capture_region(
        &self,
        args: &serde_json::Value,
        output_path: &std::path::Path,
        format: &str,
    ) -> ToolResult {
        let x = match args["x"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                );
            }
        };
        let y = match args["y"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                );
            }
        };
        let w = match args["width"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                );
            }
        };
        let h = match args["height"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                );
            }
        };

        // Use window-mcp if available
        if let Some(ref caller) = self.mcp_caller {
            if caller.is_connected() {
                let mcp_args = serde_json::json!({
                    "file_path": output_path.to_string_lossy().to_string(),
                    "x": x,
                    "y": y,
                    "width": w,
                    "height": h
                });
                match caller
                    .call_tool("capture_screenshot_to_file", &mcp_args)
                    .await
                {
                    Ok(result) => {
                        return ToolResult::success(&format!(
                            "Region screenshot saved to {} ({}x{} at {},{})\n{}",
                            output_path.display(),
                            w,
                            h,
                            x,
                            y,
                            result
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "[Tools] MCP region capture failed, falling back to PowerShell"
                        );
                    }
                }
            }
        }

        let script = self.build_region_script(x, y, w, h, output_path, format);
        self.execute_capture(&script, output_path).await
    }

    /// Capture a specific window.
    async fn capture_window(
        &self,
        args: &serde_json::Value,
        output_path: &std::path::Path,
        format: &str,
    ) -> ToolResult {
        let hwnd = args["hwnd"].as_str().unwrap_or("");
        let window_title = args["window_title"].as_str().unwrap_or("");

        if hwnd.is_empty() && window_title.is_empty() {
            return ToolResult::error(
                "parameter 'hwnd' or 'window_title' is required for window mode",
            );
        }

        // Use window-mcp for window-level capture (supports background windows
        // via PrintWindow, which PowerShell's CopyFromScreen cannot do).
        if let Some(ref caller) = self.mcp_caller {
            if caller.is_connected() {
                let mut mcp_args = serde_json::json!({
                    "file_path": output_path.to_string_lossy().to_string()
                });

                let resolved_hwnd;
                if !hwnd.is_empty() {
                    mcp_args["hwnd"] = serde_json::Value::String(hwnd.to_string());
                } else if !window_title.is_empty() {
                    // Find the window first by title
                    let find_args = serde_json::json!({
                        "title_contains": window_title
                    });
                    match caller.call_tool("find_window_by_title", &find_args).await {
                        Ok(find_result) => {
                            resolved_hwnd = if let Ok(parsed) =
                                serde_json::from_str::<serde_json::Value>(&find_result)
                            {
                                parsed["hwnd"].as_str().unwrap_or("").to_string()
                            } else {
                                String::new()
                            };
                            if !resolved_hwnd.is_empty() {
                                mcp_args["hwnd"] = serde_json::Value::String(resolved_hwnd.clone());
                            }
                        }
                        Err(e) => {
                            return ToolResult::error(&format!(
                                "failed to find window '{}': {}",
                                window_title, e
                            ));
                        }
                    }
                }

                match caller
                    .call_tool("capture_screenshot_to_file", &mcp_args)
                    .await
                {
                    Ok(result) => {
                        return ToolResult::success(&format!(
                            "Window screenshot saved to {}\n{}",
                            output_path.display(),
                            result
                        ));
                    }
                    Err(e) => return ToolResult::error(&format!("window capture failed: {}", e)),
                }
            }
        }

        // PowerShell fallback
        self.capture_window_fallback(hwnd, window_title, output_path, format)
            .await
    }

    // --------------- PowerShell script builders ---------------

    /// Build the full-screen capture PowerShell script.
    fn build_full_screen_script(&self, output_path: &std::path::Path, format: &str) -> String {
        let escaped_path = output_path
            .to_string_lossy()
            .to_string()
            .replace('\'', "''");
        let fmt_enum = Self::image_format_enum(format);
        format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bitmap = New-Object System.Drawing.Bitmap($screen.Width, $screen.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$bitmap.Save('{}', [System.Drawing.Imaging.ImageFormat]::{})
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "OK"
"#,
            escaped_path, fmt_enum
        )
    }

    /// Build the region capture PowerShell script.
    fn build_region_script(
        &self,
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        output_path: &std::path::Path,
        format: &str,
    ) -> String {
        let escaped_path = output_path
            .to_string_lossy()
            .to_string()
            .replace('\'', "''");
        let fmt_enum = Self::image_format_enum(format);
        format!(
            r#"
Add-Type -AssemblyName System.Drawing
$bounds = New-Object System.Drawing.Rectangle({}, {}, {}, {})
$bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('{}', [System.Drawing.Imaging.ImageFormat]::{})
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "OK"
"#,
            x, y, w, h, escaped_path, fmt_enum
        )
    }

    /// Capture a window using PowerShell fallback (CopyFromScreen).
    ///
    /// The window must be visible and unobstructed for this to work.
    async fn capture_window_fallback(
        &self,
        hwnd: &str,
        window_title: &str,
        output_path: &std::path::Path,
        format: &str,
    ) -> ToolResult {
        let escaped_path = output_path
            .to_string_lossy()
            .to_string()
            .replace('\'', "''");
        let fmt_enum = Self::image_format_enum(format);

        let find_part = if !hwnd.is_empty() {
            // Parse hwnd from "HWND(0x...)" format or plain hex
            let hwnd_clean = hwnd.trim_start_matches("HWND(").trim_end_matches(')');
            format!("$handle = [IntPtr]0x{}", hwnd_clean)
        } else {
            let escaped_title = window_title.replace('\'', "''");
            format!(
                r#"$proc = Get-Process | Where-Object {{ $_.MainWindowTitle -like '*{}*' -and $_.MainWindowHandle -ne 0 }} | Select-Object -First 1
if (-not $proc) {{ Write-Error "Window not found: {}"; exit 1 }}
$handle = $proc.MainWindowHandle"#,
                escaped_title, escaped_title
            )
        };

        let script = format!(
            r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {{
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT {{ public int Left, Top, Right, Bottom; }}
}}
"@
{}
$rect = New-Object WinAPI+RECT
[WinAPI]::GetWindowRect($handle, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) {{ Write-Error "Invalid window dimensions"; exit 1 }}
$bounds = New-Object System.Drawing.Rectangle($rect.Left, $rect.Top, $w, $h)
$bitmap = New-Object System.Drawing.Bitmap($w, $h)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('{}', [System.Drawing.Imaging.ImageFormat]::{})
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "$w x $h"
"#,
            find_part, escaped_path, fmt_enum
        );

        self.execute_capture(&script, output_path).await
    }
}

#[async_trait]
impl Tool for ScreenCaptureTool {
    fn name(&self) -> &str {
        "screen_capture"
    }

    fn description(&self) -> &str {
        "Capture a screenshot of the screen, a region, or a specific window.\n\
         \n\
         Supported modes:\n\
         - full_screen: Capture the entire primary display\n\
         - region:      Capture a rectangular area specified by x, y, width, height\n\
         - window:      Capture a specific window identified by title or handle\n\
         \n\
         Screenshots are saved as PNG files under the workspace temp/ directory.\n\
         Returns the file path on success.\n\
         \n\
         On Windows, uses System.Drawing for native capture. When a window-mcp server\n\
         is connected, it is used for window-level captures (supports background windows)."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "description": "Capture mode",
                    "enum": ["full_screen", "region", "window"]
                },
                "x": {
                    "type": "integer",
                    "description": "X coordinate of the region (region mode)"
                },
                "y": {
                    "type": "integer",
                    "description": "Y coordinate of the region (region mode)"
                },
                "width": {
                    "type": "integer",
                    "description": "Width of the region (region mode)"
                },
                "height": {
                    "type": "integer",
                    "description": "Height of the region (region mode)"
                },
                "window_title": {
                    "type": "string",
                    "description": "Window title to capture (window mode, partial match)"
                },
                "hwnd": {
                    "type": "string",
                    "description": "Window handle for capture (window mode, e.g. 'HWND(0x12345)')"
                },
                "format": {
                    "type": "string",
                    "description": "Output image format: png (default) or jpg",
                    "enum": ["png", "jpg"]
                }
            },
            "required": ["mode"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let mode_str = match args["mode"].as_str() {
            Some(m) => m,
            None => return ToolResult::error("parameter 'mode' is required"),
        };

        let mode = match CaptureMode::from_str(mode_str) {
            Ok(m) => m,
            Err(ref e) => return ToolResult::error(e.as_str()),
        };

        // Determine output format
        let format = args["format"].as_str().unwrap_or("png").to_string();

        // Prepare output path
        let output_path = match self.prepare_output_path(&format) {
            Ok(p) => p,
            Err(r) => return r,
        };

        match mode {
            CaptureMode::FullScreen => self.capture_full_screen(&output_path, &format).await,
            CaptureMode::Region => self.capture_region(args, &output_path, &format).await,
            CaptureMode::Window => self.capture_window(args, &output_path, &format).await,
        }
    }
}

#[cfg(test)]
mod tests;
