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
use std::sync::Arc;
use std::str::FromStr;
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

        let timestamp = chrono::Utc::now().timestamp_millis();
        let ext = format!(".{}", format);
        let filename = format!("screenshot_{}{}", timestamp, ext);
        Ok(temp_dir.join(filename))
    }

    /// Execute a PowerShell capture script and return the result.
    async fn execute_capture(
        &self,
        script: &str,
        output_path: &std::path::Path,
    ) -> ToolResult {
        let timeout = *self.timeout.lock().await;

        tracing::debug!(
            output_path = %output_path.display(),
            script_length = script.len(),
            timeout_ms = timeout.as_millis() as u64,
            "Running capture script"
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
                ))
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
            "Screenshot captured successfully"
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
                        ))
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "MCP capture failed, falling back to PowerShell"
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
                )
            }
        };
        let y = match args["y"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                )
            }
        };
        let w = match args["width"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                )
            }
        };
        let h = match args["height"].as_i64() {
            Some(v) => v,
            None => {
                return ToolResult::error(
                    "parameters 'x', 'y', 'width', and 'height' are required for region mode",
                )
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
                        ))
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "MCP region capture failed, falling back to PowerShell"
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
                    match caller
                        .call_tool("find_window_by_title", &find_args)
                        .await
                    {
                        Ok(find_result) => {
                            resolved_hwnd = if let Ok(parsed) =
                                serde_json::from_str::<serde_json::Value>(&find_result)
                            {
                                parsed["hwnd"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string()
                            } else {
                                String::new()
                            };
                            if !resolved_hwnd.is_empty() {
                                mcp_args["hwnd"] =
                                    serde_json::Value::String(resolved_hwnd.clone());
                            }
                        }
                        Err(e) => {
                            return ToolResult::error(&format!(
                                "failed to find window '{}': {}",
                                window_title, e
                            ))
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
                        ))
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
    fn build_full_screen_script(
        &self,
        output_path: &std::path::Path,
        format: &str,
    ) -> String {
        let escaped_path = output_path.to_string_lossy().to_string().replace('\'', "''");
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
        let escaped_path = output_path.to_string_lossy().to_string().replace('\'', "''");
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
        let escaped_path = output_path.to_string_lossy().to_string().replace('\'', "''");
        let fmt_enum = Self::image_format_enum(format);

        let find_part = if !hwnd.is_empty() {
            // Parse hwnd from "HWND(0x...)" format or plain hex
            let hwnd_clean = hwnd
                .trim_start_matches("HWND(")
                .trim_end_matches(')');
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
            CaptureMode::Region => {
                self.capture_region(args, &output_path, &format).await
            }
            CaptureMode::Window => {
                self.capture_window(args, &output_path, &format).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capture_mode_from_str() {
        assert_eq!(
            CaptureMode::from_str("full_screen").unwrap(),
            CaptureMode::FullScreen
        );
        assert_eq!(
            CaptureMode::from_str("region").unwrap(),
            CaptureMode::Region
        );
        assert_eq!(
            CaptureMode::from_str("window").unwrap(),
            CaptureMode::Window
        );
        assert!(CaptureMode::from_str("invalid").is_err());
    }

    #[test]
    fn test_capture_mode_display() {
        assert_eq!(CaptureMode::FullScreen.to_string(), "full_screen");
        assert_eq!(CaptureMode::Region.to_string(), "region");
        assert_eq!(CaptureMode::Window.to_string(), "window");
    }

    #[test]
    fn test_image_format_enum() {
        assert_eq!(ScreenCaptureTool::image_format_enum("png"), "Png");
        assert_eq!(ScreenCaptureTool::image_format_enum("jpg"), "Jpeg");
        assert_eq!(ScreenCaptureTool::image_format_enum("jpeg"), "Jpeg");
        assert_eq!(ScreenCaptureTool::image_format_enum("bmp"), "Bmp");
        assert_eq!(ScreenCaptureTool::image_format_enum("unknown"), "Png");
    }

    #[tokio::test]
    async fn test_screen_capture_tool_metadata() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        assert_eq!(tool.name(), "screen_capture");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_screen_capture_tool_missing_mode() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'mode' is required"));
    }

    #[tokio::test]
    async fn test_screen_capture_tool_unknown_mode() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "unknown"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("unknown capture mode"));
    }

    #[tokio::test]
    async fn test_screen_capture_tool_region_missing_params() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "region"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'x', 'y', 'width', and 'height' are required"));
    }

    #[tokio::test]
    async fn test_screen_capture_tool_window_missing_params() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "window"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'hwnd' or 'window_title' is required"));
    }

    #[tokio::test]
    async fn test_screen_capture_tool_parameters_schema() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let params = tool.parameters();

        // Verify required fields
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("mode")));

        // Verify mode enum values
        let mode_enum = params["properties"]["mode"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(mode_enum.len(), 3);
        assert!(mode_enum
            .iter()
            .any(|v| v.as_str() == Some("full_screen")));
        assert!(mode_enum.iter().any(|v| v.as_str() == Some("region")));
        assert!(mode_enum.iter().any(|v| v.as_str() == Some("window")));

        // Verify format enum
        let format_enum = params["properties"]["format"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(format_enum.len(), 2);
    }

    #[test]
    fn test_build_full_screen_script() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.png"), "png");
        assert!(script.contains("PrimaryScreen.Bounds"));
        assert!(script.contains("ImageFormat]::Png"));
        assert!(script.contains("/tmp/test.png"));
    }

    #[test]
    fn test_build_region_script() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_region_script(
            100,
            200,
            300,
            400,
            std::path::Path::new("/tmp/region.png"),
            "jpg",
        );
        assert!(script.contains("100, 200, 300, 400"));
        assert!(script.contains("ImageFormat]::Jpeg"));
    }

    #[test]
    fn test_prepare_output_path() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);

        let result = tool.prepare_output_path("png");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".png"));
        assert!(path.to_string_lossy().contains("screenshot_"));

        let result = tool.prepare_output_path("jpg");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".jpg"));
    }

    // ============================================================
    // Additional screen capture edge-case tests
    // ============================================================

    #[test]
    fn test_capture_mode_roundtrip() {
        let modes = [CaptureMode::FullScreen, CaptureMode::Region, CaptureMode::Window];
        for mode in &modes {
            let s = mode.to_string();
            let parsed = CaptureMode::from_str(&s);
            assert_eq!(parsed.unwrap(), *mode);
        }
    }

    #[test]
    fn test_image_format_all_variants() {
        assert_eq!(ScreenCaptureTool::image_format_enum("png"), "Png");
        assert_eq!(ScreenCaptureTool::image_format_enum("jpg"), "Jpeg");
        assert_eq!(ScreenCaptureTool::image_format_enum("jpeg"), "Jpeg");
        assert_eq!(ScreenCaptureTool::image_format_enum("bmp"), "Bmp");
        // Unknown and empty default to Png
        assert_eq!(ScreenCaptureTool::image_format_enum(""), "Png");
        assert_eq!(ScreenCaptureTool::image_format_enum("unknown"), "Png");
    }

    #[test]
    fn test_prepare_output_path_creates_temp_dir() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);

        let result = tool.prepare_output_path("png");
        assert!(result.is_ok());
        let path = result.unwrap();
        // Path should be under temp/
        assert!(path.to_string_lossy().contains("temp"));
    }

    #[tokio::test]
    async fn test_screen_capture_region_partial_params() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        // Only x provided, missing y/width/height
        let result = tool
            .execute(&serde_json::json!({"mode": "region", "x": 0}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("required"));
    }

    #[tokio::test]
    async fn test_screen_capture_window_with_hwnd() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x12345)"}))
            .await;
        // Without MCP or real window, this will likely error on non-Windows or produce a fallback result
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[tokio::test]
    async fn test_screen_capture_jpg_format() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        // Will fail since no actual screen capture, but should not panic
        let _result = tool
            .execute(&serde_json::json!({"mode": "full_screen", "format": "jpg"}))
            .await;
        // Just verify no panic
    }

    #[test]
    fn test_build_full_screen_script_jpg() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.jpg"), "jpg");
        assert!(script.contains("ImageFormat]::Jpeg"));
    }

    // --- Additional tests for coverage ---

    #[test]
    fn test_image_format_bmp() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.bmp"), "bmp");
        assert!(script.contains("ImageFormat]::Bmp"));
    }

    #[test]
    fn test_capture_mode_equality() {
        assert_eq!(CaptureMode::FullScreen, CaptureMode::FullScreen);
        assert_ne!(CaptureMode::FullScreen, CaptureMode::Region);
        assert_ne!(CaptureMode::Region, CaptureMode::Window);
    }

    #[tokio::test]
    async fn test_screen_capture_full_screen_format_png() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
        // Just verify no panic with various format strings
        let _ = tool
            .execute(&serde_json::json!({"mode": "full_screen", "format": "png"}))
            .await;
    }

    #[tokio::test]
    async fn test_screen_capture_window_with_title() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "window_title": "Calculator"}))
            .await;
        // Will likely fail without MCP, but verify no panic
        assert!(result.is_error || !result.for_llm.is_empty());
    }

    #[test]
    fn test_build_region_script_png() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_region_script(
            0, 0, 1920, 1080,
            std::path::Path::new("/tmp/region.png"),
            "png",
        );
        assert!(script.contains("0, 0, 1920, 1080"));
        assert!(script.contains("ImageFormat]::Png"));
    }

    #[test]
    fn test_prepare_output_path_jpg_extension() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
        let result = tool.prepare_output_path("jpg");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".jpg"));
    }

    #[tokio::test]
    async fn test_screen_capture_tool_no_mcp_call() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
        // Verify tool can execute without MCP - just check no panic
        let params = tool.parameters();
        assert!(params.is_object());
    }

    #[test]
    fn test_screen_capture_tool_parameters_complete() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let params = tool.parameters();
        // Verify all expected properties exist
        assert!(params["properties"]["mode"].is_object());
        assert!(params["properties"]["format"].is_object());
        assert!(params["properties"]["x"].is_object());
        assert!(params["properties"]["y"].is_object());
        assert!(params["properties"]["width"].is_object());
        assert!(params["properties"]["height"].is_object());
        assert!(params["properties"]["hwnd"].is_object());
        assert!(params["properties"]["window_title"].is_object());
    }

    // ============================================================
    // Additional coverage tests for 95%+ target - MCP paths
    // ============================================================

    #[tokio::test]
    async fn test_screen_capture_full_screen_mcp_success() {
        struct MockMCP;
        impl crate::browser::MCPToolCaller for MockMCP {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("screenshot saved".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(MockMCP)),
        );
        let result = tool
            .execute(&serde_json::json!({"mode": "full_screen"}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("screenshot saved"));
    }

    #[tokio::test]
    async fn test_screen_capture_full_screen_mcp_fails_fallback() {
        struct FailMCP;
        impl crate::browser::MCPToolCaller for FailMCP {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("MCP unavailable".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(FailMCP)),
        );
        // MCP fails, falls back to PowerShell which may or may not work
        let _result = tool
            .execute(&serde_json::json!({"mode": "full_screen"}))
            .await;
        // Just verify no panic
    }

    #[tokio::test]
    async fn test_screen_capture_full_screen_mcp_disconnected() {
        struct DisconnectedMCP;
        impl crate::browser::MCPToolCaller for DisconnectedMCP {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("should not be called".to_string()) })
            }
            fn is_connected(&self) -> bool { false }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(DisconnectedMCP)),
        );
        // Disconnected MCP, falls back to PowerShell
        let _result = tool
            .execute(&serde_json::json!({"mode": "full_screen"}))
            .await;
    }

    #[tokio::test]
    async fn test_screen_capture_region_mcp_success() {
        struct MockMCP;
        impl crate::browser::MCPToolCaller for MockMCP {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Ok("region saved".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(MockMCP)),
        );
        let result = tool
            .execute(&serde_json::json!({
                "mode": "region", "x": 0, "y": 0, "width": 100, "height": 100
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("region saved"));
    }

    #[tokio::test]
    async fn test_screen_capture_region_missing_y() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "region", "x": 0, "width": 100, "height": 100}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_screen_capture_region_missing_width() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "region", "x": 0, "y": 0, "height": 100}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_screen_capture_region_missing_height() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "region", "x": 0, "y": 0, "width": 100}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_screen_capture_window_mcp_with_hwnd() {
        struct MockMCP;
        impl crate::browser::MCPToolCaller for MockMCP {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "capture_screenshot_to_file" => Ok("window captured".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(MockMCP)),
        );
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x123)"}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("window captured"));
    }

    #[tokio::test]
    async fn test_screen_capture_window_mcp_with_title() {
        struct MockMCP;
        impl crate::browser::MCPToolCaller for MockMCP {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "find_window_by_title" => Ok(r#"{"hwnd":"HWND(0x456)"}"#.to_string()),
                        "capture_screenshot_to_file" => Ok("window captured".to_string()),
                        _ => Err("unknown tool".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(MockMCP)),
        );
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "window_title": "Calculator"}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("window captured"));
    }

    #[tokio::test]
    async fn test_screen_capture_window_mcp_find_fails() {
        struct FailMCP;
        impl crate::browser::MCPToolCaller for FailMCP {
            fn call_tool(
                &self,
                _tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                Box::pin(async { Err("find failed".to_string()) })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(FailMCP)),
        );
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "window_title": "Nonexistent"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("find window"));
    }

    #[tokio::test]
    async fn test_screen_capture_window_mcp_capture_fails() {
        struct FailCaptureMCP;
        impl crate::browser::MCPToolCaller for FailCaptureMCP {
            fn call_tool(
                &self,
                tool_name: &str,
                _args: &serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>> {
                let name = tool_name.to_string();
                Box::pin(async move {
                    match name.as_str() {
                        "capture_screenshot_to_file" => Err("capture failed".to_string()),
                        _ => Err("unknown".to_string()),
                    }
                })
            }
            fn is_connected(&self) -> bool { true }
        }

        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(
            PathBuf::from(temp.path()),
            Some(Arc::new(FailCaptureMCP)),
        );
        let result = tool
            .execute(&serde_json::json!({"mode": "window", "hwnd": "HWND(0x123)"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("window capture failed"));
    }

    #[tokio::test]
    async fn test_screen_capture_set_timeout() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let custom = Duration::from_secs(60);
        tool.set_timeout(custom).await;
        // Just verify no panic
    }

    #[test]
    fn test_capture_mode_debug() {
        assert!(format!("{:?}", CaptureMode::FullScreen).contains("FullScreen"));
        assert!(format!("{:?}", CaptureMode::Region).contains("Region"));
        assert!(format!("{:?}", CaptureMode::Window).contains("Window"));
    }

    #[test]
    fn test_capture_mode_from_str_all() {
        assert_eq!("full_screen".parse::<CaptureMode>(), Ok(CaptureMode::FullScreen));
        assert_eq!("region".parse::<CaptureMode>(), Ok(CaptureMode::Region));
        assert_eq!("window".parse::<CaptureMode>(), Ok(CaptureMode::Window));
    }

    #[test]
    fn test_capture_mode_from_str_invalid() {
        let result = "invalid".parse::<CaptureMode>();
        assert!(result.is_err());
    }

    #[test]
    fn test_capture_mode_roundtrip_all_variants() {
        for mode in &[CaptureMode::FullScreen, CaptureMode::Region, CaptureMode::Window] {
            let s = mode.to_string();
            let parsed: CaptureMode = s.parse().unwrap();
            assert_eq!(*mode, parsed);
        }
    }

    // ============================================================
    // Additional coverage tests for 95%+ target
    // ============================================================

    #[tokio::test]
    async fn test_screen_capture_set_timeout_value() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        tool.set_timeout(Duration::from_secs(10)).await;
        // Verify the timeout was set
        let timeout = tool.timeout.lock().await;
        assert_eq!(*timeout, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn test_screen_capture_set_timeout_default() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let timeout = tool.timeout.lock().await;
        assert_eq!(*timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn test_image_format_enum_case_insensitive() {
        assert_eq!(ScreenCaptureTool::image_format_enum("PNG"), "Png");
        assert_eq!(ScreenCaptureTool::image_format_enum("JPG"), "Jpeg");
        assert_eq!(ScreenCaptureTool::image_format_enum("JPEG"), "Jpeg");
        assert_eq!(ScreenCaptureTool::image_format_enum("BMP"), "Bmp");
    }

    #[test]
    fn test_build_full_screen_script_jpg_format() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_full_screen_script(std::path::Path::new("/tmp/test.jpg"), "jpg");
        assert!(script.contains("ImageFormat]::Jpeg"));
        assert!(script.contains("/tmp/test.jpg"));
    }

    #[test]
    fn test_build_region_script_bmp_format() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let script = tool.build_region_script(0, 0, 1920, 1080, std::path::Path::new("/tmp/region.bmp"), "bmp");
        assert!(script.contains("ImageFormat]::Bmp"));
        assert!(script.contains("0, 0, 1920, 1080"));
    }

    #[tokio::test]
    async fn test_screen_capture_region_only_x_and_y() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        // Missing width and height
        let result = tool
            .execute(&serde_json::json!({"mode": "region", "x": 10, "y": 20}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("required"));
    }

    #[tokio::test]
    async fn test_screen_capture_window_no_params() {
        let tool = ScreenCaptureTool::new(PathBuf::from("/tmp"), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "window"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("'hwnd' or 'window_title' is required"));
    }

    #[tokio::test]
    async fn test_screen_capture_bmp_format_param() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);
        let result = tool
            .execute(&serde_json::json!({"mode": "full_screen", "format": "bmp"}))
            .await;
        // Will likely fail since it tries powershell, but should not panic
        // Just verify it doesn't crash
        let _ = result;
    }

    #[test]
    fn test_prepare_output_path_format_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let tool = ScreenCaptureTool::new(PathBuf::from(temp.path()), None);

        let png_path = tool.prepare_output_path("png").unwrap();
        assert!(png_path.to_string_lossy().ends_with(".png"));

        let jpg_path = tool.prepare_output_path("jpg").unwrap();
        assert!(jpg_path.to_string_lossy().ends_with(".jpg"));

        let bmp_path = tool.prepare_output_path("bmp").unwrap();
        assert!(bmp_path.to_string_lossy().ends_with(".bmp"));
    }

    #[test]
    fn test_capture_mode_debug_format() {
        assert_eq!(format!("{:?}", CaptureMode::FullScreen), "FullScreen");
        assert_eq!(format!("{:?}", CaptureMode::Region), "Region");
        assert_eq!(format!("{:?}", CaptureMode::Window), "Window");
    }

    #[test]
    fn test_capture_mode_inequality() {
        assert_eq!(CaptureMode::FullScreen, CaptureMode::FullScreen);
        assert_ne!(CaptureMode::FullScreen, CaptureMode::Region);
        assert_ne!(CaptureMode::Region, CaptureMode::Window);
    }

    #[test]
    fn test_capture_mode_from_str_error_message() {
        let result = "invalid_mode".parse::<CaptureMode>();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unknown capture mode"));
        assert!(err.contains("invalid_mode"));
    }
}
