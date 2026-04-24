// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// CaptureMode defines the type of screen capture to perform.
type CaptureMode string

const (
	CaptureFullScreen CaptureMode = "full_screen"
	CaptureRegion     CaptureMode = "region"
	CaptureWindow     CaptureMode = "window"
)

// ScreenCaptureTool captures screenshots of the desktop. On Windows it uses
// PowerShell with System.Drawing for native capture. When a window-mcp server
// is available, it delegates to that for more accurate window-level captures
// (including off-screen content via PrintWindow).
//
// Captured images are saved as PNG files under {workspace}/temp/ and the file
// path is returned in the result.
type ScreenCaptureTool struct {
	mcpCaller MCPToolCaller
	workspace string
	timeout   time.Duration
}

// NewScreenCaptureTool creates a ScreenCaptureTool. Screenshots are saved to
// {workspace}/temp/. The mcpCaller may be nil; in that case the native
// PowerShell-based capture is used.
func NewScreenCaptureTool(workspace string, mcpCaller MCPToolCaller) *ScreenCaptureTool {
	return &ScreenCaptureTool{
		mcpCaller: mcpCaller,
		workspace: workspace,
		timeout:   30 * time.Second,
	}
}

// SetTimeout changes the per-capture timeout (default 30s).
func (t *ScreenCaptureTool) SetTimeout(d time.Duration) {
	t.timeout = d
}

// Name implements Tool.
func (t *ScreenCaptureTool) Name() string {
	return "screen_capture"
}

// Description implements Tool.
func (t *ScreenCaptureTool) Description() string {
	return `Capture a screenshot of the screen, a region, or a specific window.

Supported modes:
- full_screen: Capture the entire primary display
- region:      Capture a rectangular area specified by x, y, width, height
- window:      Capture a specific window identified by title or handle

Screenshots are saved as PNG files under the workspace temp/ directory.
Returns the file path on success.

On Windows, uses System.Drawing for native capture. When a window-mcp server
is connected, it is used for window-level captures (supports background windows).`
}

// Parameters implements Tool.
func (t *ScreenCaptureTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"mode": map[string]interface{}{
				"type":        "string",
				"description": "Capture mode",
				"enum":        []string{string(CaptureFullScreen), string(CaptureRegion), string(CaptureWindow)},
			},
			"x": map[string]interface{}{
				"type":        "integer",
				"description": "X coordinate of the region (region mode)",
			},
			"y": map[string]interface{}{
				"type":        "integer",
				"description": "Y coordinate of the region (region mode)",
			},
			"width": map[string]interface{}{
				"type":        "integer",
				"description": "Width of the region (region mode)",
			},
			"height": map[string]interface{}{
				"type":        "integer",
				"description": "Height of the region (region mode)",
			},
			"window_title": map[string]interface{}{
				"type":        "string",
				"description": "Window title to capture (window mode, partial match)",
			},
			"hwnd": map[string]interface{}{
				"type":        "string",
				"description": "Window handle for capture (window mode, e.g. 'HWND(0x12345)')",
			},
			"format": map[string]interface{}{
				"type":        "string",
				"description": "Output image format: png (default) or jpg",
				"enum":        []string{"png", "jpg"},
			},
		},
		"required": []string{"mode"},
	}
}

// Execute implements Tool.
func (t *ScreenCaptureTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	modeRaw, ok := args["mode"].(string)
	if !ok {
		return ErrorResult("parameter 'mode' is required")
	}

	mode := CaptureMode(modeRaw)

	// Ensure temp directory exists.
	tempDir := filepath.Join(t.workspace, "temp")
	if err := os.MkdirAll(tempDir, 0755); err != nil {
		return ErrorResult(fmt.Sprintf("failed to create temp directory: %v", err))
	}

	// Determine output format and filename.
	format, _ := args["format"].(string)
	if format == "" {
		format = "png"
	}
	ext := "." + format
	filename := fmt.Sprintf("screenshot_%d%s", time.Now().UnixMilli(), ext)
	outputPath := filepath.Join(tempDir, filename)

	switch mode {
	case CaptureFullScreen:
		return t.captureFullScreen(ctx, outputPath, format)
	case CaptureRegion:
		return t.captureRegion(ctx, args, outputPath, format)
	case CaptureWindow:
		return t.captureWindow(ctx, args, outputPath, format)
	default:
		return ErrorResult(fmt.Sprintf("unknown capture mode: %s (supported: full_screen, region, window)", mode))
	}
}

// --------------- capture implementations ---------------

func (t *ScreenCaptureTool) captureFullScreen(ctx context.Context, outputPath, format string) *ToolResult {
	// Use window-mcp if available for full-screen capture.
	if t.mcpCaller != nil && t.mcpCaller.IsConnected() {
		mcpArgs := map[string]interface{}{
			"file_path": outputPath,
		}
		result, err := t.mcpCaller.CallTool(ctx, "capture_screenshot_to_file", mcpArgs)
		if err == nil {
			return NewToolResult(fmt.Sprintf("Screenshot saved to %s\n%s", outputPath, result))
		}
		// Fall through to PowerShell on MCP error.
		logger.WarnCF("screen_capture", "MCP capture failed, falling back to PowerShell",
			map[string]interface{}{"error": err.Error()})
	}

	script := t.buildFullScreenScript(outputPath, format)
	return t.executeCapture(ctx, script, outputPath)
}

func (t *ScreenCaptureTool) captureRegion(ctx context.Context, args map[string]interface{}, outputPath, format string) *ToolResult {
	x, hasX := getIntArg(args, "x")
	y, hasY := getIntArg(args, "y")
	w, hasW := getIntArg(args, "width")
	h, hasH := getIntArg(args, "height")

	if !hasX || !hasY || !hasW || !hasH {
		return ErrorResult("parameters 'x', 'y', 'width', and 'height' are required for region mode")
	}

	// Use window-mcp if available.
	if t.mcpCaller != nil && t.mcpCaller.IsConnected() {
		mcpArgs := map[string]interface{}{
			"file_path": outputPath,
			"x":         x,
			"y":         y,
			"width":     w,
			"height":    h,
		}
		result, err := t.mcpCaller.CallTool(ctx, "capture_screenshot_to_file", mcpArgs)
		if err == nil {
			return NewToolResult(fmt.Sprintf("Region screenshot saved to %s (%dx%d at %d,%d)\n%s",
				outputPath, w, h, x, y, result))
		}
		logger.WarnCF("screen_capture", "MCP region capture failed, falling back to PowerShell",
			map[string]interface{}{"error": err.Error()})
	}

	script := t.buildRegionScript(x, y, w, h, outputPath, format)
	return t.executeCapture(ctx, script, outputPath)
}

func (t *ScreenCaptureTool) captureWindow(ctx context.Context, args map[string]interface{}, outputPath, format string) *ToolResult {
	hwnd, _ := args["hwnd"].(string)
	windowTitle, _ := args["window_title"].(string)

	if hwnd == "" && windowTitle == "" {
		return ErrorResult("parameter 'hwnd' or 'window_title' is required for window mode")
	}

	// Use window-mcp for window-level capture (supports background windows
	// via PrintWindow, which PowerShell's CopyFromScreen cannot do).
	if t.mcpCaller != nil && t.mcpCaller.IsConnected() {
		mcpArgs := map[string]interface{}{
			"file_path": outputPath,
		}
		if hwnd != "" {
			mcpArgs["hwnd"] = hwnd
		}

		// If we only have a title, find the window first.
		if hwnd == "" && windowTitle != "" {
			findResult, err := t.mcpCaller.CallTool(ctx, "find_window_by_title", map[string]interface{}{
				"title_contains": windowTitle,
			})
			if err != nil {
				return ErrorResult(fmt.Sprintf("failed to find window '%s': %v", windowTitle, err))
			}
			// Parse hwnd from find result.
			var winInfo struct {
				Hwnd string `json:"hwnd"`
			}
			if err := json.Unmarshal([]byte(findResult), &winInfo); err == nil && winInfo.Hwnd != "" {
				mcpArgs["hwnd"] = winInfo.Hwnd
			}
		}

		result, err := t.mcpCaller.CallTool(ctx, "capture_screenshot_to_file", mcpArgs)
		if err != nil {
			return ErrorResult(fmt.Sprintf("window capture failed: %v", err))
		}
		return NewToolResult(fmt.Sprintf("Window screenshot saved to %s\n%s", outputPath, result))
	}

	// PowerShell fallback: can only capture visible (foreground) windows.
	// This uses CopyFromScreen so the window must be visible on screen.
	return t.captureWindowFallback(ctx, hwnd, windowTitle, outputPath, format)
}

// --------------- PowerShell script builders ---------------

func (t *ScreenCaptureTool) buildFullScreenScript(outputPath, format string) string {
	escapedPath := strings.ReplaceAll(outputPath, "'", "''")
	return fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bitmap = New-Object System.Drawing.Bitmap($screen.Width, $screen.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$bitmap.Save('%s', [System.Drawing.Imaging.ImageFormat]::%s)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "OK"
`, escapedPath, imageFormatEnum(format))
}

func (t *ScreenCaptureTool) buildRegionScript(x, y, w, h int, outputPath, format string) string {
	escapedPath := strings.ReplaceAll(outputPath, "'", "''")
	return fmt.Sprintf(`
Add-Type -AssemblyName System.Drawing
$bounds = New-Object System.Drawing.Rectangle(%d, %d, %d, %d)
$bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('%s', [System.Drawing.Imaging.ImageFormat]::%s)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "OK"
`, x, y, w, h, escapedPath, imageFormatEnum(format))
}

// captureWindowFallback uses PowerShell to find and capture a window.
// It relies on CopyFromScreen so the window must be visible and unobstructed.
func (t *ScreenCaptureTool) captureWindowFallback(ctx context.Context, hwnd, windowTitle, outputPath, format string) *ToolResult {
	escapedPath := strings.ReplaceAll(outputPath, "'", "''")

	// Build a script that finds the window by handle or title, then captures
	// its visible area from the screen.
	var findPart string
	if hwnd != "" {
		// Parse hwnd from "HWND(0x...)" format or plain hex.
		hwndClean := strings.TrimPrefix(hwnd, "HWND(")
		hwndClean = strings.TrimSuffix(hwndClean, ")")
		findPart = fmt.Sprintf(`$handle = [IntPtr]0x%s`, hwndClean)
	} else {
		escapedTitle := strings.ReplaceAll(windowTitle, "'", "''")
		findPart = fmt.Sprintf(`$proc = Get-Process | Where-Object { $_.MainWindowTitle -like '*%s*' -and $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Write-Error "Window not found: %s"; exit 1 }
$handle = $proc.MainWindowHandle`, escapedTitle, escapedTitle)
	}

	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinAPI {
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
%s
$rect = New-Object WinAPI+RECT
[WinAPI]::GetWindowRect($handle, [ref]$rect) | Out-Null
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) { Write-Error "Invalid window dimensions"; exit 1 }
$bounds = New-Object System.Drawing.Rectangle($rect.Left, $rect.Top, $w, $h)
$bitmap = New-Object System.Drawing.Bitmap($w, $h)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('%s', [System.Drawing.Imaging.ImageFormat]::%s)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "$w x $h"
`, findPart, escapedPath, imageFormatEnum(format))

	return t.executeCapture(ctx, script, outputPath)
}

// --------------- shared utilities ---------------

// executeCapture runs a PowerShell capture script and returns the result.
func (t *ScreenCaptureTool) executeCapture(ctx context.Context, script, outputPath string) *ToolResult {
	execCtx, cancel := context.WithTimeout(ctx, t.timeout)
	defer cancel()

	cmd := exec.CommandContext(execCtx, "powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script)
	var stdout, stderr strings.Builder
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	logger.DebugCF("screen_capture", "Running capture script", map[string]interface{}{
		"output_path":    outputPath,
		"script_length":  len(script),
		"timeout":        t.timeout,
	})

	err := cmd.Run()
	if err != nil {
		errDetail := err.Error()
		if stderr.Len() > 0 {
			errDetail += "\n" + stderr.String()
		}
		return ErrorResult(fmt.Sprintf("screen capture failed: %s", errDetail))
	}

	// Verify the file was created.
	info, err := os.Stat(outputPath)
	if err != nil {
		return ErrorResult(fmt.Sprintf("screenshot file not found after capture: %v", err))
	}

	output := strings.TrimSpace(stdout.String())
	result := fmt.Sprintf("Screenshot saved to %s (size: %d bytes, info: %s)", outputPath, info.Size(), output)

	logger.InfoCF("screen_capture", "Screenshot captured successfully", map[string]interface{}{
		"path":       outputPath,
		"size_bytes": info.Size(),
	})

	return NewToolResult(result)
}

// imageFormatEnum returns the .NET ImageFormat enum name for the given format string.
func imageFormatEnum(format string) string {
	switch strings.ToLower(format) {
	case "jpg", "jpeg":
		return "Jpeg"
	case "png":
		return "Png"
	case "bmp":
		return "Bmp"
	default:
		return "Png"
	}
}
