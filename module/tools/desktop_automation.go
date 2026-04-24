// NemesisBot - AI agent
// License: MIT
// Copyright (c) 2026 NemesisBot contributors
package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"runtime"
	"strings"
	"time"

	"github.com/276793422/NemesisBot/module/logger"
)

// DesktopAction defines the supported desktop automation actions.
type DesktopAction string

const (
	DesktopFindWindow  DesktopAction = "find_window"
	DesktopListWindows DesktopAction = "list_windows"
	DesktopClickAt     DesktopAction = "click_at"
	DesktopTypeText    DesktopAction = "type_text"
	DesktopScreenshot  DesktopAction = "take_screenshot"
	DesktopGetText     DesktopAction = "get_window_text"
)

// DesktopTool provides desktop UI automation for Windows. It supports two
// execution backends:
//
//  1. MCP backend: delegates to a window-mcp server (recommended). The tool
//     calls MCP tools through the MCPToolCaller interface, the same one used by
//     BrowserTool.
//
//  2. Standalone backend: uses PowerShell and platform utilities for basic
//     operations (list windows, take screenshots). This is the fallback when no
//     MCP server is available.
type DesktopTool struct {
	mcpCaller MCPToolCaller
	workspace string
	timeout   time.Duration
}

// NewDesktopTool creates a DesktopTool. The mcpCaller may be nil, in which
// case only the standalone (PowerShell-based) operations will be available.
func NewDesktopTool(workspace string, mcpCaller MCPToolCaller) *DesktopTool {
	return &DesktopTool{
		mcpCaller: mcpCaller,
		workspace: workspace,
		timeout:   30 * time.Second,
	}
}

// SetTimeout changes the per-operation timeout (default 30s).
func (t *DesktopTool) SetTimeout(d time.Duration) {
	t.timeout = d
}

// Name implements Tool.
func (t *DesktopTool) Name() string {
	return "desktop"
}

// Description implements Tool.
func (t *DesktopTool) Description() string {
	return `Automate desktop windows on the current machine (Windows only).

Supported actions:
- find_window:     Find a window by title (partial match) and return its handle and position
- list_windows:    List all visible windows with title, class, position, and size
- click_at:        Perform a mouse click at screen coordinates (x, y)
- type_text:       Send keyboard text input to the foreground window
- take_screenshot: Capture a screenshot of a window or screen region (saves to workspace/temp/)
- get_window_text: Retrieve the text content of a window

When a window-mcp server is configured, operations are delegated to it for full
native Windows API support. Otherwise a PowerShell-based fallback is used for
basic operations.`
}

// Parameters implements Tool.
func (t *DesktopTool) Parameters() map[string]interface{} {
	return map[string]interface{}{
		"type": "object",
		"properties": map[string]interface{}{
			"action": map[string]interface{}{
				"type":        "string",
				"description": "Desktop action to perform",
				"enum": []string{
					string(DesktopFindWindow),
					string(DesktopListWindows),
					string(DesktopClickAt),
					string(DesktopTypeText),
					string(DesktopScreenshot),
					string(DesktopGetText),
				},
			},
			"title": map[string]interface{}{
				"type":        "string",
				"description": "Window title (partial match, used by find_window, get_window_text)",
			},
			"hwnd": map[string]interface{}{
				"type":        "string",
				"description": "Window handle, e.g. 'HWND(0x12345)' (used by screenshot, get_window_text)",
			},
			"x": map[string]interface{}{
				"type":        "integer",
				"description": "Screen X coordinate for click_at or screenshot region",
			},
			"y": map[string]interface{}{
				"type":        "integer",
				"description": "Screen Y coordinate for click_at or screenshot region",
			},
			"width": map[string]interface{}{
				"type":        "integer",
				"description": "Width for screenshot region",
			},
			"height": map[string]interface{}{
				"type":        "integer",
				"description": "Height for screenshot region",
			},
			"text": map[string]interface{}{
				"type":        "string",
				"description": "Text to type into the foreground window (used by type_text)",
			},
			"button": map[string]interface{}{
				"type":        "string",
				"description": "Mouse button for click_at: left (default), right, middle",
				"enum":        []string{"left", "right", "middle"},
			},
		},
		"required": []string{"action"},
	}
}

// Execute implements Tool.
func (t *DesktopTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
	if runtime.GOOS != "windows" {
		return ErrorResult("desktop automation is only supported on Windows")
	}

	actionRaw, ok := args["action"].(string)
	if !ok {
		return ErrorResult("parameter 'action' is required")
	}

	action := DesktopAction(actionRaw)
	switch action {
	case DesktopFindWindow:
		return t.executeFindWindow(ctx, args)
	case DesktopListWindows:
		return t.executeListWindows(ctx, args)
	case DesktopClickAt:
		return t.executeClickAt(ctx, args)
	case DesktopTypeText:
		return t.executeTypeText(ctx, args)
	case DesktopScreenshot:
		return t.executeScreenshot(ctx, args)
	case DesktopGetText:
		return t.executeGetWindowText(ctx, args)
	default:
		return ErrorResult(fmt.Sprintf("unknown desktop action: %s", action))
	}
}

// --------------- helpers ---------------

// hasMCP returns true if the MCP caller is available and connected.
func (t *DesktopTool) hasMCP() bool {
	return t.mcpCaller != nil && t.mcpCaller.IsConnected()
}

// getIntArg extracts an integer argument, supporting both int and float64 (JSON).
func getIntArg(args map[string]interface{}, key string) (int, bool) {
	if v, ok := args[key].(int); ok {
		return v, true
	}
	if v, ok := args[key].(float64); ok {
		return int(v), true
	}
	return 0, false
}

// --------------- action implementations ---------------

func (t *DesktopTool) executeFindWindow(ctx context.Context, args map[string]interface{}) *ToolResult {
	title, _ := args["title"].(string)
	if title == "" {
		return ErrorResult("parameter 'title' is required for find_window action")
	}

	if t.hasMCP() {
		mcpArgs := map[string]interface{}{
			"title_contains": title,
		}
		result, err := t.mcpCaller.CallTool(ctx, "find_window_by_title", mcpArgs)
		if err != nil {
			return ErrorResult(fmt.Sprintf("MCP find_window failed: %v", err))
		}
		return NewToolResult(result)
	}

	// Standalone fallback: use PowerShell to list windows and filter by title.
	return t.standaloneFindWindow(ctx, title)
}

func (t *DesktopTool) executeListWindows(ctx context.Context, args map[string]interface{}) *ToolResult {
	if t.hasMCP() {
		mcpArgs := map[string]interface{}{
			"filter_visible": true,
		}
		if title, _ := args["title"].(string); title != "" {
			mcpArgs["title_contains"] = title
		}
		result, err := t.mcpCaller.CallTool(ctx, "enumerate_windows", mcpArgs)
		if err != nil {
			return ErrorResult(fmt.Sprintf("MCP list_windows failed: %v", err))
		}
		return NewToolResult(result)
	}

	return t.standaloneListWindows(ctx)
}

func (t *DesktopTool) executeClickAt(ctx context.Context, args map[string]interface{}) *ToolResult {
	x, okX := getIntArg(args, "x")
	y, okY := getIntArg(args, "y")
	if !okX || !okY {
		return ErrorResult("parameters 'x' and 'y' are required for click_at action")
	}

	button, _ := args["button"].(string)
	if button == "" {
		button = "left"
	}

	if t.hasMCP() {
		hwnd, _ := args["hwnd"].(string)
		mcpArgs := map[string]interface{}{
			"x":      x,
			"y":      y,
			"button": button,
		}
		if hwnd != "" {
			mcpArgs["hwnd"] = hwnd
		}
		result, err := t.mcpCaller.CallTool(ctx, "click_window", mcpArgs)
		if err != nil {
			return ErrorResult(fmt.Sprintf("MCP click_at failed: %v", err))
		}
		return SilentResult(fmt.Sprintf("Clicked at (%d, %d) with %s button.\n%s", x, y, button, result))
	}

	return t.standaloneClickAt(ctx, x, y, button)
}

func (t *DesktopTool) executeTypeText(ctx context.Context, args map[string]interface{}) *ToolResult {
	text, ok := args["text"].(string)
	if !ok || text == "" {
		return ErrorResult("parameter 'text' is required for type_text action")
	}

	if t.hasMCP() {
		hwnd, _ := args["hwnd"].(string)
		mcpArgs := map[string]interface{}{
			"key": text,
		}
		if hwnd != "" {
			mcpArgs["hwnd"] = hwnd
		}
		result, err := t.mcpCaller.CallTool(ctx, "send_key_to_window", mcpArgs)
		if err != nil {
			return ErrorResult(fmt.Sprintf("MCP type_text failed: %v", err))
		}
		return SilentResult(fmt.Sprintf("Typed text.\n%s", result))
	}

	return t.standaloneTypeText(ctx, text)
}

func (t *DesktopTool) executeScreenshot(ctx context.Context, args map[string]interface{}) *ToolResult {
	x, hasX := getIntArg(args, "x")
	y, hasY := getIntArg(args, "y")
	w, hasW := getIntArg(args, "width")
	h, hasH := getIntArg(args, "height")

	if t.hasMCP() {
		mcpArgs := map[string]interface{}{}
		if hasX {
			mcpArgs["x"] = x
		}
		if hasY {
			mcpArgs["y"] = y
		}
		if hasW {
			mcpArgs["width"] = w
		}
		if hasH {
			mcpArgs["height"] = h
		}
		hwnd, _ := args["hwnd"].(string)
		if hwnd != "" {
			mcpArgs["hwnd"] = hwnd
		}
		result, err := t.mcpCaller.CallTool(ctx, "capture_screenshot_to_file", mcpArgs)
		if err != nil {
			return ErrorResult(fmt.Sprintf("MCP screenshot failed: %v", err))
		}
		return SilentResult(fmt.Sprintf("Screenshot captured.\n%s", result))
	}

	return t.standaloneScreenshot(ctx, x, y, w, h, hasX, hasY, hasW, hasH)
}

func (t *DesktopTool) executeGetWindowText(ctx context.Context, args map[string]interface{}) *ToolResult {
	hwnd, _ := args["hwnd"].(string)
	title, _ := args["title"].(string)

	if hwnd == "" && title == "" {
		return ErrorResult("parameter 'hwnd' or 'title' is required for get_window_text action")
	}

	if t.hasMCP() {
		if hwnd == "" {
			// Find the window first by title.
			findResult, err := t.mcpCaller.CallTool(ctx, "find_window_by_title", map[string]interface{}{
				"title_contains": title,
			})
			if err != nil {
				return ErrorResult(fmt.Sprintf("MCP find_window for get_window_text failed: %v", err))
			}
			// Parse the hwnd from the result.
			var winInfo struct {
				Hwnd string `json:"hwnd"`
			}
			if err := json.Unmarshal([]byte(findResult), &winInfo); err == nil && winInfo.Hwnd != "" {
				hwnd = winInfo.Hwnd
			}
		}
		if hwnd == "" {
			return ErrorResult("could not resolve window handle for get_window_text")
		}
		result, err := t.mcpCaller.CallTool(ctx, "get_window_text", map[string]interface{}{
			"hwnd": hwnd,
		})
		if err != nil {
			return ErrorResult(fmt.Sprintf("MCP get_window_text failed: %v", err))
		}
		return NewToolResult(result)
	}

	return ErrorResult("get_window_text requires a window-mcp server (no standalone fallback available)")
}

// --------------- standalone (PowerShell) fallback implementations ---------------

// windowInfo is a simplified struct for JSON deserialization from PowerShell output.
type windowInfo struct {
	Hwnd      string `json:"hwnd"`
	Title     string `json:"title"`
	ClassName string `json:"class_name"`
	Left      int    `json:"left"`
	Top       int    `json:"top"`
	Width     int    `json:"width"`
	Height    int    `json:"height"`
}

// standaloneFindWindow uses PowerShell to enumerate windows and filter by title.
func (t *DesktopTool) standaloneFindWindow(ctx context.Context, title string) *ToolResult {
	windows, err := t.enumerateWindowsPS(ctx)
	if err != nil {
		return ErrorResult(fmt.Sprintf("failed to enumerate windows: %v", err))
	}

	var matches []windowInfo
	for _, w := range windows {
		if strings.Contains(strings.ToLower(w.Title), strings.ToLower(title)) {
			matches = append(matches, w)
		}
	}

	if len(matches) == 0 {
		return NewToolResult(fmt.Sprintf("No windows found matching title: %s", title))
	}

	data, _ := json.MarshalIndent(matches, "", "  ")
	return NewToolResult(fmt.Sprintf("Found %d window(s) matching '%s':\n%s", len(matches), title, string(data)))
}

// standaloneListWindows uses PowerShell to list visible windows.
func (t *DesktopTool) standaloneListWindows(ctx context.Context) *ToolResult {
	windows, err := t.enumerateWindowsPS(ctx)
	if err != nil {
		return ErrorResult(fmt.Sprintf("failed to enumerate windows: %v", err))
	}

	data, _ := json.MarshalIndent(windows, "", "  ")
	return NewToolResult(fmt.Sprintf("Found %d visible window(s):\n%s", len(windows), string(data)))
}

// standaloneClickAt uses PowerShell to simulate a mouse click.
func (t *DesktopTool) standaloneClickAt(ctx context.Context, x, y int, button string) *ToolResult {
	// Use PowerShell with System.Windows.Forms to move cursor and click.
	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
[System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point(%d, %d)
$mouseEvent = @{
    LeftDown = 0x0002
    LeftUp   = 0x0004
    RightDown = 0x0008
    RightUp   = 0x0010
    MiddleDown = 0x0020
    MiddleUp   = 0x0040
}
`, x, y)

	switch button {
	case "right":
		script += `
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Mouse {
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, IntPtr dwExtraInfo);
}
"@
[Mouse]::mouse_event(0x0008, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 50
[Mouse]::mouse_event(0x0010, 0, 0, 0, [IntPtr]::Zero)
`
	case "middle":
		script += `
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Mouse {
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, IntPtr dwExtraInfo);
}
"@
[Mouse]::mouse_event(0x0020, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 50
[Mouse]::mouse_event(0x0040, 0, 0, 0, [IntPtr]::Zero)
`
	default: // left
		script += `
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Mouse {
    [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, uint dx, uint dy, uint dwData, IntPtr dwExtraInfo);
}
"@
[Mouse]::mouse_event(0x0002, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 50
[Mouse]::mouse_event(0x0004, 0, 0, 0, [IntPtr]::Zero)
`
	}

	execCtx, cancel := context.WithTimeout(ctx, t.timeout)
	defer cancel()

	out, err := t.runPowerShell(execCtx, script)
	if err != nil {
		return ErrorResult(fmt.Sprintf("click_at failed: %v\n%s", err, out))
	}

	return SilentResult(fmt.Sprintf("Clicked at (%d, %d) with %s button", x, y, button))
}

// standaloneTypeText uses PowerShell to send keystrokes.
func (t *DesktopTool) standaloneTypeText(ctx context.Context, text string) *ToolResult {
	// Escape single quotes for PowerShell.
	escaped := strings.ReplaceAll(text, "'", "''")

	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
Start-Sleep -Milliseconds 100
[System.Windows.Forms.SendKeys]::SendWait('%s')
`, escaped)

	execCtx, cancel := context.WithTimeout(ctx, t.timeout)
	defer cancel()

	out, err := t.runPowerShell(execCtx, script)
	if err != nil {
		return ErrorResult(fmt.Sprintf("type_text failed: %v\n%s", err, out))
	}

	return SilentResult(fmt.Sprintf("Typed %d characters", len(text)))
}

// standaloneScreenshot uses PowerShell to capture a screen region via
// System.Drawing. The screenshot is saved as a PNG file under workspace/temp/.
func (t *DesktopTool) standaloneScreenshot(ctx context.Context, x, y, w, h int, hasX, hasY, hasW, hasH bool) *ToolResult {
	// Default to full primary screen if no region specified.
	if !hasX || !hasY || !hasW || !hasH {
		script := `
Add-Type -AssemblyName System.Windows.Forms
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
Write-Output "$($screen.X),$($screen.Y),$($screen.Width),$($screen.Height)"
`
		execCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
		defer cancel()

		out, err := t.runPowerShell(execCtx, script)
		if err != nil {
			return ErrorResult(fmt.Sprintf("failed to get screen bounds: %v", err))
		}

		parts := strings.Split(strings.TrimSpace(out), ",")
		if len(parts) == 4 {
			if !hasX {
				fmt.Sscanf(parts[0], "%d", &x)
			}
			if !hasY {
				fmt.Sscanf(parts[1], "%d", &y)
			}
			if !hasW {
				fmt.Sscanf(parts[2], "%d", &w)
			}
			if !hasH {
				fmt.Sscanf(parts[3], "%d", &h)
			}
		}
	}

	// Generate output path.
	filename := fmt.Sprintf("desktop_screenshot_%d.png", time.Now().UnixMilli())
	outputPath := t.workspace + "/temp/" + filename

	script := fmt.Sprintf(`
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$bounds = New-Object System.Drawing.Rectangle(%d, %d, %d, %d)
$bitmap = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($bounds.Location, [System.Drawing.Point]::Empty, $bounds.Size)
$bitmap.Save('%s', [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()
Write-Output "OK"
`, x, y, w, h, strings.ReplaceAll(outputPath, "'", "''"))

	execCtx, cancel := context.WithTimeout(ctx, t.timeout)
	defer cancel()

	out, err := t.runPowerShell(execCtx, script)
	if err != nil {
		return ErrorResult(fmt.Sprintf("screenshot failed: %v\n%s", err, out))
	}

	return SilentResult(fmt.Sprintf("Screenshot saved to %s (%dx%d at %d,%d)", outputPath, w, h, x, y))
}

// enumerateWindowsPS lists visible windows via PowerShell.
// It uses Get-Process with MainWindowTitle to gather basic window info
// because a full EnumWindows requires P/Invoke which is cumbersome in
// PowerShell. For production use the window-mcp server is recommended.
func (t *DesktopTool) enumerateWindowsPS(ctx context.Context) ([]windowInfo, error) {
	script := `
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
`

	execCtx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()

	out, err := t.runPowerShell(execCtx, script)
	if err != nil {
		return nil, fmt.Errorf("PowerShell enumeration failed: %w\n%s", err, out)
	}

	out = strings.TrimSpace(out)
	if out == "" || out == "null" {
		return nil, nil
	}

	var windows []windowInfo
	if err := json.Unmarshal([]byte(out), &windows); err != nil {
		// PowerShell might return a single object instead of array.
		var single windowInfo
		if err2 := json.Unmarshal([]byte(out), &single); err2 == nil {
			windows = []windowInfo{single}
		} else {
			return nil, fmt.Errorf("failed to parse window list: %w", err)
		}
	}

	return windows, nil
}

// runPowerShell executes a PowerShell script and returns its stdout.
func (t *DesktopTool) runPowerShell(ctx context.Context, script string) (string, error) {
	cmd := exec.CommandContext(ctx, "powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script)
	var stdout, stderr strings.Builder
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	logger.DebugCF("desktop", "Running PowerShell script", map[string]interface{}{
		"script_length": len(script),
	})

	err := cmd.Run()
	out := stdout.String()
	if err != nil {
		if stderr.Len() > 0 {
			out += "\nSTDERR: " + stderr.String()
		}
		return out, err
	}
	return out, nil
}
